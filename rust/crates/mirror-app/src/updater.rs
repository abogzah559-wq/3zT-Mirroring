//! In-app update: HTTPS through WinHTTP, SHA-256 and ECDSA through BCrypt
//! (both part of Windows, no extra crates), and the three ways to install.
//!
//! Nothing is downloaded until the user clicks "Update". A file is only run
//! after its SHA-256 matches the digest GitHub publishes for the release
//! asset and its detached signature verifies against [`UPDATE_KEY`], the
//! public half of the key the release build signs with. [`mirror_core::update`]
//! already refused assets from anywhere but this repository's release
//! downloads.

use std::ffi::c_void;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mirror_core::update::{self, Asset, Channel, Offer, Release, Version};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::Security::Cryptography::*;

/// Public key of the release signing key: ECDSA P-256, X then Y (big-endian).
/// Only files signed with its private half (a GitHub Actions secret) install.
const UPDATE_KEY: [u8; 64] = [
    0x47, 0x4b, 0x36, 0xc6, 0x01, 0x6f, 0xa8, 0x64, 0xf3, 0x70, 0xb9, 0x5c, 0x6a, 0x07, 0x84, 0x44,
    0xc7, 0x8a, 0x90, 0x0f, 0xc9, 0x46, 0x2f, 0x67, 0x95, 0x91, 0xc1, 0x99, 0xb8, 0x81, 0x04, 0xba,
    0x03, 0x87, 0x32, 0x21, 0x26, 0xf3, 0xee, 0x64, 0xc6, 0x10, 0x4d, 0x5d, 0xf0, 0x1f, 0x4c, 0xdb,
    0xe7, 0x57, 0xa2, 0xff, 0x9a, 0x59, 0xef, 0x87, 0xdd, 0xa4, 0x56, 0x5e, 0xd8, 0x29, 0x5b, 0xfe,
];

/// `3zTMirroring.exe --replace-old=<path>`: a portable update started this exe;
/// delete the old one once it has exited.
pub const REPLACE_ARG: &str = "--replace-old=";

/// How this copy of 3zT Mirroring was installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Setup.exe: run the new Setup silently (same AppId, upgrades in place).
    Installed,
    /// Portable.exe: put the new exe next to this one.
    Portable,
    /// A source checkout or portable folder: only point to the release page.
    Source,
}

pub fn kind() -> Kind {
    let Ok(exe) = std::env::current_exe() else {
        return Kind::Source;
    };
    let dir = exe.parent().unwrap_or(Path::new("."));
    if cfg!(feature = "embedded-backend") {
        Kind::Portable
    } else if dir.join("unins000.exe").exists() {
        Kind::Installed
    } else {
        Kind::Source
    }
}

pub fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo version")
}

struct Handle(*mut c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn last_error(what: &str) -> String {
    format!("{what}: {}", windows::core::Error::from_thread())
}

/// An open HTTPS GET with a 2xx answer; [`Response::read`] streams the body.
struct Response {
    _session: Handle,
    _connect: Handle,
    request: Handle,
    pub length: Option<u64>,
}

impl Response {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, String> {
        let mut n = 0u32;
        unsafe {
            WinHttpReadData(
                self.request.0,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut n,
            )
        }
        .map_err(|e| format!("read: {e}"))?;
        Ok(n as usize)
    }
}

/// Splits `https://host/path` (only HTTPS is accepted).
fn split_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    (!host.is_empty() && !host.contains(['@', ':', ' ', '?', '#'])).then_some((host, path))
}

fn get(url: &str, accept: &str) -> Result<Response, String> {
    let (host, path) = split_url(url).ok_or("not an https URL")?;
    let agent = HSTRING::from(format!(
        "3zT-Mirroring-Updater/{}",
        env!("CARGO_PKG_VERSION")
    ));
    unsafe {
        let session = Handle(WinHttpOpen(
            &agent,
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            return Err(last_error("WinHttpOpen"));
        }
        // TLS 1.2 and 1.3 only; resolve, connect, send 15 s, receive 30 s.
        let protocols = (WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3)
            .to_le_bytes();
        let _ = WinHttpSetOption(
            Some(session.0),
            WINHTTP_OPTION_SECURE_PROTOCOLS,
            Some(&protocols),
        );
        let _ = WinHttpSetTimeouts(session.0, 15_000, 15_000, 15_000, 30_000);
        let connect = Handle(WinHttpConnect(
            session.0,
            &HSTRING::from(host),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ));
        if connect.0.is_null() {
            return Err(last_error("WinHttpConnect"));
        }
        let request = Handle(WinHttpOpenRequest(
            connect.0,
            &HSTRING::from("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err(last_error("WinHttpOpenRequest"));
        }
        // Redirects (GitHub sends downloads to its asset host) stay on
        // HTTPS: WinHTTP's default policy refuses HTTPS to HTTP.
        let headers: Vec<u16> = format!("Accept: {accept}\r\n").encode_utf16().collect();
        WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0)
            .map_err(|e| format!("send: {e}"))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())
            .map_err(|e| format!("receive: {e}"))?;
        let number = |info: u32| -> Option<u64> {
            let mut v = 0u32;
            let mut len = 4u32;
            WinHttpQueryHeaders(
                request.0,
                info | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some((&mut v as *mut u32).cast()),
                &mut len,
                std::ptr::null_mut(),
            )
            .ok()
            .map(|_| v as u64)
        };
        let status = number(WINHTTP_QUERY_STATUS_CODE).unwrap_or(0);
        if !(200..300).contains(&status) {
            return Err(format!("HTTP {status}"));
        }
        let length = number(WINHTTP_QUERY_CONTENT_LENGTH);
        Ok(Response {
            _session: session,
            _connect: connect,
            request,
            length,
        })
    }
}

/// The releases feed: GitHub's API, or `MIRROR_UPDATE_FEED` (a local JSON file
/// or another https URL) for tests.
fn feed() -> Result<serde_json::Value, String> {
    let src =
        std::env::var("MIRROR_UPDATE_FEED").unwrap_or_else(|_| update::RELEASES_URL.to_string());
    let bytes = if src.starts_with("https://") {
        let mut r = get(&src, "application/vnd.github+json")?;
        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
            if out.len() > 8 << 20 {
                return Err("feed too large".into());
            }
        }
        out
    } else {
        std::fs::read(&src).map_err(|e| format!("{src}: {e}"))?
    };
    serde_json::from_slice(&bytes).map_err(|e| format!("feed: {e}"))
}

/// Asks GitHub what to offer this build on `channel`.
pub fn check(channel: Channel) -> Result<Offer, String> {
    Ok(update::pick(&feed()?, &current_version(), channel))
}

/// The detached signature of [`asset_for`]'s file.
pub fn signature_for(kind: Kind, r: &Release) -> Option<&Asset> {
    match kind {
        Kind::Installed => r.setup_sig.as_ref(),
        Kind::Portable => r.portable_sig.as_ref(),
        Kind::Source => None,
    }
}

/// The file a user of this install kind needs from `r`.
pub fn asset_for(kind: Kind, r: &Release) -> Option<&Asset> {
    match kind {
        Kind::Installed => r.setup.as_ref(),
        Kind::Portable => r.portable.as_ref(),
        Kind::Source => None,
    }
}

struct Sha256(BCRYPT_HASH_HANDLE);
impl Sha256 {
    fn new() -> Result<Self, String> {
        let mut h = BCRYPT_HASH_HANDLE::default();
        let st = unsafe { BCryptCreateHash(BCRYPT_SHA256_ALG_HANDLE, &mut h, None, None, 0) };
        st.ok().map_err(|e| format!("sha256: {e}"))?;
        Ok(Self(h))
    }
    fn update(&mut self, data: &[u8]) -> Result<(), String> {
        unsafe { BCryptHashData(self.0, data, 0) }
            .ok()
            .map_err(|e| format!("sha256: {e}"))
    }
    fn finish(self) -> Result<[u8; 32], String> {
        let mut out = [0u8; 32];
        unsafe { BCryptFinishHash(self.0, &mut out, 0) }
            .ok()
            .map_err(|e| format!("sha256: {e}"))?;
        Ok(out)
    }
}

fn to_hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of `data`.
fn sha256(data: &[u8]) -> Result<[u8; 32], String> {
    let mut h = Sha256::new()?;
    h.update(data)?;
    h.finish()
}

/// Checks a raw 64-byte P-256 signature (r || s) over `digest` (a SHA-256)
/// against `key` (X || Y).
fn verify_signature(key: &[u8; 64], digest: &[u8; 32], sig: &[u8]) -> Result<(), String> {
    if sig.len() != 64 {
        return Err(format!("signature has {} bytes", sig.len()));
    }
    // BCRYPT_ECCKEY_BLOB: magic "ECS1" (public P-256), key size, then X, Y.
    let mut blob = Vec::with_capacity(8 + 64);
    blob.extend_from_slice(&0x3153_4345u32.to_le_bytes());
    blob.extend_from_slice(&32u32.to_le_bytes());
    blob.extend_from_slice(key);
    let mut handle = BCRYPT_KEY_HANDLE::default();
    unsafe {
        BCryptImportKeyPair(
            BCRYPT_ECDSA_P256_ALG_HANDLE,
            None,
            BCRYPT_ECCPUBLIC_BLOB,
            &mut handle,
            &blob,
            0,
        )
    }
    .ok()
    .map_err(|e| format!("signature key: {e}"))?;
    let st = unsafe { BCryptVerifySignature(handle, None, digest, sig, BCRYPT_FLAGS(0)) };
    unsafe {
        let _ = BCryptDestroyKey(handle);
    }
    st.ok()
        .map_err(|_| "the signature does not match".to_string())
}
impl Drop for Sha256 {
    fn drop(&mut self) {
        unsafe {
            let _ = BCryptDestroyHash(self.0);
        }
    }
}

/// SHA-256 of `data`, lower-case hex.
#[cfg(test)]
pub fn sha256_hex(data: &[u8]) -> Result<String, String> {
    Ok(to_hex(&sha256(data)?))
}

/// Reads a small file (a signature) from `a`, checked against its digest.
fn fetch_small(a: &Asset) -> Result<Vec<u8>, String> {
    if a.size > 4096 {
        return Err("signature file too large".into());
    }
    let mut r = get(&a.url, "application/octet-stream")?;
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if out.len() as u64 > a.size {
            return Err("signature larger than announced".into());
        }
    }
    if to_hex(&sha256(&out)?) != a.sha256 {
        return Err("signature file SHA-256 mismatch".into());
    }
    Ok(out)
}

/// [`download`] plus the release signature: nothing stays at `dest` unless
/// both the SHA-256 and the signature check out.
pub fn download_verified(
    a: &Asset,
    sig: Option<&Asset>,
    dest: &Path,
    progress: &dyn Fn(u64, u64),
) -> Result<(), String> {
    let sig = sig.ok_or("this release is not signed")?;
    let sig = fetch_small(sig)?;
    let digest = download(a, dest, progress)?;
    if let Err(e) = verify_signature(&UPDATE_KEY, &digest, &sig) {
        let _ = std::fs::remove_file(dest);
        return Err(e);
    }
    Ok(())
}

/// Downloads `a` to `dest` (through `dest.part`) and checks its size and
/// SHA-256. `progress(done, total)` runs about every 100 ms. On any problem
/// the partial file is removed and nothing is left at `dest`.
pub fn download(a: &Asset, dest: &Path, progress: &dyn Fn(u64, u64)) -> Result<[u8; 32], String> {
    let part = dest.with_extension("part");
    let result = (|| {
        let mut r = get(&a.url, "application/octet-stream")?;
        if r.length.is_some_and(|l| l != a.size) {
            return Err(format!(
                "size {} instead of {}",
                r.length.unwrap_or(0),
                a.size
            ));
        }
        let mut f = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
        let mut hash = Sha256::new()?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut done = 0u64;
        let mut last = Instant::now();
        loop {
            let n = r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            done += n as u64;
            if done > a.size {
                return Err("larger than announced".into());
            }
            hash.update(&buf[..n])?;
            f.write_all(&buf[..n]).map_err(|e| format!("write: {e}"))?;
            if last.elapsed() >= Duration::from_millis(100) {
                progress(done, a.size);
                last = Instant::now();
            }
        }
        f.sync_all().map_err(|e| format!("write: {e}"))?;
        drop(f);
        progress(done, a.size);
        if done != a.size {
            return Err(format!("got {done} of {} bytes", a.size));
        }
        let digest = hash.finish()?;
        let got = to_hex(&digest);
        if got != a.sha256 {
            return Err(format!("SHA-256 mismatch ({got})"));
        }
        let _ = std::fs::remove_file(dest);
        std::fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        Ok(digest)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// Where an update for `kind` is downloaded.
pub fn download_path(kind: Kind, a: &Asset) -> Result<PathBuf, String> {
    match kind {
        Kind::Portable => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let dir = exe.parent().ok_or("no exe folder")?;
            Ok(dir.join(&a.name))
        }
        _ => {
            let dir = std::env::temp_dir().join(format!("{}-update", mirror_core::DATA_DIR_NAME));
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            Ok(dir.join(&a.name))
        }
    }
}

/// Starts the downloaded file. `replace`: an update of this edition (the
/// caller quits right after); otherwise the other edition (beta/stable),
/// which installs next to this one, so its Setup runs normally and a
/// portable exe just starts.
pub fn launch(kind: Kind, file: &Path, replace: bool) -> Result<(), String> {
    use std::process::Command;
    let mut c = Command::new(file);
    match kind {
        _ if !replace => {}
        Kind::Installed => {
            // Same AppId: upgrades in place and keeps the settings; the
            // firewall rule from the first install stays. Setup needs
            // administrator rights: start it elevated here, so a declined
            // UAC prompt is an error and 3zT Mirroring keeps running.
            return run_elevated(
                file,
                "/SILENT /SP- /SUPPRESSMSGBOXES /NORESTART /MERGETASKS=!firewall /RELAUNCH=1",
            );
        }
        Kind::Portable => {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            if exe == file {
                return Err("already this file".into());
            }
            c.arg(format!("{REPLACE_ARG}{}", exe.display()));
        }
        Kind::Source => return Err("source checkout".into()),
    }
    c.spawn()
        .map(|_| ())
        .map_err(|e| format!("{}: {e}", file.display()))
}

/// Starts `file` with the "runas" verb (UAC). Fails when the user declines.
fn run_elevated(file: &Path, args: &str) -> Result<(), String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let file_w = HSTRING::from(file.as_os_str());
    let args_w = HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: windows::core::w!("runas"),
        lpFile: PCWSTR(file_w.as_ptr()),
        lpParameters: PCWSTR(args_w.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| {
        if e.code() == windows::Win32::Foundation::ERROR_CANCELLED.to_hresult() {
            "the update was cancelled".to_string()
        } else {
            format!("{}: {e}", file.display())
        }
    })?;
    if !info.hProcess.is_invalid() {
        unsafe {
            let _ = CloseHandle(info.hProcess);
        }
    }
    Ok(())
}

/// The old exe a portable update may delete: an `.exe` in this exe's own
/// folder, not this exe.
fn replace_target(arg: &str) -> Option<PathBuf> {
    let old = PathBuf::from(arg.strip_prefix(REPLACE_ARG)?);
    let me = std::env::current_exe().ok()?;
    let ok = old
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        && old.parent() == me.parent()
        && old != me;
    ok.then_some(old)
}

/// Run first in `main`: after a portable update, wait (at most 15 s) until
/// the old 3zT Mirroring has exited and its exe can be deleted, so the
/// single-instance check does not find it.
pub fn finish_portable_update() {
    let Some(old) = std::env::args().find_map(|a| replace_target(&a)) else {
        return;
    };
    let until = Instant::now() + Duration::from_secs(15);
    while old.exists() && Instant::now() < until {
        if std::fs::remove_file(&old).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    if old.exists() {
        log::warn!("update: could not remove {}", old.display());
    } else {
        log::info!("update: removed {}", old.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_signature_verifies_with_the_built_in_key() {
        let digest = sha256(b"3zT Mirroring update test").unwrap();
        let sig: [u8; 64] = [
            0x83, 0xa6, 0x6c, 0xfb, 0x41, 0xfa, 0x17, 0xbb, 0x57, 0x7f, 0xd4, 0xe2, 0xc3, 0x79,
            0x4f, 0x50, 0x5d, 0xa3, 0x0f, 0x8e, 0x9c, 0xda, 0xdd, 0x62, 0x08, 0xb6, 0xd0, 0xa5,
            0x57, 0x5f, 0x92, 0xce, 0x7f, 0x30, 0x42, 0xca, 0xed, 0x92, 0x09, 0xd8, 0xaf, 0xe5,
            0xe4, 0xd3, 0xb2, 0xcc, 0xcd, 0xd2, 0xfb, 0x16, 0xc7, 0x83, 0x7c, 0xd3, 0x77, 0xa8,
            0xe4, 0xac, 0x29, 0xf3, 0x94, 0x51, 0x9b, 0x43,
        ];
        assert!(verify_signature(&UPDATE_KEY, &digest, &sig).is_ok());
        let other = sha256(b"something else").unwrap();
        assert!(verify_signature(&UPDATE_KEY, &other, &sig).is_err());
        let mut bad = sig;
        bad[10] ^= 1;
        assert!(verify_signature(&UPDATE_KEY, &digest, &bad).is_err());
        assert!(verify_signature(&UPDATE_KEY, &digest, &sig[..63]).is_err());
    }

    #[test]
    fn sha256_matches_known_values() {
        assert_eq!(
            sha256_hex(b"").unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn only_https_urls_with_a_plain_host() {
        assert_eq!(
            split_url("https://api.github.com/repos/x?y=1"),
            Some(("api.github.com", "/repos/x?y=1"))
        );
        assert_eq!(split_url("https://github.com"), Some(("github.com", "/")));
        assert!(split_url("http://github.com/x").is_none());
        assert!(split_url("https://user@evil/x").is_none());
        assert!(split_url("https://host:8443/x").is_none());
    }

    #[test]
    fn replace_only_an_exe_next_to_this_one() {
        let me = std::env::current_exe().unwrap();
        let dir = me.parent().unwrap();
        let other = dir.join("3zT Mirroring-old.exe");
        assert_eq!(
            replace_target(&format!("{REPLACE_ARG}{}", other.display())),
            Some(other)
        );
        assert!(replace_target(&format!("{REPLACE_ARG}{}", me.display())).is_none());
        assert!(replace_target(&format!(
            "{REPLACE_ARG}{}",
            dir.join("settings.json").display()
        ))
        .is_none());
        assert!(replace_target(&format!("{REPLACE_ARG}C:\\Windows\\notepad.exe")).is_none());
        assert!(replace_target("--other").is_none());
    }

    #[test]
    fn a_bad_download_leaves_nothing() {
        let dir = std::env::temp_dir().join("mirror-update-test");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("x.exe");
        let a = Asset {
            name: "x.exe".into(),
            url: "http://example.invalid/x.exe".into(),
            size: 1,
            sha256: "00".repeat(32),
        };
        assert!(download(&a, &dest, &|_, _| {}).is_err());
        assert!(!dest.exists());
        assert!(!dest.with_extension("part").exists());
    }

    /// Real network: `cargo test --release -p mirror-app -- --ignored
    /// live_download`. Fetches the GitHub feed and downloads the v2.5.0
    /// Portable.exe through WinHTTP (GitHub redirects it to its asset
    /// host), checking size and SHA-256. The file is deleted, never run.
    #[test]
    #[ignore]
    fn live_download() {
        let list = feed().unwrap();
        let rel = list
            .as_array()
            .unwrap()
            .iter()
            .filter_map(update::release)
            .find(|r| r.tag == "v2.5.0")
            .expect("v2.5.0 in the feed");
        let a = rel.portable.expect("v2.5.0 Portable.exe with a digest");
        let dir = std::env::temp_dir().join("mirror-update-live");
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join(&a.name);
        let t = Instant::now();
        download(&a, &dest, &|_, _| {}).unwrap();
        let len = std::fs::metadata(&dest).unwrap().len();
        eprintln!("{} bytes in {:?}, sha256 {}", len, t.elapsed(), a.sha256);
        assert_eq!(len, a.size);
        std::fs::remove_file(&dest).unwrap();
        // A wrong digest is refused and leaves nothing behind.
        let bad = Asset {
            sha256: "00".repeat(32),
            ..a
        };
        assert!(download(&bad, &dest, &|_, _| {})
            .unwrap_err()
            .contains("SHA-256"));
        assert!(!dest.exists());
    }
}
