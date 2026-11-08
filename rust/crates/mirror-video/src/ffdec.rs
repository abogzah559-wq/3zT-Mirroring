//! HEVC through `videodec.dll` (FFmpeg, D3D11 hardware decoding on the
//! window's own device, software as a last resort).
//!
//! Used when Media Foundation has no HEVC decoder: Windows only ships one
//! with Microsoft's paid "HEVC Video Extensions", while the graphics driver
//! itself decodes HEVC fine. iOS 27 sends the screen only as HEVC.

use std::ffi::c_void;
use std::sync::OnceLock;

use windows::core::{Interface, PCSTR, PCWSTR};
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Texture2D, D3D11_BOX, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_WRITE_DISCARD,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::d3d::Gpu;

#[repr(C)]
pub struct VdFrame {
    pub width: i32,
    pub height: i32,
    pub hardware: i32,
    pub texture: *mut c_void,
    pub slice: i32,
    pub plane: [*const u8; 3],
    pub stride: [i32; 3],
    pub nv12: i32,
}

impl Default for VdFrame {
    fn default() -> Self {
        VdFrame {
            width: 0,
            height: 0,
            hardware: 0,
            texture: std::ptr::null_mut(),
            slice: 0,
            plane: [std::ptr::null(); 3],
            stride: [0; 3],
            nv12: 0,
        }
    }
}

type OpenFn = unsafe extern "C" fn(*mut c_void, *mut i32) -> *mut c_void;
type SendFn = unsafe extern "C" fn(*mut c_void, *const u8, i32) -> i32;
type ReceiveFn = unsafe extern "C" fn(*mut c_void, *mut VdFrame) -> i32;
type CloseFn = unsafe extern "C" fn(*mut c_void);

struct Api {
    open: OpenFn,
    send: SendFn,
    receive: ReceiveFn,
    flush: CloseFn,
    close: CloseFn,
}

/// videodec.dll next to the exe, loaded once (None when it is missing).
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        let path = std::env::current_exe().ok()?.with_file_name("videodec.dll");
        if !path.is_file() {
            return None;
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide_null();
        let lib: HMODULE = LoadLibraryW(PCWSTR(wide.as_ptr())).ok()?;
        let sym = |name: &[u8]| GetProcAddress(lib, PCSTR(name.as_ptr()));
        Some(Api {
            open: std::mem::transmute::<_, OpenFn>(sym(b"vd_open\0")?),
            send: std::mem::transmute::<_, SendFn>(sym(b"vd_send\0")?),
            receive: std::mem::transmute::<_, ReceiveFn>(sym(b"vd_receive\0")?),
            flush: std::mem::transmute::<_, CloseFn>(sym(b"vd_flush\0")?),
            close: std::mem::transmute::<_, CloseFn>(sym(b"vd_close\0")?),
        })
    })
    .as_ref()
}

trait WideNull {
    fn encode_wide_null(&self) -> Vec<u16>;
}

impl WideNull for std::ffi::OsStr {
    fn encode_wide_null(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(std::iter::once(0)).collect()
    }
}

/// Whether the built-in HEVC decoder is installed with the app.
pub fn available() -> bool {
    api().is_some()
}

pub struct FfDec {
    api: &'static Api,
    h: *mut c_void,
    pub hardware: bool,
}

// The handle is only used from the decoder thread that made it.
unsafe impl Send for FfDec {}

impl FfDec {
    pub fn new(gpu: &Gpu) -> Option<Self> {
        let api = api()?;
        let mut hw = 0i32;
        let h = unsafe { (api.open)(gpu.device.as_raw(), &mut hw) };
        if h.is_null() {
            return None;
        }
        Some(FfDec {
            api,
            h,
            hardware: hw != 0,
        })
    }

    pub fn flush(&self) {
        unsafe { (self.api.flush)(self.h) }
    }

    /// Decodes one Annex-B access unit; `sink` gets every finished frame.
    pub fn decode(&mut self, au: &[u8], sink: &mut dyn FnMut(&VdFrame)) -> Result<(), String> {
        for _ in 0..8 {
            let r = unsafe { (self.api.send)(self.h, au.as_ptr(), au.len() as i32) };
            if r < 0 {
                return Err(format!("videodec: send failed ({r})"));
            }
            self.drain(sink)?;
            if r == 0 {
                return Ok(());
            }
        }
        Err("videodec: decoder does not take input".into())
    }

    fn drain(&mut self, sink: &mut dyn FnMut(&VdFrame)) -> Result<(), String> {
        loop {
            let mut f = VdFrame::default();
            match unsafe { (self.api.receive)(self.h, &mut f) } {
                0 => return Ok(()),
                1 => sink(&f),
                r => return Err(format!("videodec: receive failed ({r})")),
            }
        }
    }
}

impl Drop for FfDec {
    fn drop(&mut self) {
        unsafe { (self.api.close)(self.h) }
    }
}

/// Copies a decoded frame into `dst` (an NV12 texture of `w` x `h`).
/// `upload` is a dynamic NV12 texture for software frames, kept between calls.
pub fn copy_frame(
    gpu: &Gpu,
    f: &VdFrame,
    dst: &ID3D11Texture2D,
    w: u32,
    h: u32,
    upload: &mut Option<(ID3D11Texture2D, u32, u32)>,
) -> windows::core::Result<()> {
    unsafe {
        if f.hardware != 0 {
            let Some(src) = ID3D11Texture2D::from_raw_borrowed(&f.texture) else {
                return Ok(());
            };
            let bx = D3D11_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: w,
                bottom: h,
                back: 1,
            };
            gpu.context
                .CopySubresourceRegion(dst, 0, 0, 0, 0, src, f.slice as u32, Some(&bx));
            return Ok(());
        }
        if !matches!(upload, Some((_, uw, uh)) if *uw == w && *uh == h) {
            *upload = Some((gpu.nv12_texture(w, h, true)?, w, h));
        }
        let up = &upload.as_ref().unwrap().0;
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        gpu.context
            .Map(up, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m))?;
        let out = m.pData as *mut u8;
        let pitch = m.RowPitch as usize;
        let (w, h) = (w as usize, h as usize);
        // Luma.
        for y in 0..h {
            std::ptr::copy_nonoverlapping(
                f.plane[0].add(y * f.stride[0] as usize),
                out.add(y * pitch),
                w,
            );
        }
        // Chroma: NV12 interleaved U/V rows after the luma rows.
        let uv = out.add(h * pitch);
        for y in 0..h / 2 {
            let row = uv.add(y * pitch);
            if f.nv12 != 0 {
                std::ptr::copy_nonoverlapping(f.plane[1].add(y * f.stride[1] as usize), row, w);
            } else {
                let u = f.plane[1].add(y * f.stride[1] as usize);
                let v = f.plane[2].add(y * f.stride[2] as usize);
                for x in 0..w / 2 {
                    *row.add(2 * x) = *u.add(x);
                    *row.add(2 * x + 1) = *v.add(x);
                }
            }
        }
        gpu.context.Unmap(up, 0);
        gpu.context.CopyResource(dst, up);
        Ok(())
    }
}

/// The visible rectangle of a `w` x `h` frame.
pub fn full_rect(w: u32, h: u32) -> RECT {
    RECT {
        left: 0,
        top: 0,
        right: w as i32,
        bottom: h as i32,
    }
}
