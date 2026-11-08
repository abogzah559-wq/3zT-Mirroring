//! Client side of the 3zT Mirroring backend protocol.
//!
//! The backend is the Python service on 127.0.0.1:47521, started by the
//! window and stopped with it.

use std::path::{Path, PathBuf};

pub mod api;
pub mod nal;
pub mod sidecar;
pub mod source;
pub mod stream;
pub mod update;

pub const HOST: &str = "127.0.0.1";

/// Backend port.
pub const PORT: u16 = 47521;

/// Product name for window titles and messages.
pub const APP_NAME: &str = "3zT Mirroring";

/// Folder name under %APPDATA% and %LOCALAPPDATA% for prefs, settings and the
/// unpacked backend.
pub const DATA_DIR_NAME: &str = "3zT Mirroring";

/// Window log file, in `log_dir()`.
pub const LOG_FILE_NAME: &str = "mirroring.log";

/// All logs (window, app windows, backend, crashes):
/// `%LOCALAPPDATA%\3zT Mirroring\logs`, %TEMP% when that is unavailable.
/// The backend (mirroring/paths.py LOG_DIR) uses the same folder.
pub fn log_dir() -> PathBuf {
    local_data_dir()
        .map(|d| d.join("logs"))
        .unwrap_or_else(std::env::temp_dir)
}

/// Backend URL. `MIRRORING_PORT` overrides the port (tests, a second instance).
pub fn base_url() -> String {
    let port = std::env::var("MIRRORING_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(PORT);
    format!("http://{HOST}:{port}")
}

/// The app's data folder inside a base folder such as %APPDATA% or %LOCALAPPDATA%.
pub fn data_dir_in(base: &Path) -> PathBuf {
    base.join(DATA_DIR_NAME)
}

/// `%LOCALAPPDATA%\3zT Mirroring`, when LOCALAPPDATA is set.
pub fn local_data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|b| data_dir_in(Path::new(&b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_folders() {
        assert_eq!(APP_NAME, "3zT Mirroring");
        let base = Path::new("base");
        assert_eq!(data_dir_in(base), base.join("3zT Mirroring"));
    }
}
