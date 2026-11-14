//! Windows' own Open dialog (files, or one folder), shown on its own thread
//! so the window's event loop never runs inside the dialog's modal loop.

use std::path::PathBuf;

/// What to pick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// One or more files.
    Files,
    /// One folder.
    Folder,
}

/// Shows the dialog over `owner` (an HWND as isize, 0 = none) and calls
/// `done` on the dialog's thread with the chosen paths (empty = cancelled).
pub fn open(
    owner: isize,
    pick: Pick,
    title: String,
    start_in: Option<PathBuf>,
    done: impl FnOnce(Vec<PathBuf>) + Send + 'static,
) {
    std::thread::spawn(move || done(show(owner, pick, &title, start_in).unwrap_or_default()));
}

#[cfg(windows)]
fn show(
    owner: isize,
    pick: Pick,
    title: &str,
    start_in: Option<PathBuf>,
) -> windows::core::Result<Vec<PathBuf>> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx,
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName,
        FOS_ALLOWMULTISELECT, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
    };

    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let result = (|| {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            let extra = match pick {
                Pick::Files => FOS_ALLOWMULTISELECT,
                Pick::Folder => FOS_PICKFOLDERS,
            };
            dialog.SetOptions(dialog.GetOptions()? | extra | FOS_FORCEFILESYSTEM)?;
            dialog.SetTitle(&HSTRING::from(title))?;
            if let Some(dir) = start_in.filter(|d| d.is_dir()) {
                let path = HSTRING::from(dir.as_os_str());
                if let Ok(item) =
                    SHCreateItemFromParsingName::<_, _, IShellItem>(&path, None::<&IBindCtx>)
                {
                    let _ = dialog.SetFolder(&item);
                }
            }
            let parent = (owner != 0).then_some(HWND(owner as *mut _));
            if dialog.Show(parent).is_err() {
                // Cancelled (or closed).
                return Ok(Vec::new());
            }
            let items = dialog.GetResults()?;
            let mut out = Vec::new();
            for i in 0..items.GetCount()? {
                let item = items.GetItemAt(i)?;
                let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
                if let Ok(s) = name.to_string() {
                    out.push(PathBuf::from(s));
                }
                CoTaskMemFree(Some(name.0 as *const _));
            }
            Ok(out)
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        result
    }
}

#[cfg(not(windows))]
fn show(
    _owner: isize,
    _pick: Pick,
    _title: &str,
    _start_in: Option<PathBuf>,
) -> Result<Vec<PathBuf>, ()> {
    Ok(Vec::new())
}

/// `Pictures\3zT Imports` (the default place for files from the phone).
pub fn default_import_folder() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join("Pictures").join("3zT Imports")
}
