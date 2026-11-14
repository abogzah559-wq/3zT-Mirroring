#![windows_subsystem = "windows"]
//! 3zT Mirroring: Rust shell.

mod android;
mod appwin;
mod backdrop;
mod devprofiles;
mod dots;
mod filedrop;
mod flow;
mod i18n;
mod input;
mod prefs;
mod shortcuts;
mod tray;
mod updater;
mod usbphones;

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use slint::winit_030::winit::platform::windows::{BackdropType, WindowAttributesExtWindows};
use slint::winit_030::winit::window::WindowLevel;
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, LogicalSize, Model};
use windows::Win32::Foundation::HWND;

use mirror_core::api::Api;
use mirror_core::sidecar::Sidecar;
use mirror_video::{Fit, PointerEvent, VideoView};

use input::{Cmd, Input};

slint::include_modules!();

static SIDECAR: OnceLock<Mutex<Option<Sidecar>>> = OnceLock::new();
static DEMO_T0: OnceLock<Instant> = OnceLock::new();

/// Minimal file logger in `mirror_core::log_dir()`. Each start begins a new
/// file; the previous run is kept as `<name>.prev.log` for bug reports.
struct FileLog(Mutex<std::fs::File>, Instant);

impl log::Log for FileLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record) {
        use std::io::Write;
        if self.enabled(r.metadata()) {
            let t = self.1.elapsed().as_secs_f64();
            let _ = writeln!(
                self.0.lock().unwrap(),
                "{t:>9.3}s [{}] {}: {}",
                r.level(),
                r.target(),
                r.args()
            );
        }
    }
    fn flush(&self) {}
}

/// Local wall-clock time "YYYY-MM-DD HH:MM:SS" (no date crate needed).
fn local_time() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

fn init_log() {
    init_log_named(mirror_core::LOG_FILE_NAME);
}

/// App windows log to their own file, so they never truncate the main log.
fn init_log_named(name: &str) {
    use std::io::Write;
    let dir = mirror_core::log_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(name);
    let _ = std::fs::rename(&path, path.with_extension("prev.log"));
    if let Ok(mut f) = std::fs::File::create(path) {
        let _ = writeln!(
            f,
            "3zT Mirroring {} | started {} | {} | pid {}",
            env!("CARGO_PKG_VERSION"),
            local_time(),
            windows_version(),
            std::process::id()
        );
        let l: &'static FileLog = Box::leak(Box::new(FileLog(Mutex::new(f), Instant::now())));
        let _ = log::set_logger(l).map(|()| log::set_max_level(log::LevelFilter::Info));
    }
}

/// "Windows build 26200.1234 (25H2)" from the registry, for bug reports.
fn windows_version() -> String {
    use windows::core::{w, PCWSTR};
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    };
    let key = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    let text = |name: PCWSTR| -> String {
        let mut buf = [0u16; 64];
        let mut len = (buf.len() * 2) as u32;
        let ok = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key,
                name,
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut len),
            )
        }
        .is_ok();
        if !ok {
            return String::new();
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n])
    };
    let mut ubr = 0u32;
    let mut len = 4u32;
    let _ = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            w!("UBR"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut ubr as *mut u32).cast()),
            Some(&mut len),
        )
    };
    format!(
        "Windows build {}.{} ({})",
        text(w!("CurrentBuild")),
        ubr,
        text(w!("DisplayVersion"))
    )
}

#[derive(Default)]
struct App {
    /// Last "touch control is off" reason shown.
    control_note: String,
    /// The "F2 shows the controls" hint was shown for this picture.
    clean_hint: bool,
    view: Option<VideoView>,
    streaming: bool,
    device_rotation: u32,
    /// (shown w, shown h, settings open, clean view) the window was last fitted to.
    fitted: Option<(u32, u32, bool, bool)>,
    /// Logical window size 3zT Mirroring itself last set while fitting; any other
    /// size while streaming means the user resized the window.
    fit_set: Option<(f32, f32)>,
    /// Picture size the user chose by resizing, relative to the default fit
    /// (0 = never resized). Kept when the picture turns or settings change.
    user_scale: f32,
    /// Picture prefs the window was fitted for (fit, rotation, crop, follow):
    /// only these may resize the window, not volume or other settings.
    fit_prefs: Option<String>,
    meter_at: Option<Instant>,
    /// Apple session went live at; cleared once a frame arrived or after the
    /// "no picture" hint.
    live_since: Option<Instant>,
    dark: bool,
    backdrop: Option<(bool, bool, i32, bool)>,
    on_top: Option<bool>,
    dots: Option<dots::Dots>,
    // Stage width (logical) to keep while the settings panel slides.
    hold_stage: Option<f32>,
    /// Settings slide: from, to, start, duration.
    slide: Option<(f32, f32, Instant, Duration)>,
    dots_at: Option<Instant>,
    /// A mode choice was just sent: keep the chooser closed until the
    /// backend's state catches up.
    mode_sent: Option<Instant>,
    /// A choice is waiting for the backend (it may still be starting).
    mode_pending: bool,
    /// Result of the last "Check drivers", shown under the service line.
    drivers_text: Option<String>,
    /// Device overlay 3zT Mirroring opened last ("control-center" / "notifications")
    /// and when. Forgotten after a click on the picture or [`PANEL_TTL`]:
    /// the user may have closed it on the device, and a stale "open" made
    /// the next press send the close swipe (swipe up from the bottom), which
    /// left the app instead.
    panel: Option<(&'static str, Instant)>,
    /// Connect card: when Start was pressed (the backend may still
    /// say "idle" for a moment).
    flow_started: Option<Instant>,
    /// USB instance id of the device, for driver changes.
    drv_instance: Option<String>,
    /// What the open notice sheet is for ("drv:restore", "restart", "info").
    sheet: Option<String>,
    /// Full screen: where the window was before (position, size, maximised).
    full_restore: Option<(slint::PhysicalPosition, slint::PhysicalSize, bool)>,
    /// Startup navigation: platform screen, Apple or Android.
    flow: Option<flow::Flow>,
    /// The backend's last "needsChoice" (None until it first answered).
    apple_needs: Option<bool>,
    /// Android session; `None` when none runs.
    android: Option<Arc<android::Shared>>,
    /// The Android picture is live (the window shows video, fits to it).
    android_live: bool,
    /// Android status last shown in the window.
    android_shown: Option<android::Status>,
    /// USB / Wi-Fi of the device being mirrored.
    android_link: String,
    /// Which list entry was started (-1 = first ready), for Restart.
    android_index: i32,
    /// Height the first sizing chose, and when (see clamp_to_work_area).
    start_h: Option<(f32, Instant)>,
    /// Apple phase seen last (auto unlock fires on the step to streaming).
    apple_phase: String,
    /// When to send the auto-unlock (Apple Driver mode, picture running).
    unlock_at: Option<Instant>,
    /// Whether the backend was asked if a passcode is saved.
    passcode_asked: bool,
    /// Link the user picked for a phone on both USB and Wi-Fi
    /// (Some(true) = Wi-Fi); kept for restarts.
    android_via: Option<bool>,
    /// Settings the running Android session started with (sound target,
    /// picture options); a change restarts it.
    android_key: Option<String>,
    /// A source waiting for the video view to exist.
    android_pending: Option<Box<dyn mirror_core::source::VideoSource>>,
    /// Devices the Android workspace shows, in row order.
    android_list: Vec<mirror_android::discovery::AndroidDevice>,
    /// App windows: the device whose apps the picker shows, its
    /// apps (cached per device key) and the rows shown after the filter.
    apps_device: Option<mirror_android::discovery::AndroidDevice>,
    apps_cache: std::collections::HashMap<String, Vec<mirror_android::apps::DeviceApp>>,
    apps_shown: Vec<mirror_android::apps::DeviceApp>,
    /// MIRROR_ANDROID_FIXTURE: started once when the Android workspace opens.
    fixture_started: bool,
    /// Per-device Android settings, loaded on first use.
    android_store: Option<mirror_android::profiles::Store>,
    /// Stable key of the device last started: Settings edits go to it.
    android_dev: Option<String>,
    /// Encoder lists read this run, per device key (skips the query on
    /// the next start of the same phone).
    android_encoders: std::collections::HashMap<String, Vec<mirror_android::encoders::Encoder>>,
    /// This session's working configuration was saved (lastGood).
    android_good_saved: bool,
}

/// 3zT Mirroring's own full screen (the window covers the monitor). winit's full
/// screen left Windows' glass blur grey afterwards (light theme after dark)
/// until the app restarted.
fn is_full() -> bool {
    APP.with_borrow(|a| a.full_restore.is_some())
}

/// Physical bounds of the monitor the window is on.
/// The first time the window is on screen: 85% of the work area's height
/// and up to 55% of its width (the old start was too small and
/// too narrow), kept inside the work area on small or high-DPI screens.
fn clamp_to_work_area(ui: &AppWindow) {
    let Some((wx, wy, ww, wh)) = work_area(ui) else {
        return;
    };
    let cur = ui.window().size().to_logical(ui.window().scale_factor());
    let h = cur.height.max(wh * 0.85).min(wh * 0.95);
    // Other resizes in the next ticks still read the old height (the size
    // arrives with a later event): they keep at least this one for 2 s.
    APP.with_borrow_mut(|a| a.start_h = Some((h, Instant::now())));
    // Room for the device panel and Settings side by side: 55% of the
    // screen width, at most 1.35 x the height.
    let w = cur.width.max((ww * 0.55).min(h * 1.35)).min(ww * 0.95);
    if (w, h) != (cur.width, cur.height) {
        ui.window().set_size(LogicalSize::new(w, h));
    }
    // Open in the middle of the screen (Windows' default put it at the top left).
    let (x, y) = centered((wx, wy, ww, wh), (w, h));
    ui.window().set_position(slint::LogicalPosition::new(x, y));
}

/// Top-left corner that centres a `w` x `h` window in the work area.
fn centered((wx, wy, ww, wh): (f32, f32, f32, f32), (w, h): (f32, f32)) -> (f32, f32) {
    (
        (wx + (ww - w) / 2.0).max(wx).round(),
        (wy + (wh - h) / 2.0).max(wy).round(),
    )
}

/// CSS `cubic-bezier(0.05, 0.7, 0.1, 1)` (the settings slide curve)
/// at time `x` in 0..=1.
fn slide_ease(x: f32) -> f32 {
    let (x1, y1, x2, y2) = (0.05f32, 0.7f32, 0.1f32, 1.0f32);
    let bez = |t: f32, a: f32, b: f32| {
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    };
    // x(t) is monotonic: bisection finds t for x.
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if bez(mid, x1, x2) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez((lo + hi) / 2.0, y1, y2)
}

fn monitor_rect(ui: &AppWindow) -> Option<(i32, i32, u32, u32)> {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    let h = hwnd_of(ui.window())?;
    unsafe {
        let m = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(m, &mut mi).as_bool() {
            return None;
        }
        let r = mi.rcMonitor;
        Some((
            r.left,
            r.top,
            (r.right - r.left) as u32,
            (r.bottom - r.top) as u32,
        ))
    }
}

thread_local! {
    static APP: RefCell<App> = RefCell::new(App::default());
    static START: Instant = Instant::now();
}

fn hwnd_of(w: &slint::Window) -> Option<HWND> {
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    w.with_winit_window(|ww| match ww.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as _)),
        _ => None,
    })
    .flatten()
}

/// Windows "app mode" (Settings > Personalization > Colors).
fn system_dark() -> bool {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let mut v = 1u32;
    let mut n = 4u32;
    unsafe {
        let _ = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut _ as _),
            Some(&mut n),
        );
    }
    v == 0
}

fn apply_look(ui: &AppWindow) {
    let p = ui.get_prefs();
    let dark = match p.theme.as_str() {
        "dark" => true,
        "system" => system_dark(),
        _ => false,
    };
    ui.global::<Theme>().set_dark(dark);
    // Thickness 0..100 -> clarity: 0% thick = 1.0 (clear), 100% = 0.25.
    let thick = (p.glass / 100.0).clamp(0.0, 1.0);
    ui.global::<Theme>()
        .set_glass(if p.glass_on { 1.0 - 0.75 * thick } else { 0.0 });
    let glass_on = p.glass_on;
    let glass_k = thick;
    // Square corners in full screen / maximised (rounded ones show the desktop).
    let full = is_full();
    let round = !full && !ui.window().is_maximized();
    APP.with_borrow_mut(|a| {
        a.dark = dark;
        let glass_q = (glass_k * 20.0).round() as i32; // re-apply in 5% steps
                                                       // Full screen paints a solid background, so no blur there. A dark
                                                       // blur kept through full screen came back grey afterwards (also in
                                                       // light theme) until the app restarted; turning it off avoids that.
        let blur = glass_on && !full;
        if a.backdrop != Some((blur, dark, glass_q, round)) {
            if let Some(h) = hwnd_of(ui.window()) {
                let mode = if blur {
                    backdrop::Backdrop::Accent
                } else {
                    backdrop::Backdrop::None
                };
                backdrop::apply(h, mode, dark, glass_k, round);
                a.backdrop = Some((blur, dark, glass_q, round));
            }
        }
        if a.on_top != Some(p.on_top) {
            let lvl = if p.on_top {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            };
            ui.window().with_winit_window(|w| w.set_window_level(lvl));
            a.on_top = Some(p.on_top);
        }
    });
}

/// Asks the backend whether a passcode is saved, or saves one (`Some`;
/// an empty string removes it). Off the UI thread.
fn passcode_query(ui: &AppWindow, api: &Api, save: Option<String>) {
    let api = api.clone();
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let r = match &save {
            Some(code) => api.post("/api/passcode", &json!({ "passcode": code })),
            None => api.get("/api/passcode"),
        };
        let set = r
            .as_ref()
            .and_then(|v| v.get("set"))
            .and_then(Value::as_bool);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            match set {
                Some(s) => {
                    ui.set_passcode_set(s);
                    if save.is_some() {
                        let msg = if s {
                            tr_ui(&ui, "Passcode saved (encrypted on this PC)")
                        } else {
                            tr_ui(&ui, "Passcode removed")
                        };
                        show_toast(&ui, msg);
                    }
                }
                None if save.is_some() => show_toast(
                    &ui,
                    tr_ui(
                        &ui,
                        "Couldn't save the passcode: the 3zT Mirroring service is not running",
                    ),
                ),
                None => APP.with_borrow_mut(|a| a.passcode_asked = false),
            }
        });
    });
}

fn push_audio(api: &Api, p: &Prefs, muted: bool) {
    let api = api.clone();
    let body = json!({ "enabled": p.audio_out.as_str() == "pc", "volume": p.volume / 100.0, "muted": muted });
    android::set_audio(p.audio_out.as_str() == "pc", p.volume / 100.0, muted);
    std::thread::spawn(move || api.audio(&body));
}

fn str_of<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for k in path {
        cur = cur.get(*k)?;
    }
    cur.as_str()
}

fn tr_ui(_ui: &AppWindow, en: &str) -> String {
    i18n::tr(en)
}

/// The Apple device list: connected devices first, then devices trusted
/// before that are not connected now. `current` is the one Start uses.
fn apple_rows(ui: &AppWindow, st: &Value, current: &str) -> Vec<AppleDev> {
    let tr = |en: &str| tr_ui(ui, en);
    let mut rows = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for d in st
        .get("devices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = str_of(d, &["id"]) else {
            continue;
        };
        if id == "airplay" || !seen.insert(id.to_string()) {
            continue;
        }
        let model = str_of(d, &["name"]).unwrap_or("iPhone/iPad");
        let title = str_of(d, &["deviceName"]).unwrap_or(model);
        let wifi = d.get("wifi").and_then(Value::as_bool).unwrap_or(false);
        let ready = d.get("ready").and_then(Value::as_bool).unwrap_or(false);
        let need = str_of(d, &["need"]).unwrap_or("");
        let how = if wifi { "Wi-Fi" } else { "USB" };
        let status = match need {
            "" if ready => tr("ready"),
            "" | "trust" => tr("unlock + Trust"),
            "wifi" => tr("Wi-Fi not set up"),
            "mode" => tr("choose a mode"),
            "restore" => tr("Studio mode driver"),
            _ => tr("needs a driver"),
        };
        let os = str_of(d, &["os"])
            .map(|v| format!(" · iOS {v}"))
            .unwrap_or_default();
        let sub = if title == model {
            format!("{how}{os} · {status}")
        } else {
            format!("{model} · {how}{os} · {status}")
        };
        rows.push(AppleDev {
            id: id.into(),
            title: title.into(),
            sub: sub.into(),
            state: if ready && need.is_empty() {
                "ready"
            } else {
                "need"
            }
            .into(),
            wifi,
            selected: id == current,
        });
    }
    for d in st
        .get("known")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = str_of(d, &["id"]) else {
            continue;
        };
        if !seen.insert(id.to_string()) {
            continue;
        }
        let model = str_of(d, &["name"]).unwrap_or("iPhone/iPad");
        let title = str_of(d, &["deviceName"]).unwrap_or(model);
        let where_ = if ui.get_mode_link().as_str() == "wireless" {
            tr("not seen on Wi-Fi")
        } else {
            tr("not connected")
        };
        let sub = if title == model {
            tr(&format!("Saved · {where_}"))
        } else {
            tr(&format!("{model} · saved · {where_}"))
        };
        rows.push(AppleDev {
            id: id.into(),
            title: title.into(),
            sub: sub.into(),
            state: "offline".into(),
            wifi: false,
            selected: id == str_of(st, &["selected"]).unwrap_or("") && current.is_empty(),
        });
    }
    rows
}

/// Backend state -> UI. Runs on the UI thread.
fn on_state(ui: &AppWindow, api: &Api, st: &Value) {
    let tr = i18n::tr;
    let phase = str_of(st, &["phase"]).unwrap_or("idle").to_string();
    // Why touch control is off while the picture runs (iOS 26 and older,
    // Developer Mode...): say it once per reason.
    let control_note = st
        .get("control")
        .filter(|c| c.get("control").and_then(Value::as_bool) == Some(false))
        .and_then(|c| str_of(c, &["note"]))
        .unwrap_or("")
        .to_string();
    let new_note = APP.with_borrow_mut(|a| {
        if a.control_note != control_note {
            a.control_note = control_note.clone();
            !control_note.is_empty()
        } else {
            false
        }
    });
    if new_note {
        show_toast(ui, control_note);
    }
    // The backend starts the picked device when it is connected, otherwise
    // the first ready one; show that one.
    let list = st.get("devices").and_then(Value::as_array);
    let selected = str_of(st, &["selected"]).unwrap_or("");
    let first = list
        .and_then(|l| l.iter().find(|d| str_of(d, &["id"]) == Some(selected)))
        .or_else(|| {
            list.and_then(|l| {
                l.iter()
                    .find(|d| d.get("ready").and_then(Value::as_bool).unwrap_or(false))
            })
        })
        .or_else(|| list.and_then(|l| l.first()));
    let first_id = first.and_then(|d| str_of(d, &["id"])).unwrap_or("");
    let rows = apple_rows(ui, st, first_id);
    ui.set_apple_devices(std::rc::Rc::new(slint::VecModel::from(rows)).into());
    let device = str_of(st, &["device", "name"])
        .or_else(|| first.and_then(|d| str_of(d, &["name"])))
        .unwrap_or("");
    let wifi = first
        .and_then(|d| d.get("wifi"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let backend = str_of(st, &["capture", "backend"])
        .or_else(|| str_of(st, &["connection", "active"]))
        .unwrap_or("");
    let link = match backend {
        "usbdirect" => tr("Studio mode"),
        "apple" | "hybrid" => format!("Apple · {}", if wifi { "Wi-Fi" } else { "USB" }),
        _ if wifi => "Wi-Fi".into(),
        _ if !device.is_empty() => "USB".into(),
        _ => String::new(),
    };
    let message = if phase == "error" {
        str_of(st, &["error"]).unwrap_or("").to_string()
    } else {
        String::new()
    };
    // Auto unlock: Apple Driver mode only (the backend refuses Studio),
    // once the picture runs, if a passcode is saved and the option is on.
    let was = APP.with_borrow_mut(|a| std::mem::replace(&mut a.apple_phase, phase.clone()));
    if phase == "streaming" && was != "streaming" {
        // A new session: the device keyboard starts in English.
        IOS_ARABIC.store(false, Relaxed);
    }
    // Whether a passcode is saved is checked when it fires (the first
    // status can arrive before the passcode query answers).
    if phase == "streaming"
        && was != "streaming"
        && matches!(backend, "apple" | "hybrid")
        && ui.get_prefs().auto_unlock
    {
        APP.with_borrow_mut(|a| a.unlock_at = Some(Instant::now() + Duration::from_millis(1500)));
    }
    if !APP.with_borrow_mut(|a| std::mem::replace(&mut a.passcode_asked, true)) {
        passcode_query(ui, api, None);
    }
    ui.set_phase(phase.clone().into());
    ui.set_device(device.into());
    ui.set_link(link.into());
    ui.set_message(tr(&message).into());
    let conn = match str_of(st, &["connection", "active"]) {
        Some("apple") => tr("Apple Driver (Standard)"),
        Some("usbdirect") => tr("Studio mode"),
        _ => tr("Not chosen yet"),
    };
    ui.set_connection(conn.into());
    // Connection mode chooser: nothing connects until a mode is chosen.
    let c = st.get("connection");
    let needs = c
        .and_then(|c| c.get("needsChoice"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    APP.with_borrow_mut(|a| a.apple_needs = Some(needs));
    ui.set_apple_detected(!device.is_empty());
    // Only the Apple workspace may show the Apple chooser (flow.rs).
    let on_apple = APP.with_borrow_mut(|a| match a.flow.as_mut() {
        Some(f) => {
            if needs {
                f.backend_needs_choice();
            }
            f.apple_active()
        }
        None => false,
    });
    ui.set_mode_active(str_of(st, &["connection", "active"]).unwrap_or("").into());
    ui.set_mode_link(
        str_of(st, &["connection", "link"])
            .unwrap_or("wired")
            .into(),
    );
    ui.set_mode_remember(
        c.and_then(|c| c.get("remember"))
            .and_then(Value::as_bool)
            .unwrap_or(true)
            || needs,
    );
    let just_sent = APP.with_borrow(|a| {
        a.mode_pending
            || a.mode_sent
                .map(|t| t.elapsed() < Duration::from_secs(3))
                .unwrap_or(false)
    });
    if needs && !just_sent && on_apple && !ui.get_chooser_open() {
        ui.set_chooser_cancelable(false);
        open_chooser(ui);
    }
    // Opened at launch before the backend answered, but a remembered mode
    // is already active: nothing to choose.
    if !needs && !just_sent && ui.get_chooser_open() && !ui.get_chooser_cancelable() {
        ui.set_chooser_open(false);
    }
    if let Some(p) = str_of(st, &["connection", "pending"]) {
        let name = if p == "apple" {
            "Apple Driver"
        } else {
            "Studio mode"
        };
        ui.set_connection(
            format!(
                "{} · {}",
                ui.get_connection(),
                tr(&format!("{name} after an app restart"))
            )
            .into(),
        );
    }
    let service = st.get("service").and_then(Value::as_bool).unwrap_or(true);
    let native_audio = st
        .get("nativeAudio")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let checked = APP.with_borrow(|a| {
        a.drivers_text
            .clone()
            .map(|t| format!("\n{t}"))
            .unwrap_or_default()
    });
    ui.set_driver_text(
        format!(
            "Apple Mobile Device Service: {}\n{}: {}{checked}",
            if service {
                tr("running")
            } else {
                tr("not running")
            },
            tr("Native audio"),
            if native_audio {
                tr("available")
            } else {
                tr("not available")
            },
        )
        .into(),
    );
    // Connect card (1-2-3-4). AirPlay / Hybrid wait for the device
    // to pick 3zT Mirroring in Screen Mirroring: no card then (the guide shows).
    let airplay = first
        .map(|d| {
            d.get("airplay").and_then(Value::as_bool).unwrap_or(false)
                || d.get("hybrid").and_then(Value::as_bool).unwrap_or(false)
        })
        .unwrap_or(false);
    let kind = if str_of(st, &["connection", "active"]) == Some("usbdirect") {
        "usbdirect"
    } else {
        "apple"
    };
    ui.set_flow_kind(kind.into());
    // Studio mode over Wi-Fi: the receiver waits for the device to pick it.
    let receiver = first
        .and_then(|d| d.get("airplay"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    ui.set_apple_listening(receiver && matches!(phase.as_str(), "connecting" | "activating"));
    ui.set_device_wifi(
        wifi || str_of(st, &["device", "wifi"]).is_some()
            && st["device"]["wifi"].as_bool().unwrap_or(false),
    );
    // What the device still needs before Start can work ("" = ready).
    let need = first
        .map(|d| match d.get("need").and_then(Value::as_str) {
            Some(n) => n.to_string(),
            None if !d.get("ready").and_then(Value::as_bool).unwrap_or(true) => "trust".into(),
            None => String::new(),
        })
        .unwrap_or_default();
    ui.set_device_need(need.into());
    // Driver buttons follow the device's driver.
    let drv = st.get("drivers");
    let dev = drv.and_then(|d| d.get("devices")).and_then(|d| d.get(0));
    let lib = drv
        .and_then(|d| d.pointer("/libusb0/available"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let unsafe_filter = dev
        .and_then(|d| d.get("unsafeFilter"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let dmode = dev.and_then(|d| str_of(d, &["mode"])).unwrap_or("");
    ui.set_drv_filter(dev.is_some() && unsafe_filter);
    ui.set_drv_restore(
        dev.is_some() && !unsafe_filter && (dmode == "replaced" || dmode == "unknown"),
    );
    ui.set_drv_bind(dev.is_some() && !unsafe_filter && dmode == "apple" && lib);
    APP.with_borrow_mut(|a| {
        a.drv_instance = dev.and_then(|d| str_of(d, &["instance"])).map(String::from)
    });
    let fresh = APP.with_borrow(|a| {
        a.flow_started
            .map(|t| t.elapsed() < Duration::from_secs(3))
            .unwrap_or(false)
    });
    let step = ui.get_flow_step();
    match phase.as_str() {
        "connecting" | "activating" if airplay => ui.set_flow_step(-1),
        "connecting" | "activating" => {
            let n = if phase == "connecting" { 1 } else { 2 };
            ui.set_flow_failed(false);
            ui.set_flow_step(step.max(n));
        }
        "streaming" => {
            ui.set_flow_step(-1);
            ui.set_flow_failed(false);
            APP.with_borrow_mut(|a| a.flow_started = None);
        }
        "error" if step >= 0 => ui.set_flow_failed(true),
        _ if step >= 0 && !fresh && !ui.get_flow_failed() => ui.set_flow_step(-1),
        _ => {}
    }
    let rot = st.get("rotation").and_then(Value::as_u64).unwrap_or(0) as u32;
    let live = phase == "streaming";
    APP.with_borrow_mut(|a| {
        a.device_rotation = if [0, 90, 180, 270].contains(&rot) {
            rot
        } else {
            0
        };
        if live != a.streaming {
            a.streaming = live;
            a.fitted = None;
            if let Some(v) = a.view.as_mut() {
                if live {
                    v.start(api.clone());
                } else {
                    v.stop();
                }
            }
            a.live_since = live.then(Instant::now);
            if live {
                log::info!("apple: streaming ({backend}, device {device:?})");
                push_audio(api, &ui.get_prefs(), ui.get_muted());
            }
        }
    });
}

fn effective_rotation(p: &Prefs, shown: (u32, u32), device_rotation: u32) -> u32 {
    match p.rotation.as_str() {
        "auto" => {
            if shown.0 > shown.1 {
                0
            } else {
                device_rotation
            }
        }
        r => r.parse().unwrap_or(0),
    }
}

/// Logical work area of the monitor the window is on.
fn work_area(ui: &AppWindow) -> Option<(f32, f32, f32, f32)> {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    let h = hwnd_of(ui.window())?;
    let sf = ui.window().scale_factor();
    unsafe {
        let m = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(m, &mut mi).as_bool() {
            return None;
        }
        let r = mi.rcWork;
        Some((
            r.left as f32 / sf,
            r.top as f32 / sf,
            (r.right - r.left) as f32 / sf,
            (r.bottom - r.top) as f32 / sf,
        ))
    }
}

/// Size the window around the picture (portrait or landscape).
/// Gap between the picture and the glass stage edge (logical px).
const PIC_INSET: f32 = 6.0;

/// The gap in use. In the clean view the window is the picture, with a thin
/// edge left free so the window can still be resized with the mouse (the
/// native picture covers everything under it).
fn pic_inset(ui: &AppWindow) -> f32 {
    if ui.get_clean() {
        CLEAN_EDGE
    } else {
        PIC_INSET
    }
}

/// Free edge around the picture in the clean view (logical px).
const CLEAN_EDGE: f32 = 4.0;

/// Smallest window size: the header's width while it shows, else only a
/// sane minimum for the bare picture.
fn min_window(ui: &AppWindow) -> (f32, f32) {
    if ui.get_clean() {
        (160.0, 160.0)
    } else {
        (MIN_W, 420.0)
    }
}
/// Narrowest window that still fits the header (logo, name, buttons,
/// window controls).
const MIN_W: f32 = 520.0;
/// How long 3zT Mirroring trusts that an overlay it opened is still open.
const PANEL_TTL: Duration = Duration::from_secs(20);

/// Stage width (logical px) while there is no picture yet: the smallest
/// one; [`idle_stage_w`] widens it on larger screens.
const IDLE_STAGE_W: f32 = 440.0;

/// Idle stage width for this screen: 32% of the work area, 440-640 px
/// (440 px next to Settings is too narrow on a large screen).
fn idle_stage_w(ui: &AppWindow) -> f32 {
    work_area(ui)
        .map(|(_, _, ww, _)| (ww * 0.32).clamp(IDLE_STAGE_W, 640.0))
        .unwrap_or(IDLE_STAGE_W)
}

/// Picture prefs that change the window's fitted size.
fn fit_prefs_key(p: &Prefs) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}",
        p.fit, p.rotation, p.fit_window, p.crop_l, p.crop_t, p.crop_r, p.crop_b
    )
}

/// Default fitted picture size (logical px, without chrome) for `shown`.
fn default_fit(ui: &AppWindow, shown: (u32, u32)) -> Option<(f32, f32)> {
    let (_, _, ww, wh) = work_area(ui)?;
    let chrome_w = ui.get_chrome_w() + 2.0 * pic_inset(ui);
    let chrome_h = ui.get_chrome_h() + 2.0 * pic_inset(ui);
    let aspect = shown.0 as f32 / shown.1.max(1) as f32;
    // The window takes up to 85% of the screen's height (was 62%: too small).
    let max_h = wh * 0.85 - chrome_h;
    let max_w = ww * 0.6 - chrome_w;
    let (mut sw, mut sh) = (max_h * aspect, max_h);
    if sw > max_w {
        sw = max_w;
        sh = sw / aspect;
    }
    Some((sw.max(1.0), sh.max(1.0)))
}

/// Fit the window to the picture. `scale` > 0: the size the user chose by
/// resizing, relative to the default fit (kept within the screen).
fn fit_window(ui: &AppWindow, shown: (u32, u32), scale: f32) -> Option<(f32, f32)> {
    let (wx, wy, ww, wh) = work_area(ui)?;
    let chrome_w = ui.get_chrome_w() + 2.0 * pic_inset(ui);
    let chrome_h = ui.get_chrome_h() + 2.0 * pic_inset(ui);
    let (mut sw, mut sh) = default_fit(ui, shown)?;
    if scale > 0.0 {
        let k = scale
            .min((ww - chrome_w) / sw)
            .min((wh - chrome_h) / sh)
            .max(0.2);
        sw *= k;
        sh *= k;
    }
    let (min_w, min_h) = min_window(ui);
    let size = LogicalSize::new((sw + chrome_w).max(min_w), (sh + chrome_h).max(min_h));
    ui.window().set_size(size);
    keep_on_screen(ui, (wx, wy, ww, wh), size);
    Some((size.width, size.height))
}

/// Show the connection mode chooser, widening the window so the sheet fits.
fn open_chooser(ui: &AppWindow) {
    ui.set_chooser_open(true);
    widen_for_sheet(ui);
}

/// Widens the window so a full-window sheet (mode chooser, platform screen) fits.
fn widen_for_sheet(ui: &AppWindow) {
    let Some((wx, wy, ww, wh)) = work_area(ui) else {
        return;
    };
    let cur = ui.window().size().to_logical(ui.window().scale_factor());
    // Never narrower than the idle stage plus the open settings panel (the
    // window may not have taken that size yet when this runs at launch).
    let want = if ui.get_settings_open() {
        idle_stage_w(ui) + 16.0 + 368.0
    } else {
        600.0
    };
    let w = cur.width.max(want.max(600.0).min(ww - 16.0));
    let start = APP
        .with_borrow(|a| a.start_h)
        .filter(|(_, at)| at.elapsed() < Duration::from_secs(2))
        .map_or(0.0, |(h, _)| h);
    let h = cur.height.max(start).max(820.0f32.min(wh - 16.0));
    if (w, h) != (cur.width, cur.height) {
        let size = LogicalSize::new(w, h);
        ui.window().set_size(size);
        keep_on_screen(ui, (wx, wy, ww, wh), size);
    }
}

/// True when the backend's settings.json remembers a connection mode
/// (source checkout: next to mirroring/; installed: %LOCALAPPDATA%\3zT Mirroring).
fn mode_remembered() -> bool {
    let mut files = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors().skip(1) {
            if dir.join("mirroring").join("connection.py").exists()
                || dir.join("portable.txt").exists()
            {
                files.push(dir.join("settings.json"));
                break;
            }
        }
    }
    // Installed and portable builds keep settings in %LOCALAPPDATA%\3zT Mirroring;
    // a source checkout or portable folder only uses its own file.
    if files.is_empty() {
        if let Some(d) = mirror_core::local_data_dir() {
            files.push(d.join("settings.json"));
        }
    }
    files
        .iter()
        .filter_map(|f| std::fs::read(f).ok())
        .filter_map(|b| serde_json::from_slice::<Value>(&b).ok())
        .any(|v| {
            v.get("rememberConnectionMode")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && matches!(
                    v.get("connectionMode").and_then(Value::as_str),
                    Some("apple" | "usbdirect")
                )
        })
}

/// Short message above the dock for 3.5 s.
fn show_toast(ui: &AppWindow, text: String) {
    ui.set_toast("".into());
    // Messages from the service arrive in English.
    ui.set_toast(i18n::tr(&text).into());
    let w = ui.as_weak();
    slint::Timer::single_shot(Duration::from_millis(3500), move || {
        if let Some(ui) = w.upgrade() {
            ui.set_toast("".into());
        }
    });
}

/// "Check drivers" result, one line per device.
fn drivers_summary(d: &Value) -> String {
    let mut lines = Vec::new();
    for dev in d
        .get("devices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let fam = dev
            .get("family")
            .and_then(Value::as_str)
            .unwrap_or("iPhone");
        let mode = match dev.get("mode").and_then(Value::as_str).unwrap_or("") {
            "apple" => "Apple Driver",
            "replaced" => "Studio mode (libusb0)",
            "coexist" => "mixed, needs repair",
            _ => "unknown",
        };
        lines.push(format!("{fam}: {mode}"));
    }
    if lines.is_empty() {
        lines.push("No iPhone/iPad on USB".into());
    }
    lines.join("\n")
}

/// without Media Foundation (Windows N/KN without the Media Feature
/// Pack) there is no picture. Say so and open the official place to add it.
fn check_media_foundation(ui: &AppWindow) {
    let ok = mirror_video::media_foundation_present() && {
        // On its own thread: Media Foundation and COM, see hevc_ok.
        std::thread::spawn(|| mirror_video::decoder_available(&mirror_core::api::Codec::H264))
            .join()
            .unwrap_or(false)
    };
    if ok {
        return;
    }
    open_sheet(
        ui,
        "mfp",
        tr_ui(ui, "The Media Feature Pack is missing"),
        tr_ui(ui, "This Windows (an N or KN edition) has no Windows video components, so 3zT Mirroring cannot show the picture. Add the free \"Media Feature Pack\" from Microsoft: Settings › Apps › Optional features › View features › Media Feature Pack. Then restart the PC."),
        tr_ui(ui, "Open Optional features"),
        tr_ui(ui, "Later"),
    );
}

fn open_sheet(ui: &AppWindow, kind: &str, title: String, body: String, ok: String, cancel: String) {
    APP.with_borrow_mut(|a| a.sheet = Some(kind.into()));
    ui.set_sheet_title(title.into());
    ui.set_sheet_body(body.into());
    ui.set_sheet_ok(ok.into());
    ui.set_sheet_cancel(cancel.into());
    ui.set_sheet_open(true);
}

/// Ask before a driver change (it needs administrator approval).
fn confirm_driver(ui: &AppWindow, action: &str) {
    if matches!(
        ui.get_phase().as_str(),
        "connecting" | "activating" | "streaming"
    ) {
        show_toast(ui, tr_ui(ui, "Stop casting before changing drivers."));
        return;
    }
    if APP.with_borrow(|a| a.drv_instance.is_none()) {
        show_toast(ui, tr_ui(ui, "Connect the device first."));
        return;
    }
    let (title, body) = match action {
        "restore" => (
            tr_ui(ui, "Restore the Apple driver?"),
            tr_ui(ui, "Apple Driver mode can see the device again. Studio mode stops until its driver is installed again. A backup is saved first. Windows will ask for administrator permission. Unplug and reconnect the cable afterwards."),
        ),
        "bind" => (
            tr_ui(ui, "Install the Studio mode driver?"),
            tr_ui(ui, "Like TikTok LIVE Studio, 3zT Mirroring installs the libusb-win32 driver for this device. While it is installed, Apple mode and Apple apps can't see the device; press Restore Apple driver to switch back. If there is no driver package for this model yet, 3zT Mirroring builds one and signs it with a new certificate named \"3zT Mirroring Studio mode\", added to Windows' trusted certificates (its private key is deleted). A backup is saved first, and Apple's driver comes back automatically if the device doesn't start. Windows will ask for administrator permission. Unplug and reconnect the cable afterwards."),
        ),
        _ => (
            tr_ui(ui, "Remove the libusb0 filter?"),
            tr_ui(ui, "It is stacked on Apple's driver, which can crash Windows. Apple's driver stays. A backup is saved first. Windows will ask for administrator permission."),
        ),
    };
    open_sheet(
        ui,
        &format!("drv:{action}"),
        title,
        body,
        tr_ui(ui, "Continue"),
        tr_ui(ui, "Cancel"),
    );
}

/// Run a confirmed driver change, then offer to restart the service.
fn run_driver(ui: &AppWindow, api: &Api, action: String) {
    let Some(instance) = APP.with_borrow(|a| a.drv_instance.clone()) else {
        return;
    };
    open_sheet(
        ui,
        "busy",
        tr_ui(ui, "Changing the device's driver…"),
        tr_ui(ui, "Approve the Windows permission prompt. A PowerShell window shows each step; keep the cable plugged in until it finishes."),
        String::new(),
        String::new(),
    );
    let remember = ui.get_mode_remember();
    let (api, w) = (api.clone(), ui.as_weak());
    std::thread::spawn(move || {
        let r = api.post_slow(
            "/api/drivers/action",
            &json!({ "action": action, "instance": instance, "confirmed": true }),
            900,
        );
        let ok = r
            .as_ref()
            .and_then(|r| r.get("ok"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if ok && action != "removefilter" {
            // Keep the connection mode in step with the driver now installed.
            let mode = if action == "bind" {
                "usbdirect"
            } else {
                "apple"
            };
            api.post(
                "/api/connection",
                &json!({ "mode": mode, "remember": remember }),
            );
        }
        let msg = r
            .as_ref()
            .and_then(|r| str_of(r, &["message"]))
            .unwrap_or("")
            .to_string();
        let rolled = r
            .as_ref()
            .and_then(|r| r.get("rolledBack"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = w.upgrade() else { return };
            if ok {
                let target = if action == "bind" {
                    "Studio mode"
                } else {
                    "Apple Driver"
                };
                open_sheet(
                    &ui,
                    "restart",
                    tr_ui(&ui, "Driver changed"),
                    format!("{msg}

{}", tr_ui(&ui, &format!("The 3zT Mirroring service needs a restart to use {target} mode. Unplug the cable and plug it in again."))),
                    tr_ui(&ui, "Restart now"),
                    tr_ui(&ui, "Later"),
                );
            } else {
                let body = if msg.is_empty() {
                    tr_ui(&ui, "The backend did not answer.")
                } else {
                    msg.to_string()
                };
                let body = if rolled {
                    format!("{body} {}", tr_ui(&ui, "The previous state was restored."))
                } else {
                    body
                };
                open_sheet(
                    &ui,
                    "info",
                    tr_ui(&ui, "Driver not changed"),
                    body,
                    "OK".into(),
                    String::new(),
                );
            }
        });
    });
}

/// Move the window back inside the work area if a resize pushed it out.
fn keep_on_screen(ui: &AppWindow, (wx, wy, ww, wh): (f32, f32, f32, f32), size: LogicalSize) {
    let sf = ui.window().scale_factor();
    let pos = ui.window().position().to_logical(sf);
    let (w, h) = (size.width, size.height); // frameless: client = window
    let x = pos.x.min(wx + ww - w).max(wx);
    let y = pos.y.min(wy + wh - h).max(wy);
    if (x - pos.x).abs() > 0.5 || (y - pos.y).abs() > 0.5 {
        ui.window().set_position(slint::LogicalPosition::new(x, y));
    }
}

/// Icon font: "Segoe Fluent Icons" ships with Windows 11 only; Windows 10
/// has "Segoe MDL2 Assets" with the same code points for every icon 3zT Mirroring
/// uses (checked against segmdl2.ttf 1.86). Without this, Windows 10 drew
/// every icon as an empty box.
fn icon_font() -> &'static str {
    // Test hook: MIRROR_ICON_FONT=mdl2 shows the Windows 10 icons on Windows 11.
    if std::env::var("MIRROR_ICON_FONT").as_deref() == Ok("mdl2") {
        return pick_icon_font(false);
    }
    let fonts = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| r"C:\Windows".into())
        .join("Fonts");
    pick_icon_font(fonts.join("SegoeIcons.ttf").exists())
}

fn pick_icon_font(has_fluent: bool) -> &'static str {
    if has_fluent {
        "Segoe Fluent Icons"
    } else {
        "Segoe MDL2 Assets"
    }
}

/// Mirrors the startup navigation into the window.
fn apply_flow(ui: &AppWindow, f: &flow::Flow) {
    ui.set_screen(f.screen.as_str().into());
    ANDROID_POLL.store(
        match f.screen {
            flow::Screen::Platform => 1,
            flow::Screen::Android => 2,
            flow::Screen::Apple => 0,
        },
        Relaxed,
    );
    if f.apple_chooser {
        ui.set_chooser_cancelable(false);
        open_chooser(ui);
    } else if f.screen != flow::Screen::Apple {
        ui.set_chooser_open(false);
    }
    if f.screen == flow::Screen::Platform {
        widen_for_sheet(ui);
    }
}

/// What the Android poll does: 0 = nothing (Apple workspace), 1 = count
/// devices (platform screen), 2 = list them with names (Android workspace).
static ANDROID_POLL: AtomicU8 = AtomicU8::new(0);

enum AdbView {
    /// No ADB server, and no `adb` program to start one with.
    NoAdb,
    /// No ADB server yet; 3zT Mirroring is starting it.
    Starting,
    /// No ADB server; starting it failed.
    NotRunning,
    /// Port 5037 answers, but not as an ADB server.
    Foreign,
    /// An ADB server runs; `other` = a different version than 3zT Mirroring's.
    Running { version: u32, other: bool },
}

/// Polls the ADB server while the platform screen or the Android workspace
/// shows. When none runs, it starts one with the `adb` it finds (once; it
/// never kills or replaces a running server). Properties are read once per
/// transport and cached.
fn start_android_poll(ui: &AppWindow) {
    use mirror_android::adb::{check_server, AdbClient, ServerStatus, BUNDLED_SERVER_VERSION};
    use mirror_android::{devices, discovery};
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let client = AdbClient {
            timeout: Duration::from_millis(500),
            ..AdbClient::default()
        };
        let mut cache: std::collections::HashMap<
            (String, Option<u64>),
            mirror_android::props::Props,
        > = Default::default();
        let mut tried_start = false;
        loop {
            let mode = ANDROID_POLL.load(Relaxed);
            if mode == 0 {
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            let (view, list) = match check_server(&client, BUNDLED_SERVER_VERSION) {
                ServerStatus::NotRunning if !tried_start => {
                    tried_start = true;
                    match android::find_adb() {
                        None => (AdbView::NoAdb, Vec::new()),
                        Some(adb) => {
                            log::info!("android: starting the ADB server with {}", adb.display());
                            let port = client.addr.port();
                            let w = weak.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(ui) = w.upgrade() {
                                    show_android(&ui, &AdbView::Starting, &[], false);
                                }
                            });
                            match mirror_android::adb::start_server(&adb, port) {
                                Ok(s) if s.success() => {
                                    // Next round lists the devices.
                                    continue;
                                }
                                Ok(s) => {
                                    log::warn!("android: adb start-server exited with {s}");
                                    (AdbView::NotRunning, Vec::new())
                                }
                                Err(e) => {
                                    log::warn!("android: adb start-server: {e}");
                                    (AdbView::NotRunning, Vec::new())
                                }
                            }
                        }
                    }
                }
                ServerStatus::NotRunning => (
                    if android::find_adb().is_some() {
                        AdbView::NotRunning
                    } else {
                        AdbView::NoAdb
                    },
                    Vec::new(),
                ),
                ServerStatus::Foreign(_) => (AdbView::Foreign, Vec::new()),
                s => {
                    let (version, other) = match s {
                        ServerStatus::Mismatch { found, .. } => (found, true),
                        ServerStatus::Compatible(v) => (v, false),
                        _ => (0, true),
                    };
                    let entries = client
                        .devices_l()
                        .map(|t| devices::parse_list(&t))
                        .unwrap_or_default();
                    cache.retain(|k, _| {
                        entries
                            .iter()
                            .any(|e| (e.serial.clone(), e.transport_id) == *k)
                    });
                    let list = discovery::build(entries, |d| {
                        if mode < 2 {
                            return None;
                        }
                        let key = (d.serial.clone(), d.transport_id);
                        if let Some(p) = cache.get(&key) {
                            return Some(p.clone());
                        }
                        let target = match d.transport_id {
                            Some(id) => mirror_android::adb::Target::TransportId(id),
                            None => mirror_android::adb::Target::Serial(&d.serial),
                        };
                        let p = client.getprop(&target).ok()?;
                        cache.insert(key, p.clone());
                        Some(p)
                    });
                    (AdbView::Running { version, other }, list)
                }
            };
            // nothing in ADB, but Windows sees a phone (MTP/PTP).
            let phone_seen = list.is_empty()
                && matches!(view, AdbView::Running { .. })
                && usbphones::phone_without_adb();
            let w = weak.clone();
            if slint::invoke_from_event_loop(move || {
                if let Some(ui) = w.upgrade() {
                    show_android(&ui, &view, &list, phone_seen);
                }
            })
            .is_err()
            {
                return;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

fn show_android(
    ui: &AppWindow,
    view: &AdbView,
    list: &[mirror_android::discovery::AndroidDevice],
    phone_seen: bool,
) {
    use mirror_android::devices::Connection;
    use mirror_android::discovery::Readiness;
    ui.set_android_count(list.len() as i32);
    let status = match view {
        AdbView::NoAdb => tr_ui(ui, "ADB was not found. Install the Android SDK Platform-Tools (official, from Google) or set ANDROID_HOME, then open this screen again."),
        AdbView::Starting => tr_ui(ui, "Starting ADB…"),
        AdbView::NotRunning => tr_ui(ui, "ADB did not start. Close other apps that use ADB, then try again."),
        AdbView::Foreign => tr_ui(ui, "ADB port 5037 is used by another program. 3zT Mirroring does not change it."),
        AdbView::Running { version, other } => {
            let base = if list.is_empty() && phone_seen {
                tr_ui(ui, "Windows sees a phone on USB, but ADB does not yet. Turn on USB debugging (steps below), then unplug and plug the cable in again.")
            } else if list.is_empty() {
                tr_ui(ui, "No Android device yet. Follow the steps below.")
            } else {
                tr_ui(ui, &format!("{} device(s) seen through ADB.", list.len()))
            };
            if *other {
                format!(
                    "{base} {}",
                    tr_ui(ui, &format!("Another app's ADB server (version {version}) is running; 3zT Mirroring does not stop it."))
                )
            } else {
                base
            }
        }
    };
    ui.set_android_status(status.into());
    let rows: Vec<AndroidRow> = list
        .iter()
        .map(|d| {
            let link = match d.primary.connection {
                Connection::Usb => "USB",
                Connection::WifiTcp { .. } | Connection::WifiTls { .. } => "Wi-Fi",
                Connection::Emulator => tr_ui_static(ui, "Emulator"),
                Connection::Unknown => "?",
            };
            let state = match &d.readiness {
                Readiness::Ready => tr_ui(ui, "Ready"),
                Readiness::NeedsAuthorization => tr_ui(ui, "Tap Allow on the phone"),
                Readiness::Offline => tr_ui(ui, "Offline, reconnect the cable"),
                Readiness::NoPermission => tr_ui(ui, "No USB permission or driver"),
                Readiness::NotAndroidSystem => tr_ui(ui, "Recovery or bootloader mode"),
                Readiness::TooOld { sdk } => {
                    tr_ui(ui, &format!("Android too old (API {sdk}, needs 21+)"))
                }
                Readiness::Unknown => tr_ui(ui, "Reading device info…"),
            };
            let both_links = d.transports.iter().any(|t| t.connection == Connection::Usb)
                && d.transports.iter().any(|t| t.connection.is_wireless());
            let mut sub = vec![if both_links { "USB + Wi-Fi" } else { link }.to_string()];
            if let Some(r) = &d.android_release {
                sub.push(format!("Android {r}"));
            }
            sub.push(state);
            let on_wifi = d.transports.iter().any(|t| {
                matches!(
                    t.connection,
                    Connection::WifiTcp { .. } | Connection::WifiTls { .. }
                )
            });
            let on_usb = d.transports.iter().any(|t| t.connection == Connection::Usb);
            AndroidRow {
                title: d.name.display.clone().into(),
                sub: sub.join(" · ").into(),
                ok: d.readiness == Readiness::Ready,
                both: d.readiness == Readiness::Ready && on_usb && on_wifi,
                wifi: d.readiness == Readiness::Ready
                    && d.primary.connection == Connection::Usb
                    && !on_wifi,
            }
        })
        .collect();
    ui.set_android_devices(std::rc::Rc::new(slint::VecModel::from(rows)).into());
    APP.with_borrow_mut(|a| a.android_list = list.to_vec());
    // Test hook: show the QR pairing card (no phone needed to see it).
    static QR_TEST: AtomicBool = AtomicBool::new(false);
    if std::env::var_os("MIRROR_QR_TEST").is_some() && !QR_TEST.swap(true, Relaxed) {
        android_wifi_qr(ui);
    }
    // Test hook: play a recorded scrcpy stream as soon as the workspace shows.
    let fixture = std::env::var_os("MIRROR_ANDROID_FIXTURE").is_some();
    if fixture && !APP.with_borrow(|a| a.fixture_started) {
        APP.with_borrow_mut(|a| a.fixture_started = true);
        android_start(ui, -1);
        // Test hook: a toast over the playing picture (the picture must not hide it).
        if let Some(t) = std::env::var_os("MIRROR_TOAST_TEST") {
            let (w, t) = (ui.as_weak(), t.to_string_lossy().into_owned());
            slint::Timer::single_shot(Duration::from_secs(4), move || {
                if let Some(ui) = w.upgrade() {
                    show_toast(&ui, t);
                }
            });
        }
    }
}

/// Whether H.265 can be decoded: Windows' own HEVC decoder (checked once,
/// Media Foundation lookup) or the built-in FFmpeg one next to the exe.
fn hevc_ok() -> bool {
    if mirror_video::builtin_hevc_available() {
        return true;
    }
    static OK: OnceLock<bool> = OnceLock::new();
    // On its own thread: Media Foundation initialises COM as multithreaded,
    // and the UI thread must stay single-threaded for winit (OleInitialize
    // fails with RPC_E_CHANGED_MODE otherwise and the window panics).
    *OK.get_or_init(|| {
        std::thread::spawn(|| mirror_video::decoder_available(&mirror_core::api::Codec::Hevc))
            .join()
            .unwrap_or(false)
    })
}

/// scrcpy server options from Settings › Android video and Settings › Audio.
/// H.265 falls back to H.264 when Windows has no HEVC decoder.
/// The codec and encoder come from the Smart Encoder plan (`android.rs`).
fn android_options(p: &Prefs, audio_supported: bool) -> mirror_android::mirror::VideoOptions {
    let num = |s: &str, d: u32| s.parse::<u32>().unwrap_or(d);
    mirror_android::mirror::VideoOptions {
        max_size: num(&p.android_max_size, 1920),
        bit_rate: num(&p.android_bitrate, 8).saturating_mul(1_000_000),
        max_fps: num(&p.android_fps, 60),
        codec: "h264",
        encoder: None,
        audio: audio_supported && p.audio_out.as_str() == "pc",
        control: true,
        new_display: None,
    }
}

/// What the user chose for codec and encoder.
fn android_pref(p: &Prefs) -> mirror_android::encoders::Preference {
    mirror_android::encoders::Preference {
        codec: p.android_codec.to_string(),
        encoder: p.android_encoder.to_string(),
    }
}

fn android_decoders() -> mirror_android::encoders::Decoders {
    mirror_android::encoders::Decoders {
        h265: hevc_ok(),
        av1: false,
    }
}

/// Runs `f` on the per-device store (loaded on first use) and saves it.
fn with_store<R>(f: impl FnOnce(&mut mirror_android::profiles::Store) -> R) -> R {
    APP.with_borrow_mut(|a| {
        let s = a.android_store.get_or_insert_with(devprofiles::load);
        let r = f(s);
        devprofiles::save(s);
        r
    })
}

/// Loads a device's own settings into Settings before it starts. A phone
/// seen for the first time takes the current Settings as its own.
fn android_select_device(ui: &AppWindow, d: &mirror_android::discovery::AndroidDevice) {
    use mirror_android::devices::Connection;
    let key = d.id.key.clone();
    let mut p = ui.get_prefs();
    let known = with_store(|s| {
        let known = s.get(&key).is_some();
        let e = s.entry(&key);
        e.name = d.name.display.clone();
        e.manufacturer = d.manufacturer.clone().unwrap_or_default();
        e.model = d.model.clone().unwrap_or_default();
        e.android = d.android_release.clone().unwrap_or_default();
        e.connection = match d.primary.connection {
            Connection::WifiTcp { .. } | Connection::WifiTls { .. } => "wifi",
            _ => "usb",
        }
        .into();
        if known {
            devprofiles::apply(&mut p, e);
        } else {
            e.set_picture(devprofiles::picture_of(&p));
        }
        known
    });
    if known {
        prefs::save(&p);
        ui.set_prefs(p);
    }
    APP.with_borrow_mut(|a| a.android_dev = Some(key));
    refresh_encoder_ui(ui);
}

/// Settings changed: store them for the selected device.
fn android_store_prefs(p: &Prefs) {
    let Some(key) = APP.with_borrow(|a| a.android_dev.clone()) else {
        return;
    };
    with_store(|s| {
        let e = s.entry(&key);
        e.set_picture(devprofiles::picture_of(p));
    });
}

/// Short button label for an encoder: the vendor part of its name and how
/// it runs, e.g. `qti hw`, `android sw`.
fn encoder_label(e: &mirror_android::encoders::Encoder) -> String {
    let parts: Vec<&str> = e.name.split('.').collect();
    let vendor = match parts.as_slice() {
        ["c2" | "OMX", v, ..] => v,
        [v, ..] => v,
        [] => "?",
    };
    let vendor: String = vendor.chars().take(10).collect();
    let accel = e.accel.as_str();
    if accel.is_empty() {
        vendor
    } else {
        format!("{vendor} {accel}")
    }
}

/// Fills the encoder choices and the detection summary in Settings.
fn refresh_encoder_ui(ui: &AppWindow) {
    use mirror_android::encoders::{usable_codecs, video_encoders, Accel};
    let list = APP.with_borrow(|a| {
        a.android_dev
            .as_ref()
            .and_then(|k| a.android_encoders.get(k).cloned())
    });
    let g = ui.global::<AndroidEnc>();
    let Some(list) = list else {
        g.set_options(slint::ModelRc::new(slint::VecModel::from(vec!["".into()])));
        g.set_labels(slint::ModelRc::new(slint::VecModel::from(vec![
            slint::SharedString::from(i18n::tr("Auto")),
        ])));
        g.set_codecs(slint::ModelRc::new(slint::VecModel::from(Vec::<
            slint::SharedString,
        >::new())));
        g.set_detected(slint::SharedString::new());
        return;
    };
    let p = ui.get_prefs();
    let codec = match p.android_codec.as_str() {
        "h265" => "h265",
        _ => "h264",
    };
    let encs = video_encoders(&list, codec);
    let mut options: Vec<slint::SharedString> = vec!["".into()];
    let mut labels: Vec<slint::SharedString> = vec![i18n::tr("Auto").into()];
    // Seg shows at most six options.
    for e in encs.iter().take(5) {
        options.push(e.name.as_str().into());
        labels.push(encoder_label(e).into());
    }
    g.set_options(slint::ModelRc::new(slint::VecModel::from(options)));
    g.set_labels(slint::ModelRc::new(slint::VecModel::from(labels)));
    let codecs: Vec<slint::SharedString> = usable_codecs(&list, android_decoders())
        .into_iter()
        .map(Into::into)
        .collect();
    g.set_codecs(slint::ModelRc::new(slint::VecModel::from(codecs)));
    // "H.264: 2 (1 hw) · H.265: 1 (1 hw) · AV1: 1 (no PC decoder)"
    let mut parts = Vec::new();
    for (c, name) in [("h264", "H.264"), ("h265", "H.265"), ("av1", "AV1")] {
        let v = video_encoders(&list, c);
        if v.is_empty() {
            continue;
        }
        let hw = v.iter().filter(|e| e.accel == Accel::Hardware).count();
        let mut s = format!("{name}: {} ({hw} hw)", v.len());
        if c == "av1" || (c == "h265" && !hevc_ok()) {
            s.push_str(&tr_ui(ui, ", no PC decoder"));
        }
        parts.push(s);
    }
    g.set_detected(parts.join(" · ").into());
}

/// Alt+C: reads the text on the picture with Windows OCR (through the
/// backend's /api/ocr, as in the classic window) and puts it on the PC
/// clipboard. Lines from earlier captures are stitched (Alt+R clears).
fn copy_screen_text(ui: &AppWindow) {
    static BUSY: AtomicBool = AtomicBool::new(false);
    let live = APP.with_borrow(|a| a.streaming || a.android_live);
    if !live {
        show_toast(ui, tr_ui(ui, "Start mirroring first."));
        return;
    }
    if BUSY.swap(true, Relaxed) {
        return;
    }
    let shot = APP.with_borrow(|a| a.view.as_ref()?.snapshot(Duration::from_millis(400)));
    let Some(shot) = shot else {
        BUSY.store(false, Relaxed);
        show_toast(ui, tr_ui(ui, "No picture to read yet."));
        return;
    };
    show_toast(ui, tr_ui(ui, "Reading the text on the screen…"));
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let image = format!(
            "data:image/bmp;base64,{}",
            shortcuts::base64(&shortcuts::bmp(shot.width, shot.height, &shot.bgra))
        );
        let r = Api::new().post_slow("/api/ocr", &json!({ "image": image }), 40);
        let _ = slint::invoke_from_event_loop(move || {
            BUSY.store(false, Relaxed);
            let Some(ui) = weak.upgrade() else { return };
            let ok = r
                .as_ref()
                .and_then(|v| v.get("ok"))
                .and_then(Value::as_bool)
                == Some(true);
            let text = r
                .as_ref()
                .and_then(|v| v.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let num = |k: &str| {
                r.as_ref()
                    .and_then(|v| v.get(k))
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            };
            let msg = if !ok {
                tr_ui(&ui, "Text recognition failed (Windows OCR).")
            } else if text.is_empty() {
                tr_ui(&ui, "No text found on the screen.")
            } else if !set_pc_clipboard_text(&text) {
                tr_ui(
                    &ui,
                    "Text read, but Windows didn't allow copying it to the clipboard.",
                )
            } else if num("added") == 0 {
                tr_ui(
                    &ui,
                    "Nothing new on this screen; the copied text is unchanged.",
                )
            } else {
                let (n, total) = (num("added"), num("total"));
                tr_ui(
                    &ui,
                    &format!(
                        "Copied to the PC: {n} new line(s) ({total} in total). Alt+R starts over."
                    ),
                )
            };
            show_toast(&ui, msg);
        });
    });
}

/// Puts `text` on the Windows clipboard. False when the clipboard is busy.
fn set_pc_clipboard_text(text: &str) -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        let ok = (|| {
            EmptyClipboard().ok()?;
            let g = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2).ok()?;
            let p = GlobalLock(g) as *mut u16;
            if p.is_null() {
                return None;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
            let _ = GlobalUnlock(g);
            // The clipboard owns the memory after this.
            SetClipboardData(CF_UNICODETEXT, Some(HANDLE(g.0))).ok()?;
            Some(())
        })()
        .is_some();
        let _ = CloseClipboard();
        ok
    }
}

/// Text on the Windows clipboard (Ctrl+V to Android), `None` without text.
fn pc_clipboard_text() -> Option<String> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        OpenClipboard(None).ok()?;
        let text = GetClipboardData(CF_UNICODETEXT).ok().and_then(|h| {
            let g = HGLOBAL(h.0);
            let p = GlobalLock(g) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            let _ = GlobalUnlock(g);
            Some(s)
        });
        let _ = CloseClipboard();
        text.filter(|s| !s.is_empty())
    }
}

/// The settings a running Android session depends on.
fn android_key(p: &Prefs) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        p.audio_out,
        p.android_max_size,
        p.android_fps,
        p.android_bitrate,
        p.android_codec,
        p.android_encoder
    )
}

/// Log of the app windows (`--android-app`), next to the main log.
const APP_LOG_FILE_NAME: &str = "mirroring-apps.log";

/// App picker rows for `filter` (name or package, any case), at most 80.
fn apps_matching<'a>(
    all: &'a [mirror_android::apps::DeviceApp],
    filter: &str,
) -> Vec<&'a mirror_android::apps::DeviceApp> {
    let f = filter.trim().to_lowercase();
    all.iter()
        .filter(|a| f.is_empty() || a.name.to_lowercase().contains(&f) || a.package.contains(&f))
        .take(80)
        .collect()
}

/// Launcher tile text: the first letter of the first two words ("Google
/// Maps" → "GM", "Chrome" → "C").
fn app_letters(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect::<String>()
        .chars()
        .take(2)
        .collect()
}

/// Launcher tile colour: one of 12 hues, fixed per package.
fn app_tint(package: &str) -> slint::Color {
    const HUES: [u32; 12] = [
        0xef4444, 0xf97316, 0xf59e0b, 0x84cc16, 0x22c55e, 0x14b8a6, 0x06b6d4, 0x3b82f6, 0x6366f1,
        0x8b5cf6, 0xd946ef, 0xec4899,
    ];
    // FNV-1a: stable across runs (std's hasher is randomly seeded).
    let h = package.bytes().fold(0x811c_9dc5u32, |h, b| {
        (h ^ b as u32).wrapping_mul(0x0100_0193)
    });
    let c = HUES[(h % HUES.len() as u32) as usize];
    slint::Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Shows the picker's rows for `filter`.
fn apps_show(ui: &AppWindow, filter: &str) {
    let rows = APP.with_borrow_mut(|a| {
        let key = a.apps_device.as_ref().map(|d| d.id.key.clone())?;
        let all = a.apps_cache.get(&key)?;
        a.apps_shown = apps_matching(all, filter).into_iter().cloned().collect();
        Some((a.apps_shown.clone(), all.len()))
    });
    let Some((shown, total)) = rows else { return };
    let model: Vec<AppRow> = shown
        .iter()
        .map(|a| AppRow {
            letter: app_letters(&a.name).into(),
            tint: app_tint(&a.package),
            name: a.name.clone().into(),
            sub: if a.system {
                format!("{} · {}", a.package, tr_ui(ui, "system"))
            } else {
                a.package.clone()
            }
            .into(),
        })
        .collect();
    let status = if total == 0 {
        tr_ui(ui, "The phone listed no apps.")
    } else if shown.is_empty() {
        tr_ui(ui, "No app matches.")
    } else if shown.len() < total {
        tr_ui(
            ui,
            &format!("{} of {total} apps. Type to search.", shown.len()),
        )
    } else {
        format!("{total} apps.")
    };
    ui.set_android_apps_status(status.into());
    ui.set_android_apps(std::rc::Rc::new(slint::VecModel::from(model)).into());
}

/// Opens the app picker for list entry `index` and reads its apps (once per
/// device; about a second or two on the phone).
fn apps_open(ui: &AppWindow) {
    if ui.get_android_apps_open() {
        ui.set_android_apps_open(false);
        return;
    }
    // The phone being mirrored (set in android_start), on the link in use.
    let Some(d) = APP.with_borrow(|a| a.android.as_ref().and(a.apps_device.clone())) else {
        show_toast(ui, tr_ui(ui, "Start mirroring an Android phone first."));
        return;
    };
    ui.set_android_apps_window_ok(d.sdk.unwrap_or(0) >= mirror_android::apps::MIN_SDK_NEW_DISPLAY);
    ui.set_android_apps_title(tr_ui(ui, &format!("Apps on {}", d.name.display)).into());
    ui.set_android_apps_open(true);
    let cached = APP.with_borrow_mut(|a| {
        a.apps_device = Some(d.clone());
        a.apps_cache.contains_key(&d.id.key)
    });
    if cached {
        ui.set_android_apps_busy(false);
        apps_show(ui, "");
        return;
    }
    let server = match scrcpy_server() {
        Ok(s) => s,
        Err(e) => {
            ui.set_android_apps_status(e.into());
            return;
        }
    };
    ui.set_android_apps_busy(true);
    ui.set_android_apps(std::rc::Rc::new(slint::VecModel::<AppRow>::default()).into());
    ui.set_android_apps_status(tr_ui(ui, "Reading the phone's apps…").into());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let client = mirror_android::adb::AdbClient::default();
        let got = mirror_android::apps::query(&client, &d.target(), &server);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            ui.set_android_apps_busy(false);
            match got {
                Ok(list) => {
                    log::info!("android: {} apps on {}", list.len(), d.name.display);
                    APP.with_borrow_mut(|a| {
                        a.apps_cache.insert(
                            d.id.key.clone(),
                            mirror_android::apps::sorted_for_picker(list),
                        );
                    });
                    apps_show(&ui, "");
                }
                Err(e) => {
                    log::warn!("android: app list failed: {e}");
                    ui.set_android_apps_status(
                        tr_ui(&ui, &format!("Could not read the apps: {e}")).into(),
                    );
                }
            }
        });
    });
}

/// Opens picker row `index` in its own window.
fn apps_launch(ui: &AppWindow, index: i32, place: &str) {
    let Some((d, app)) = APP.with_borrow(|a| {
        Some((
            a.apps_device.clone()?,
            a.apps_shown.get(index.max(0) as usize)?.clone(),
        ))
    }) else {
        return;
    };
    ui.set_android_apps_open(false);
    if place != "window" {
        // On the phone's own screen, in the picture being mirrored.
        match android::active() {
            Some(sh) => {
                sh.send(&[mirror_android::control::Control::StartApp(
                    app.package.clone(),
                )]);
                log::info!("android: start {} on the phone", app.package);
                show_toast(ui, tr_ui(ui, &format!("Opening {}…", app.name)));
            }
            None => show_toast(ui, tr_ui(ui, "Phone control is not ready.")),
        }
        return;
    }
    let args = appwin::AppArgs {
        serial: d.primary.serial.clone(),
        transport_id: d.primary.transport_id,
        package: app.package.clone(),
        name: app.name.clone(),
        device: d.name.display.clone(),
    };
    match appwin::spawn(&args) {
        Ok(()) => show_toast(ui, tr_ui(ui, &format!("Opening {}…", app.name))),
        Err(e) => {
            log::warn!("android: app window failed: {e}");
            show_toast(ui, tr_ui(ui, &format!("The app window did not open: {e}")));
        }
    }
}

fn scrcpy_server() -> Result<Arc<Vec<u8>>, String> {
    static SERVER: OnceLock<Result<Arc<Vec<u8>>, String>> = OnceLock::new();
    SERVER
        .get_or_init(|| android::load_server().map(Arc::new))
        .clone()
}

/// Starts mirroring list entry `index` (-1 = the first ready device), or the
/// recording in MIRROR_ANDROID_FIXTURE.
/// Runs one Wi-Fi step on a worker thread (the ADB calls block for
/// seconds) and shows its message. `Ok` is a success message.
/// Shows the current step of a Wi-Fi job under the device list.
type Step<'a> = &'a dyn Fn(&'static str);

fn android_wifi_job(
    ui: &AppWindow,
    job: impl FnOnce(Step) -> Result<(&'static str, String), (&'static str, String)> + Send + 'static,
) {
    if ui.get_android_wifi_busy() {
        return;
    }
    ui.set_android_wifi_busy(true);
    ui.set_android_wifi_status("".into());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let w = weak.clone();
        let step = move |en: &'static str| {
            let w = w.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = w.upgrade() {
                    let s = tr_ui(&ui, en);
                    ui.set_android_wifi_status(s.into());
                }
            });
        };
        let r = job(&step);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            ui.set_android_wifi_busy(false);
            ui.set_android_wifi_status("".into());
            let ((en, detail), _ok) = match r {
                Ok(m) => (m, true),
                Err(m) => (m, false),
            };
            let tail = if detail.is_empty() {
                String::new()
            } else {
                format!(" ({detail})")
            };
            show_toast(&ui, format!("{}{tail}", tr_ui(&ui, en)));
        });
    });
}

/// "Wi-Fi" on a USB device: read its Wi-Fi address, switch adbd to TCP
/// (`adb tcpip 5555`) and `adb connect` to it. The settings follow the
/// device, because its identity is the hardware serial.
fn android_wifi_device(ui: &AppWindow, index: i32) {
    use mirror_android::adb::{AdbClient, Target};
    use mirror_android::wireless::{Reply, TCPIP_PORT};
    let dev = APP.with_borrow(|a| a.android_list.get(index.max(0) as usize).cloned());
    let Some(d) = dev else { return };
    android_wifi_job(ui, move |step| {
        let c = AdbClient::default();
        let serial = d.primary.serial.clone();
        step("1/3 · Reading the phone's Wi-Fi address…");
        let target = match d.primary.transport_id {
            Some(id) => Target::TransportId(id),
            None => Target::Serial(&serial),
        };
        let ip = match c.wifi_address(&target) {
            Ok(Some(ip)) => ip,
            Ok(None) => return Err(("The phone is not on Wi-Fi", String::new())),
            Err(e) => return Err(("Couldn't read the Wi-Fi address", e.to_string())),
        };
        step("2/3 · Turning on ADB over Wi-Fi on the phone (its screen may flicker)…");
        if let Err(e) = c.tcpip(&target, TCPIP_PORT) {
            return Err(("Couldn't turn on TCP/IP mode", e.to_string()));
        }
        // adbd restarts on the phone; give it a moment, then retry.
        step("3/3 · Connecting over Wi-Fi…");
        let mut last = String::new();
        for _ in 0..4 {
            std::thread::sleep(Duration::from_millis(1200));
            match c.connect_device(&ip, TCPIP_PORT) {
                Ok(Reply::Ok(_)) => {
                    log::info!("android: Wi-Fi connected to {ip}:{TCPIP_PORT}");
                    return Ok((
                        "Connected over Wi-Fi. You can unplug the cable",
                        format!("{ip}:{TCPIP_PORT}"),
                    ));
                }
                Ok(Reply::Failed(m)) => last = m,
                Err(e) => last = e.to_string(),
            }
        }
        log::warn!("android: Wi-Fi connect to {ip} failed: {last}");
        Err((
            "Couldn't connect over Wi-Fi. The PC and the phone must be on the same network",
            last,
        ))
    });
}

/// "Connect": `adb connect IP:port`.
fn android_wifi_connect(ui: &AppWindow, addr: String) {
    use mirror_android::adb::AdbClient;
    use mirror_android::wireless::{parse_endpoint, Reply};
    let Some((host, port)) = parse_endpoint(&addr) else {
        show_toast(
            ui,
            tr_ui(ui, "Invalid address. Example: 192.168.1.20:37099"),
        );
        return;
    };
    android_wifi_job(ui, move |step| {
        step("Connecting to the phone over Wi-Fi…");
        match AdbClient::default().connect_device(&host, port) {
            Ok(Reply::Ok(_)) => Ok(("Connected over Wi-Fi", format!("{host}:{port}"))),
            Ok(Reply::Failed(m)) => Err(("Couldn't connect", m)),
            Err(e) => Err(("Couldn't connect", e.to_string())),
        }
    });
}

/// "Pair": `adb pair IP:port CODE` (Android 11+ wireless debugging).
fn android_wifi_pair(ui: &AppWindow, addr: String, code: String) {
    use mirror_android::adb::AdbClient;
    use mirror_android::wireless::{parse_endpoint, valid_pair_code, Reply};
    let Some((host, port)) = parse_endpoint(&addr) else {
        show_toast(
            ui,
            tr_ui(
                ui,
                "Enter the pairing address (IP:port) shown on the phone.",
            ),
        );
        return;
    };
    if !valid_pair_code(&code) {
        show_toast(ui, tr_ui(ui, "The pairing code has 6 digits."));
        return;
    }
    android_wifi_job(ui, move |step| {
        step("Pairing with the phone…");
        match AdbClient::default().pair(&host, port, &code) {
            Ok(Reply::Ok(_)) => Ok((
                "Paired. Now enter the address on the Wireless debugging screen and press Connect",
                String::new(),
            )),
            Ok(Reply::Failed(m)) => Err(("Pairing failed", m)),
            Err(e) => Err(("Pairing failed", e.to_string())),
        }
    });
}

/// Current QR pairing attempt; a new attempt or Cancel bumps it and the
/// old poll thread stops.
static QR_RUN: AtomicU64 = AtomicU64::new(0);

/// Black-on-white QR code with a 4-module quiet zone, 6 px per module.
fn qr_image(qr: &qrcodegen::QrCode) -> slint::Image {
    const SCALE: usize = 6;
    const QUIET: i32 = 4;
    let n = (qr.size() + QUIET * 2) as usize;
    let side = n * SCALE;
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(side as u32, side as u32);
    let px = buf.make_mut_slice();
    for (i, p) in px.iter_mut().enumerate() {
        let (x, y) = ((i % side) / SCALE, (i / side) / SCALE);
        let dark = qr.get_module(x as i32 - QUIET, y as i32 - QUIET);
        let v = if dark { 0 } else { 255 };
        *p = slint::Rgba8Pixel {
            r: v,
            g: v,
            b: v,
            a: 255,
        };
    }
    slint::Image::from_rgba8(buf)
}

/// "Pair with QR" (Android 11+): show `WIFI:T:ADB;S:<name>;P:<password>;;`,
/// wait until the phone that scanned it announces `_adb-tls-pairing` with
/// that name over mDNS, pair with the password, then connect to the
/// phone's `_adb-tls-connect` service. Gives up after two minutes.
fn android_wifi_qr(ui: &AppWindow) {
    use mirror_android::adb::AdbClient;
    use mirror_android::wireless::{find_connect, find_pairing, qr_payload, random_token, Reply};
    let run = QR_RUN.fetch_add(1, Relaxed) + 1;
    let name = format!("mirroring-{}", random_token(8));
    let password = random_token(10);
    let Ok(qr) =
        qrcodegen::QrCode::encode_text(&qr_payload(&name, &password), qrcodegen::QrCodeEcc::Medium)
    else {
        return;
    };
    ui.set_android_qr(qr_image(&qr));
    ui.set_android_qr_status(tr_ui(ui, "Waiting for the phone to scan…").into());
    ui.set_android_qr_on(true);
    let weak = ui.as_weak();
    // Status line (and a toast when the attempt ends) for this attempt only.
    let report = move |en: &'static str, detail: String, end: bool| {
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            if QR_RUN.load(Relaxed) != run {
                return;
            }
            let tail = if detail.is_empty() {
                String::new()
            } else {
                format!(" ({detail})")
            };
            let msg = format!("{}{tail}", tr_ui(&ui, en));
            if end {
                ui.set_android_qr_on(false);
                show_toast(&ui, msg);
            } else {
                ui.set_android_qr_status(msg.into());
            }
        });
    };
    std::thread::spawn(move || {
        let c = AdbClient::default();
        let live = || QR_RUN.load(Relaxed) == run;
        let t0 = Instant::now();
        let mut last_err = String::new();
        let found = loop {
            if !live() {
                return;
            }
            if t0.elapsed() > Duration::from_secs(120) {
                break None;
            }
            match c.mdns_services() {
                Ok(list) => {
                    if let Some(s) = find_pairing(&list, &name) {
                        break Some(s.clone());
                    }
                }
                Err(e) => last_err = e.to_string(),
            }
            std::thread::sleep(Duration::from_millis(800));
        };
        let Some(svc) = found else {
            report(
                "The QR code was not scanned within 2 minutes. Try again, or use a pairing code",
                last_err,
                true,
            );
            return;
        };
        report("Phone found, pairing…", String::new(), false);
        match c.pair(&svc.host, svc.port, &password) {
            Ok(Reply::Ok(_)) => {}
            Ok(Reply::Failed(m)) => return report("Pairing failed", m, true),
            Err(e) => return report("Pairing failed", e.to_string(), true),
        }
        report("Paired, connecting…", String::new(), false);
        // The connect service may be announced a moment after pairing.
        let t1 = Instant::now();
        while live() && t1.elapsed() < Duration::from_secs(30) {
            if let Ok(list) = c.mdns_services() {
                if let Some(s) = find_connect(&list, &svc.host) {
                    match c.connect_device(&s.host, s.port) {
                        Ok(Reply::Ok(_)) => {
                            return report(
                                "Connected over Wi-Fi",
                                format!("{}:{}", s.host, s.port),
                                true,
                            )
                        }
                        Ok(Reply::Failed(m)) => return report("Connect failed", m, true),
                        Err(e) => return report("Connect failed", e.to_string(), true),
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(800));
        }
        if live() {
            report(
                "Paired. Enter the address on the Wireless debugging screen and press Connect",
                String::new(),
                true,
            );
        }
    });
}

fn android_start(ui: &AppWindow, index: i32) {
    use mirror_android::devices::Connection;
    use mirror_android::discovery::Readiness;
    android_stop(ui);
    let shared = Arc::new(android::Shared::default());
    let (source, link): (Box<dyn mirror_core::source::VideoSource>, String) =
        if let Some(f) = std::env::var_os("MIRROR_ANDROID_FIXTURE") {
            (
                Box::new(android::FixtureSource::new(f.into(), shared.clone())),
                tr_ui(ui, "Test recording"),
            )
        } else {
            let dev = APP.with_borrow(|a| {
                let ready =
                    |d: &&mirror_android::discovery::AndroidDevice| d.readiness == Readiness::Ready;
                if index >= 0 {
                    a.android_list.get(index as usize).filter(ready).cloned()
                } else {
                    a.android_list.iter().find(ready).cloned()
                }
            });
            // A phone on both USB and Wi-Fi: use the link the user picked.
            let via = APP.with_borrow(|a| a.android_via);
            let dev = dev.map(|mut d| {
                if let Some(t) = via.and_then(|w| {
                    d.transports
                        .iter()
                        .find(|t| t.connection.is_wireless() == w)
                        .cloned()
                }) {
                    d.primary = t;
                }
                d
            });
            let Some(d) = dev else {
                show_toast(ui, tr_ui(ui, "No Android device is ready yet."));
                return;
            };
            let server = match scrcpy_server() {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("android: {e}");
                    ui.set_android_message(
                        tr_ui(
                            ui,
                            &format!(
                                "The scrcpy server file is missing ({e}). Reinstall 3zT Mirroring."
                            ),
                        )
                        .into(),
                    );
                    return;
                }
            };
            let link = match d.primary.connection {
                Connection::Usb => "USB",
                Connection::WifiTcp { .. } | Connection::WifiTls { .. } => "Wi-Fi",
                Connection::Emulator => "Emulator",
                Connection::Unknown => "",
            };
            android_select_device(ui, &d);
            APP.with_borrow_mut(|a| a.apps_device = Some(d.clone()));
            let known = APP.with_borrow(|a| a.android_encoders.get(&d.id.key).cloned());
            let p = ui.get_prefs();
            (
                Box::new(android::DeviceSource::new(
                    d.primary.serial.clone(),
                    d.primary.transport_id,
                    d.name.display.clone(),
                    server,
                    shared.clone(),
                    android_options(&p, d.audio_supported),
                    android::Smart {
                        pref: android_pref(&p),
                        dec: android_decoders(),
                        known,
                    },
                )),
                link.to_string(),
            )
        };
    ui.set_android_message("".into());
    APP.with_borrow_mut(|a| {
        android::set_active(Some(shared.clone()));
        a.android = Some(shared);
        a.android_index = index;
        a.android_good_saved = false;
        a.android_key = Some(android_key(&ui.get_prefs()));
        a.android_link = link;
        a.android_shown = None;
        a.fitted = None;
        match a.view.as_mut() {
            Some(v) => v.start_source(source),
            None => a.android_pending = Some(source),
        }
    });
    android_tick(ui);
}

/// Ends the Android session, if any.
fn android_stop(ui: &AppWindow) {
    ui.set_android_apps_open(false);
    let had = APP.with_borrow_mut(|a| {
        a.android_pending = None;
        let Some(s) = a.android.take() else {
            return false;
        };
        android::set_active(None);
        s.stop();
        if let Some(v) = a.view.as_mut() {
            v.stop();
        }
        a.android_live = false;
        a.android_shown = None;
        a.fitted = None;
        true
    });
    if had {
        ui.set_android_phase("idle".into());
    }
}

/// Mirrors the Android session's status into the window (every frame tick;
/// cheap when nothing changed).
fn android_tick(ui: &AppWindow) {
    let Some((st, link)) = APP.with_borrow(|a| {
        let st = a.android.as_ref()?.status();
        (a.android_shown.as_ref() != Some(&st)).then(|| (st, a.android_link.clone()))
    }) else {
        return;
    };
    let live = st.phase == android::Phase::Streaming;
    ui.set_android_phase(st.phase.as_str().into());
    ui.set_android_device(st.device.clone().into());
    ui.set_android_link(link.into());
    let message = match st.phase {
        android::Phase::Error => tr_ui(ui, &format!("Mirroring failed: {}. Reconnect the cable or tap Allow on the phone, then Start again.", st.error)),
        android::Phase::Connecting if !st.error.is_empty() => tr_ui(ui, &format!("Retrying… ({})", st.error)),
        _ => String::new(),
    };
    ui.set_android_message(message.into());
    let using = if st.encoder.is_empty() {
        st.codec.to_uppercase().replace("H26", "H.26")
    } else {
        format!(
            "{} · {}",
            st.codec.to_uppercase().replace("H26", "H.26"),
            st.encoder
        )
    };
    ui.global::<AndroidEnc>().set_using(if st.codec.is_empty() {
        "".into()
    } else {
        using.clone().into()
    });
    if live {
        android_session_ok(ui, &st, &using);
    }
    APP.with_borrow_mut(|a| {
        if live != a.android_live {
            a.android_live = live;
            a.fitted = None;
        }
        a.android_shown = Some(st);
    });
}

/// Performs a window shortcut.
fn run_shortcut(
    ui: &AppWindow,
    input: &Input,
    s: shortcuts::Shortcut,
    android_screen: bool,
    apple_live: bool,
) {
    use shortcuts::Shortcut;
    let need_apple = || {
        show_toast(
            ui,
            tr_ui(ui, "This shortcut needs Apple Driver mode while mirroring."),
        );
    };
    match s {
        Shortcut::Mute => {
            ui.invoke_action("mute".into());
            let msg = if ui.get_muted() {
                tr_ui(ui, "Sound on the PC: off (Alt+M)")
            } else {
                tr_ui(ui, "Sound on the PC: on (Alt+M)")
            };
            show_toast(ui, msg);
        }
        Shortcut::Settings => {
            // In the clean view Settings are out of sight: show the bars and
            // open them, instead of toggling a hidden panel.
            let open = ui.get_clean() || !ui.get_settings_open();
            ui.set_bars_hidden(false);
            ui.set_settings_open(open);
            ui.invoke_settings_toggled(open);
        }
        Shortcut::Rotate => {
            ui.invoke_action("rotate".into());
            let r = ui.get_prefs().rotation;
            let label = if r.as_str() == "auto" {
                "Auto".to_string()
            } else {
                format!("{r}°")
            };
            show_toast(ui, tr_ui(ui, &format!("Rotation: {label}")));
        }
        Shortcut::Back => ui.invoke_action(
            if android_screen {
                "back"
            } else {
                "swipe-right"
            }
            .into(),
        ),
        Shortcut::Lock => ui.invoke_action("lock".into()),
        Shortcut::CopyText => copy_screen_text(ui),
        Shortcut::ClearText => {
            std::thread::spawn(|| Api::new().post("/api/ocr/clear", &json!({})));
            show_toast(
                ui,
                tr_ui(ui, "Copy history cleared. The next Alt+C starts fresh."),
            );
        }
        Shortcut::Android(a) => ui.invoke_action(a.into()),
        Shortcut::Apple(_) if !apple_live => need_apple(),
        Shortcut::Apple("paste-pc") => match pc_clipboard_text() {
            Some(t) => {
                // Arabic and English alike, on the device's own layout.
                let t: String = t.chars().take(MAX_PASTE).collect();
                let layout = shortcuts::KbLayout::from_pref(ui.get_prefs().ios_kb_layout.as_str());
                let n = t.chars().filter(|c| *c != '\r').count();
                let (_, skipped) = type_on_apple(input, &t, layout);
                let msg = if skipped > 0 {
                    format!(
                        "Typing {} characters on the device ({skipped} have no key and were skipped).",
                        n - skipped
                    )
                } else {
                    format!("Typing {n} characters on the device.")
                };
                show_toast(ui, tr_ui(ui, &msg));
            }
            None => show_toast(ui, tr_ui(ui, "The PC clipboard has no text.")),
        },
        Shortcut::Apple("wake") => unlock_device(ui, input, false),
        Shortcut::Apple(a) => input.send(Cmd::Action(a)),
    }
}

/// Types `text` with the Apple device's hardware keyboard (Apple Driver /
/// Hybrid), switching the device keyboard language whenever the script
/// changes. `layout`: the device's Arabic hardware layout. Returns whether
/// anything was typed and how many characters have no key.
fn type_on_apple(input: &Input, text: &str, layout: shortcuts::KbLayout) -> (bool, usize) {
    let mut sent = false;
    let mut skipped = 0;
    for c in text.chars() {
        let c = if c == '\r' { continue } else { c };
        let arabic_now = IOS_ARABIC.load(Relaxed);
        let Some((script, mut usage, mut shifted)) = shortcuts::device_key(c, arabic_now, layout)
        else {
            skipped += 1;
            continue;
        };
        // Switch the device keyboard when the PC starts typing in the other
        // script; the key may then sit elsewhere.
        let want = match script {
            shortcuts::Script::Arabic => Some(true),
            shortcuts::Script::English => Some(false),
            shortcuts::Script::Either => None,
        };
        if let Some(want) = want.filter(|w| *w != arabic_now) {
            IOS_ARABIC.store(want, Relaxed);
            input.send(Cmd::SwitchLanguage);
            if let Some((_, u, sh)) = shortcuts::device_key(c, want, layout) {
                (usage, shifted) = (u, sh);
            }
        }
        // TEMPORARY (2.1 keyboard diagnosis, removed before the release):
        // only with MIRROR_KEYLOG set on a test PC.
        if std::env::var_os("MIRROR_KEYLOG").is_some() {
            log::info!(
                "keylog: got U+{:04X} usage 0x{usage:02x} shift {shifted} arabic_before {arabic_now} arabic_now {} layout {layout:?}",
                c as u32,
                IOS_ARABIC.load(Relaxed)
            );
        }
        let mut usages = Vec::with_capacity(2);
        if shifted {
            usages.push(shortcuts::HID_SHIFT);
        }
        usages.push(usage);
        input.send(Cmd::Keys(usages));
        sent = true;
    }
    (sent, skipped)
}

/// Unlock (Alt+U, or `auto` when mirroring starts): the backend only types
/// the passcode while the device is locked, then checks that it unlocked;
/// the toast says what really happened.
fn unlock_device(ui: &AppWindow, input: &Input, auto: bool) {
    device_keyboard_english(input);
    let (tx, rx) = std::sync::mpsc::channel();
    input.send(Cmd::Unlock(if auto { "auto-unlock" } else { "wake" }, tx));
    show_toast(ui, tr_ui(ui, "Unlocking the device…"));
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let reply = rx.recv().ok().flatten();
        let message = reply
            .as_ref()
            .and_then(|v| v.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            let text = match message.as_str() {
                "unlocked" => "Unlocked.",
                "already-unlocked" if auto => return,
                "already-unlocked" => "The device is already unlocked.",
                "woken" => "No passcode is saved, so the device was only woken.",
                "unverified" => "Passcode typed. The lock state can't be read on this connection, so check the device.",
                "unknown-state" => "Couldn't tell whether the device is locked, so the passcode wasn't typed. Press Alt+U to unlock.",
                "failed" => "Couldn't unlock. Enter the passcode on the device.",
                _ => "The device didn't take the unlock. Enter the passcode on the device.",
            };
            show_toast(&ui, tr_ui(&ui, text));
        });
    });
}

/// Alt+G and the AR / EN mark: the device typed the other language than
/// 3zT Mirroring expected, so switch it (and what 3zT Mirroring expects).
fn switch_device_language(input: &Input) {
    IOS_ARABIC.fetch_xor(true, Relaxed);
    input.send(Cmd::SwitchLanguage);
}

/// Letters only the right layout types right: و ز ظ were the keys reported
/// as punctuation with the wrong one.
const KB_TEST_TEXT: &str = "وزظ";

enum KbTest {
    /// Settings › Test the keyboard: ask for a text field first.
    Start,
    /// The user has a text field open: type the letters, ask what came out.
    Type,
    /// The answer: did the letters come out right?
    Answer(bool),
}

/// Settings › Control › Test the keyboard. Types و ز ظ with the chosen
/// Arabic layout and asks one question; a "no" switches to the other layout
/// (iOS has two Arabic hardware layouts, and nothing tells 3zT Mirroring
/// which one the device uses).
fn keyboard_test(ui: &AppWindow, input: &Input, step: KbTest) {
    let apple_live = ui.get_screen().as_str() != "android" && APP.with_borrow(|a| a.streaming);
    if !apple_live || !ui.get_prefs().keyboard {
        show_toast(
            ui,
            tr_ui(
                ui,
                "Start mirroring in Apple Driver mode with \"PC keyboard to the device\" on, then test.",
            ),
        );
        return;
    }
    let mut p = ui.get_prefs();
    let layout = shortcuts::KbLayout::from_pref(p.ios_kb_layout.as_str());
    match step {
        KbTest::Start => open_sheet(
            ui,
            "kbtest",
            tr_ui(ui, "Keyboard test"),
            tr_ui(
                ui,
                "Open an empty text field on the device (a new note, for example), then press Type. 3zT Mirroring types three Arabic letters there.",
            ),
            tr_ui(ui, "Type"),
            tr_ui(ui, "Cancel"),
        ),
        KbTest::Type => {
            type_on_apple(input, KB_TEST_TEXT, layout);
            open_sheet(
                ui,
                "kbcheck",
                tr_ui(ui, "Keyboard test"),
                tr_ui(ui, "Did these three letters appear on the device: و ز ظ ?"),
                tr_ui(ui, "Yes"),
                tr_ui(ui, "No"),
            );
        }
        KbTest::Answer(true) => show_toast(
            ui,
            tr_ui(ui, "The keyboard layout is right. You can delete the test letters."),
        ),
        KbTest::Answer(false) => {
            let other = layout.other();
            p.ios_kb_layout = other.pref().into();
            ui.set_prefs(p.clone());
            prefs::save(&p);
            let name = match other {
                shortcuts::KbLayout::Arabic => "Arabic",
                shortcuts::KbLayout::ArabicPc => "Arabic – PC",
            };
            show_toast(
                ui,
                tr_ui(
                    ui,
                    &format!(
                        "Switched to the \"{}\" layout. Delete the test letters and test again; if it is still wrong, press Alt+G once and test again.",
                        tr_ui(ui, name)
                    ),
                ),
            );
        }
    }
}

/// The device keyboard back to English (passcodes and pasted text are typed
/// with US keys).
fn device_keyboard_english(input: &Input) {
    if IOS_ARABIC.swap(false, Relaxed) {
        input.send(Cmd::SwitchLanguage);
    }
}

/// Before a session ends: leave the device keyboard in English, which is
/// what the next session assumes.
fn device_keyboard_reset_now(api: &Api) {
    if IOS_ARABIC.swap(false, Relaxed) && APP.with_borrow(|a| a.streaming) {
        for r in input::key_reports(&[shortcuts::HID_CTRL, 0x2c]) {
            api.key(&json!({ "usages": r }));
        }
    }
}

/// First frame of a session: remember the encoder list and the working
/// configuration, and say so when a fallback was needed.
fn android_session_ok(ui: &AppWindow, st: &android::Status, using: &str) {
    let Some((key, shared, saved)) = APP.with_borrow(|a| {
        Some((
            a.android_dev.clone()?,
            a.android.clone()?,
            a.android_good_saved,
        ))
    }) else {
        return;
    };
    if saved {
        return;
    }
    if let Some(list) = shared.encoders() {
        APP.with_borrow_mut(|a| a.android_encoders.insert(key.clone(), list));
    }
    let (w, h) = shared.frame_size();
    with_store(|s| {
        s.entry(&key).last_good = Some(mirror_android::profiles::LastGood {
            codec: st.codec.clone(),
            encoder: st.encoder.clone(),
            width: w,
            height: h,
        });
    });
    APP.with_borrow_mut(|a| a.android_good_saved = true);
    refresh_encoder_ui(ui);
    if st.fell_back {
        show_toast(
            ui,
            tr_ui(
                ui,
                &format!("The chosen encoder failed; now using {using}. Your choice is kept."),
            ),
        );
    }
}

fn tr_ui_static(_ui: &AppWindow, en: &'static str) -> &'static str {
    i18n::tr_static(en)
}

/// Tray menu words.
fn tray_labels(_p: &Prefs) -> (&'static str, &'static str) {
    (
        i18n::tr_static("Open 3zT Mirroring"),
        i18n::tr_static("Quit"),
    )
}

/// "Close to the tray": hide the window, keep the session (sound keeps
/// playing; Stop or Quit from the tray ends it).
fn hide_to_tray(ui: &AppWindow) {
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
    if let Some(h) = hwnd_of(ui.window()) {
        let _ = unsafe { ShowWindow(h, SW_HIDE) };
        tray::HIDDEN.store(true, Relaxed);
    }
}

/// Brings the window back (from the tray, minimised or behind others).
fn show_window(ui: &AppWindow) {
    use windows::Win32::UI::WindowsAndMessaging::{
        IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };
    if let Some(h) = hwnd_of(ui.window()) {
        unsafe {
            let _ = ShowWindow(
                h,
                if IsIconic(h).as_bool() {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
            let _ = SetForegroundWindow(h);
        }
        tray::HIDDEN.store(false, Relaxed);
    }
}

fn quit_app(ui: &AppWindow) {
    device_keyboard_reset_now(&Api::new());
    tray::set_icon(false, "", "");
    let _ = ui.window().hide();
    let _ = slint::quit_event_loop();
}

/// The window's close button and Alt+F4.
fn close_requested(ui: &AppWindow) {
    if ui.get_prefs().close_to_tray {
        hide_to_tray(ui);
    } else {
        quit_app(ui);
    }
}

// ---- Updates: see updater.rs and mirror_core::update. ----

thread_local! {
    /// The last offer from "Check for updates" (`None` = nothing to install).
    static UPDATE_OFFER: RefCell<Option<(mirror_core::update::Release, bool)>> = const { RefCell::new(None) };
}

fn update_channel(_p: &Prefs) -> mirror_core::update::Channel {
    mirror_core::update::Channel::of(&updater::current_version())
}

fn update_clear(ui: &AppWindow) {
    UPDATE_OFFER.with_borrow_mut(|o| *o = None);
    ui.set_upd_version("".into());
    ui.set_upd_notes("".into());
    ui.set_upd_switch(false);
    ui.set_upd_progress(-1.0);
}

/// Asks GitHub for the newest release on the chosen channel. `manual`: the
/// user pressed the button (a startup check only shows a toast when there
/// is something new).
fn update_check(ui: &AppWindow, manual: bool) {
    use mirror_core::update::Offer;
    if ui.get_upd_busy() {
        return;
    }
    let channel = update_channel(&ui.get_prefs());
    update_clear(ui);
    ui.set_upd_busy(true);
    ui.set_upd_status(tr_ui(ui, "Checking for updates…").into());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let r = updater::check(channel);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            ui.set_upd_busy(false);
            match r {
                Ok(Offer::UpToDate) => {
                    log::info!("update: up to date ({channel:?})");
                    ui.set_upd_status(tr_ui(&ui, "3zT Mirroring is up to date.").into());
                }
                Ok(offer) => {
                    let (rel, switch) = match offer {
                        Offer::Update(r) => (r, false),
                        Offer::Switch(r) => (r, true),
                        Offer::UpToDate => unreachable!(),
                    };
                    log::info!("update: offer {} (switch {switch})", rel.tag);
                    let v = rel.version.to_string();
                    let status = if switch {
                        tr_ui(&ui, &format!("Other edition: v{v}. Installs as a separate app; settings do not move over."))
                    } else {
                        tr_ui(&ui, &format!("New version available: v{v}"))
                    };
                    ui.set_upd_status(status.into());
                    ui.set_upd_notes(mirror_core::update::short_notes(&rel.notes, 360).into());
                    ui.set_upd_version(v.clone().into());
                    ui.set_upd_switch(switch);
                    if !manual && !switch {
                        let t = tr_ui(
                            &ui,
                            &format!(
                                "3zT Mirroring v{v} is available: press Update in the header."
                            ),
                        );
                        show_toast(&ui, t);
                    }
                    UPDATE_OFFER.with_borrow_mut(|o| *o = Some((rel, switch)));
                }
                Err(e) => {
                    log::warn!("update: check failed: {e}");
                    if manual {
                        ui.set_upd_status(tr_ui(&ui, &format!("Could not check: {e}")).into());
                    } else {
                        ui.set_upd_status("".into());
                    }
                }
            }
        });
    });
}

/// Longest PC clipboard text Alt+V types on an iPhone/iPad.
const MAX_PASTE: usize = 4000;

/// The iPhone/iPad hardware keyboard is set to Arabic (as far as 3zT Mirroring
/// switched it; Alt+G switches by hand when they disagree).
static IOS_ARABIC: AtomicBool = AtomicBool::new(false);

/// The "Buy me a coffee" page.
const SUPPORT_URL: &str = "https://7krh-hub.pages.dev/en/support/";

/// Opens an https page in the default browser.
fn open_url(url: &str) {
    if url.starts_with("https://") {
        let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
    }
}

/// Opens a page of this repository's releases in the default browser.
fn open_release_page(url: &str) {
    if url.starts_with("https://github.com/abogzah559-wq/3zT-Mirroring/") {
        let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
    }
}

/// "Update to vX": download, verify, start, quit (an update of this
/// edition) or keep running (the other edition installs next to it).
fn update_install(ui: &AppWindow) {
    let Some((rel, switch)) = UPDATE_OFFER.with_borrow(|o| o.clone()) else {
        return;
    };
    if ui.get_upd_busy() {
        return;
    }
    let kind = updater::kind();
    let Some(asset) = updater::asset_for(kind, &rel).cloned() else {
        // A source checkout (or a release without a verified file):
        // the page, where the user downloads it.
        open_release_page(&rel.page);
        return;
    };
    let sig = updater::signature_for(kind, &rel).cloned();
    let dest = match updater::download_path(kind, &asset) {
        Ok(d) => d,
        Err(e) => {
            ui.set_upd_status(format!("Failed: {e}").into());
            return;
        }
    };
    ui.set_upd_busy(true);
    ui.set_upd_progress(0.0);
    ui.set_upd_status(tr_ui(ui, &format!("Downloading {}…", asset.name)).into());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let w = weak.clone();
        let progress = move |done: u64, total: u64| {
            let w = w.clone();
            let f = if total > 0 {
                done as f32 / total as f32
            } else {
                0.0
            };
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = w.upgrade() {
                    ui.set_upd_progress(f);
                }
            });
        };
        let r = updater::download_verified(&asset, sig.as_ref(), &dest, &progress).and_then(|_| {
            log::info!(
                "update: {} verified (sha256 {}, signature ok)",
                dest.display(),
                asset.sha256
            );
            updater::launch(kind, &dest, !switch)
        });
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            ui.set_upd_busy(false);
            ui.set_upd_progress(-1.0);
            match r {
                Ok(()) if switch => {
                    ui.set_upd_status(tr_ui(&ui, "The other edition's installer is open.").into());
                }
                Ok(()) => {
                    log::info!("update: started {}, quitting", rel.tag);
                    quit_app(&ui);
                }
                Err(e) => {
                    log::warn!("update: {e}");
                    show_toast(&ui, format!("Update failed: {e}"));
                    ui.set_upd_status(tr_ui(&ui, &format!("Update failed: {e}")).into());
                }
            }
        });
    });
}

fn main() -> Result<(), slint::PlatformError> {
    // `3zTMirroring.exe --android-app …` is one Android app in its own
    // window (no backend, no tray, no single-instance check).
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some(appwin::ARG) {
        init_log_named(APP_LOG_FILE_NAME);
        let Some(a) = appwin::AppArgs::parse(&args) else {
            log::warn!("app window: bad arguments {args:?}");
            return Ok(());
        };
        return appwin::run(a);
    }
    // Tests run with their own prefs file (MIRROR_PREFS) and may run next to the
    // user's 3zT Mirroring; everyone else gets one 3zT Mirroring per user: a second start
    // shows the running one (also when it is hidden in the tray).
    let test_mode = std::env::var_os("MIRROR_PREFS").is_some();
    // A portable update: the old exe quits; wait for it before the
    // single-instance check, then delete it.
    updater::finish_portable_update();
    if !test_mode && tray::show_running_instance() {
        return Ok(());
    }
    let background = std::env::args().any(|a| a == tray::BACKGROUND_ARG);
    init_log();
    // A finished update leaves its Setup in %TEMP%: remove downloads older
    // than ten minutes (never one that is being downloaded or run now).
    std::thread::spawn(|| {
        let dir = std::env::temp_dir().join(format!("{}-update", mirror_core::DATA_DIR_NAME));
        let old = |e: &std::fs::DirEntry| {
            e.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(600))
        };
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if old(&e) && std::fs::remove_file(e.path()).is_ok() {
                log::info!("update: removed {}", e.path().display());
            }
        }
    });
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("femtovg".into())
        .with_winit_window_attributes_hook(|attrs| {
            attrs
                .with_transparent(true)
                .with_decorations(false)
                .with_system_backdrop(BackdropType::None)
        })
        .select()?;

    let ui = AppWindow::new()?;
    ui.global::<Theme>().set_icons(icon_font().into());
    ui.global::<AppInfo>()
        .set_version(env!("CARGO_PKG_VERSION").into());
    ui.set_prefs(prefs::load());
    i18n::apply(ui.get_prefs().language.as_str());
    ui.global::<Theme>().set_rtl(i18n::is_arabic());
    {
        let weak = ui.as_weak();
        tray::install(move |e| {
            let w = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = w.upgrade() else { return };
                match e {
                    tray::Event::Toggle if !tray::HIDDEN.load(Relaxed) => hide_to_tray(&ui),
                    tray::Event::Toggle | tray::Event::Show => show_window(&ui),
                    tray::Event::Quit => quit_app(&ui),
                }
            });
        });
        let p = ui.get_prefs();
        let (open, quit) = tray_labels(&p);
        tray::set_icon(p.close_to_tray, open, quit);
        if !test_mode {
            // Keeps the Run entry pointing at this exe (a moved Portable.exe).
            tray::set_start_with_windows(p.start_with_windows, false);
        }
        filedrop::install(&ui);
        let weak = ui.as_weak();
        ui.window().on_close_requested(move || {
            if let Some(ui) = weak.upgrade() {
                close_requested(&ui);
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
    }
    let api = Api::new();
    let input = Input::new(api.clone());
    let touch_on = Arc::new(AtomicBool::new(ui.get_prefs().touch));
    let gestures_on = Arc::new(AtomicBool::new(ui.get_prefs().gestures));
    ui.set_message(i18n::tr("Starting 3zT Mirroring…").into());
    // Test hook: every settings card starts with its explanation open.
    if std::env::var_os("MIRROR_INFO_OPEN").is_some() {
        ui.global::<UiTest>().set_info_open(true);
    }
    if std::env::var_os("MIRROR_OPEN_SETTINGS").is_some() {
        ui.set_settings_open(true);
    }

    // Backend (Python sidecar for now), then a state poll loop.
    {
        let weak = ui.as_weak();
        let api = api.clone();
        std::thread::spawn(move || {
            match Sidecar::ensure(&api, Duration::from_secs(40)) {
                Ok(s) => {
                    let _ = SIDECAR.set(Mutex::new(Some(s)));
                }
                Err(e) => {
                    let w = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = w.upgrade() {
                            ui.set_phase("error".into());
                            ui.set_message(format!("Backend: {e}").into());
                        }
                    });
                    return;
                }
            }
            loop {
                if let Some(st) = api.state() {
                    let w = weak.clone();
                    let a = api.clone();
                    if slint::invoke_from_event_loop(move || {
                        if let Some(ui) = w.upgrade() {
                            on_state(&ui, &a, &st);
                        }
                    })
                    .is_err()
                    {
                        return;
                    }
                }
                std::thread::sleep(Duration::from_millis(400));
            }
        });
    }

    // Callbacks.
    {
        let api = api.clone();
        let weak = ui.as_weak();
        ui.on_start_cast(move || {
            if let Some(ui) = weak.upgrade() {
                // The card opens at step 0 (detect device) at once.
                let airplay = ui.get_mode_link().as_str() == "wireless"
                    && ui.get_flow_kind().as_str() == "usbdirect";
                if !airplay {
                    ui.set_flow_failed(false);
                    ui.set_flow_step(0);
                    APP.with_borrow_mut(|a| a.flow_started = Some(Instant::now()));
                }
            }
            let a = api.clone();
            std::thread::spawn(move || a.start_cast());
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_flow_dismiss(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_flow_step(-1);
                ui.set_flow_failed(false);
            }
        });
    }
    {
        // Apple device list: pick the device Start uses, or forget a saved one.
        let weak = ui.as_weak();
        let api2 = api.clone();
        ui.on_apple_pick(move |id| {
            let Some(ui) = weak.upgrade() else { return };
            let offline = ui
                .get_apple_devices()
                .iter()
                .find(|d| d.id == id)
                .map(|d| (d.state == "offline", d.title.to_string()));
            if let Some((true, name)) = offline {
                let t = tr_ui(&ui, &format!("{name} is not connected. Plug in the cable (or join the same Wi-Fi), unlock it, then press Start."));
                show_toast(&ui, t);
            }
            let a = api2.clone();
            let id = id.to_string();
            std::thread::spawn(move || a.post("/api/device/select", &json!({ "id": id })));
        });
        let api2 = api.clone();
        ui.on_apple_forget(move |id| {
            let a = api2.clone();
            let id = id.to_string();
            std::thread::spawn(move || {
                a.post("/api/device/select", &json!({ "id": id, "forget": true }))
            });
        });
    }
    {
        // The ready view's button when the device needs something first.
        let weak = ui.as_weak();
        ui.on_need_fix(move || {
            let Some(ui) = weak.upgrade() else { return };
            match ui.get_device_need().as_str() {
                n @ ("restore" | "bind" | "removefilter") => confirm_driver(&ui, n),
                "mode" => {
                    ui.set_chooser_cancelable(true);
                    open_chooser(&ui);
                }
                _ => {
                    ui.set_settings_open(true);
                    ui.invoke_settings_toggled(true);
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        let api = api.clone();
        let input_sheet = input.clone();
        ui.on_sheet_answer(move |yes| {
            let Some(ui) = weak.upgrade() else { return };
            let kind = APP.with_borrow_mut(|a| a.sheet.take()).unwrap_or_default();
            if kind == "busy" {
                APP.with_borrow_mut(|a| a.sheet = Some(kind));
                return; // no buttons while the change runs
            }
            ui.set_sheet_open(false);
            match (kind.as_str(), yes) {
                ("kbtest", true) => return keyboard_test(&ui, &input_sheet, KbTest::Type),
                ("kbcheck", _) => return keyboard_test(&ui, &input_sheet, KbTest::Answer(yes)),
                _ => {}
            }
            if !yes {
                return;
            }
            if kind == "mfp" {
                // Windows' own Settings page (no download by 3zT Mirroring).
                let _ = std::process::Command::new("explorer.exe")
                    .arg("ms-settings:optionalfeatures")
                    .spawn();
            } else if let Some(action) = kind.strip_prefix("drv:") {
                run_driver(&ui, &api, action.to_string());
            } else if kind == "restart" {
                show_toast(&ui, tr_ui(&ui, "Restarting the 3zT Mirroring service…"));
                let api = api.clone();
                std::thread::spawn(move || api.post_slow("/api/app/restart", &json!({}), 20));
            }
        });
    }
    {
        let api = api.clone();
        ui.on_stop_cast(move || {
            let a = api.clone();
            let arabic = IOS_ARABIC.load(Relaxed) && APP.with_borrow(|x| x.streaming);
            IOS_ARABIC.store(false, Relaxed);
            std::thread::spawn(move || {
                if arabic {
                    for r in input::key_reports(&[shortcuts::HID_CTRL, 0x2c]) {
                        a.key(&json!({ "usages": r }));
                    }
                }
                a.stop_cast()
            });
        });
    }
    {
        let api = api.clone();
        let weak = ui.as_weak();
        ui.on_restart(move || {
            if let Some(ui) = weak.upgrade() {
                show_toast(&ui, tr_ui(&ui, "Restarting the connection…"));
            }
            let a = api.clone();
            std::thread::spawn(move || {
                a.stop_cast();
                // The backend ignores Start while the old session is still
                // closing (that can take a few seconds): wait until it is
                // gone ("capture" is null), then start again.
                let t0 = Instant::now();
                while t0.elapsed() < Duration::from_secs(15) {
                    match a.state() {
                        Some(st) if st.get("capture").is_none_or(Value::is_null) => break,
                        _ => std::thread::sleep(Duration::from_millis(250)),
                    }
                }
                std::thread::sleep(Duration::from_millis(300));
                a.start_cast();
            });
        });
    }
    {
        let weak = ui.as_weak();
        let api = api.clone();
        let (t, g) = (touch_on.clone(), gestures_on.clone());
        let last = std::cell::RefCell::new(ui.get_prefs());
        ui.on_prefs_changed(move |mut p| {
            // Video profile logic, then store for the device.
            devprofiles::normalize(&last.borrow(), &mut p);
            if p.language != last.borrow().language {
                i18n::apply(p.language.as_str());
                if let Some(ui) = weak.upgrade() {
                    ui.global::<Theme>().set_rtl(i18n::is_arabic());
                }
            }
            if p.start_with_windows != last.borrow().start_with_windows && !test_mode {
                tray::set_start_with_windows(p.start_with_windows, true);
            }
            let (open, quit) = tray_labels(&p);
            tray::set_icon(p.close_to_tray, open, quit);
            let android_changed = devprofiles::picture_of(&p)
                != devprofiles::picture_of(&last.borrow())
                || p.android_profile != last.borrow().android_profile;
            *last.borrow_mut() = p.clone();
            if let Some(ui) = weak.upgrade() {
                if ui.get_prefs() != p {
                    ui.set_prefs(p.clone());
                }
                if android_changed {
                    android_store_prefs(&p);
                    refresh_encoder_ui(&ui);
                }
            }
            prefs::save(&p);
            t.store(p.touch, Relaxed);
            g.store(p.gestures, Relaxed);
            if let Some(ui) = weak.upgrade() {
                apply_look(&ui);
                push_audio(&api, &p, ui.get_muted());
                // "Device only" sound and the Android picture options are
                // server start options: restart a running Android session.
                let key = android_key(&p);
                let restart = APP.with_borrow_mut(|a| {
                    let changed = a.android_key.as_deref() != Some(key.as_str());
                    if changed {
                        a.android_key = Some(key);
                    }
                    changed && a.android.is_some()
                });
                if restart {
                    let i = APP.with_borrow(|a| a.android_index);
                    show_toast(&ui, tr_ui(&ui, "Applying the Android settings…"));
                    android_start(&ui, i);
                }
                // Only picture settings refit the window. Any other setting
                // (volume, lip sync, theme...) used to shrink a window the
                // user had resized back to the default fit.
                let key = fit_prefs_key(&p);
                APP.with_borrow_mut(|a| {
                    if a.fit_prefs.as_deref() != Some(key.as_str()) {
                        a.fit_prefs = Some(key);
                        a.fitted = None;
                    }
                });
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_settings_toggled(move |open| {
            let Some(ui) = weak.upgrade() else { return };
            if open {
                // Settings live in the side panel: show the bars with it.
                ui.set_bars_hidden(false);
            }
            // The window grows/shrinks frame by frame with the panel slide
            // (see the frame timer), keeping the picture size steady.
            let sf = ui.window().scale_factor();
            let stage = ui.window().size().to_logical(sf).width - ui.get_chrome_w();
            // 320 ms open, 240 ms close.
            let dur = Duration::from_millis(if open { 320 } else { 240 });
            let to = if open { 1.0 } else { 0.0 };
            let from = ui.get_sp();
            APP.with_borrow_mut(|a| {
                a.slide = Some((from, to, Instant::now(), dur));
                a.hold_stage = Some(stage);
                if let Some((w, h, _, c)) = a.fitted {
                    a.fitted = Some((w, h, open, c));
                }
            });
        });
    }
    ui.set_sp(if ui.get_settings_open() { 1.0 } else { 0.0 });
    if ui.get_settings_open() {
        // Settings start open: the window is the usual idle stage plus the
        // panel, not the panel squeezed into the default window (the window
        // has no size yet here, so do not measure it).
        APP.with_borrow_mut(|a| a.hold_stage = Some(IDLE_STAGE_W));
    }
    // First screen: the platform selector, unless the user opted in to
    // remembering the platform. On iPhone / iPad the Apple chooser shows at
    // once unless a remembered mode exists (the backend can take a while).
    {
        let p = ui.get_prefs();
        android::set_audio(
            p.audio_out.as_str() == "pc",
            p.volume / 100.0,
            ui.get_muted(),
        );
        ui.set_hevc_ok(hevc_ok());
        check_media_foundation(&ui);
        let f = flow::Flow::launch(&p.platform, p.remember_platform, mode_remembered());
        apply_flow(&ui, &f);
        APP.with_borrow_mut(|a| a.flow = Some(f));
        // Widen once the window exists on screen.
        let w = ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(60), move || {
            if let Some(ui) = w.upgrade() {
                if ui.get_chooser_open() || ui.get_screen().as_str() == "platform" {
                    widen_for_sheet(&ui);
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_choose_platform(move |platform, remember| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(screen) = flow::parse_platform(platform.as_str()) else {
                return;
            };
            let mut p = ui.get_prefs();
            p.platform = platform.clone();
            p.remember_platform = remember;
            ui.set_prefs(p.clone());
            prefs::save(&p);
            // Before the backend answered, its settings file tells whether
            // the Apple chooser is needed.
            let needs = APP
                .with_borrow(|a| a.apple_needs)
                .unwrap_or_else(|| !mode_remembered());
            let f = APP.with_borrow_mut(|a| {
                let f = a
                    .flow
                    .get_or_insert_with(|| flow::Flow::launch("", false, true));
                f.choose(screen, needs);
                f.clone()
            });
            apply_flow(&ui, &f);
        });
    }
    {
        let weak = ui.as_weak();
        let api = api.clone();
        ui.on_change_platform(move || {
            let Some(ui) = weak.upgrade() else { return };
            // Leave safely: end a running Apple session first.
            if APP.with_borrow(|a| a.streaming)
                || matches!(
                    ui.get_phase().as_str(),
                    "connecting" | "activating" | "streaming"
                )
            {
                let a = api.clone();
                std::thread::spawn(move || a.stop_cast());
            }
            android_stop(&ui);
            let f = APP.with_borrow_mut(|a| {
                let f = a
                    .flow
                    .get_or_insert_with(|| flow::Flow::launch("", false, true));
                f.back();
                f.clone()
            });
            apply_flow(&ui, &f);
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_android_wifi_device(move |i| {
            if let Some(ui) = weak.upgrade() {
                android_wifi_device(&ui, i);
            }
        });
        let weak = ui.as_weak();
        ui.on_android_wifi_connect(move |addr| {
            if let Some(ui) = weak.upgrade() {
                android_wifi_connect(&ui, addr.to_string());
            }
        });
        let weak = ui.as_weak();
        ui.on_android_wifi_pair(move |addr, code| {
            if let Some(ui) = weak.upgrade() {
                android_wifi_pair(&ui, addr.to_string(), code.to_string());
            }
        });
        let weak = ui.as_weak();
        ui.on_android_wifi_qr(move || {
            if let Some(ui) = weak.upgrade() {
                android_wifi_qr(&ui);
            }
        });
        let weak = ui.as_weak();
        ui.on_android_wifi_qr_cancel(move || {
            QR_RUN.fetch_add(1, Relaxed);
            if let Some(ui) = weak.upgrade() {
                ui.set_android_qr_on(false);
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_android_start(move |i| {
            if let Some(ui) = weak.upgrade() {
                APP.with_borrow_mut(|a| a.android_via = None);
                android_start(&ui, i);
            }
        });
        let weak = ui.as_weak();
        ui.on_android_start_via(move |i, via| {
            if let Some(ui) = weak.upgrade() {
                APP.with_borrow_mut(|a| a.android_via = Some(via.as_str() == "wifi"));
                android_start(&ui, i);
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_android_stop(move || {
            if let Some(ui) = weak.upgrade() {
                android_stop(&ui);
            }
        });
    }
    {
        let weak = ui.as_weak();

        ui.on_android_apps_filter(move |s| {
            if let Some(ui) = weak.upgrade() {
                apps_show(&ui, s.as_str());
            }
        });
        let weak = ui.as_weak();
        ui.on_android_app_launch(move |i, place| {
            if let Some(ui) = weak.upgrade() {
                apps_launch(&ui, i, place.as_str());
            }
        });
        let weak = ui.as_weak();
        ui.on_android_apps_close(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_android_apps_open(false);
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_android_restart(move || {
            if let Some(ui) = weak.upgrade() {
                let i = APP.with_borrow(|a| a.android_index);
                show_toast(&ui, tr_ui(&ui, "Restarting the connection…"));
                android_start(&ui, i);
            }
        });
    }
    start_android_poll(&ui);
    // Test hook: MIRROR_DEMO_SETTINGS=1 opens/closes settings every 1.5 s so the
    // slide can be filmed frame by frame.
    let demo = slint::Timer::default();
    if std::env::var_os("MIRROR_DEMO_SETTINGS").is_some() {
        let weak = ui.as_weak();
        demo.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(1500),
            move || {
                if let Some(ui) = weak.upgrade() {
                    let open = !ui.get_settings_open();
                    ui.set_settings_open(open);
                    ui.invoke_settings_toggled(open);
                }
            },
        );
    }
    {
        let weak = ui.as_weak();
        let api = api.clone();
        let input = input.clone();
        ui.on_action(move |a| {
            let Some(ui) = weak.upgrade() else { return };
            if a.as_str() == "apps" {
                apps_open(&ui);
                return;
            }
            if ui.get_screen().as_str() == "android" {
                if let Some(sh) = android::active() {
                    if sh.action(a.as_str()) {
                        return;
                    }
                }
            }
            // Control Center / Notification Center: press again to close,
            // press the other one to switch (close first, then open).
            if let Some(name) = match a.as_str() {
                "control-center" => Some("control-center"),
                "notifications" => Some("notifications"),
                _ => None,
            } {
                let open = |input: &Input, n: &str| {
                    if n == "control-center" {
                        input.send(Cmd::Stroke(0.92, 0.0, 0.92, 0.62, 12, 16));
                    } else {
                        input.send(Cmd::Stroke(0.22, 0.0, 0.22, 0.62, 12, 16));
                    }
                };
                // Swipe up from the bottom closes either overlay.
                let close = |input: &Input| input.send(Cmd::Stroke(0.5, 0.97, 0.5, 0.25, 12, 16));
                let was = APP
                    .with_borrow(|x| x.panel)
                    .filter(|(_, at)| at.elapsed() < PANEL_TTL)
                    .map(|(n, _)| n);
                match was {
                    Some(p) if p == name => {
                        close(&input);
                        APP.with_borrow_mut(|x| x.panel = None);
                    }
                    Some(_) => {
                        close(&input);
                        APP.with_borrow_mut(|x| x.panel = Some((name, Instant::now())));
                        let input = input.clone();
                        slint::Timer::single_shot(Duration::from_millis(550), move || {
                            open(&input, name)
                        });
                    }
                    None => {
                        open(&input, name);
                        APP.with_borrow_mut(|x| x.panel = Some((name, Instant::now())));
                    }
                }
                return;
            }
            if matches!(
                a.as_str(),
                "swipe-left" | "swipe-right" | "home" | "switcher" | "lock"
            ) {
                APP.with_borrow_mut(|x| x.panel = None);
            }
            match a.as_str() {
                "swipe-left" => input.send(Cmd::Stroke(1.0, 0.5, 0.12, 0.5, 10, 20)),
                "swipe-right" => input.send(Cmd::Stroke(0.0, 0.5, 0.88, 0.5, 10, 20)),
                "home" => input.send(Cmd::Home(1)),
                "switcher" => input.send(Cmd::Home(2)),
                "control-center" => input.send(Cmd::Stroke(0.92, 0.0, 0.92, 0.62, 12, 16)),
                "notifications" => input.send(Cmd::Stroke(0.22, 0.0, 0.22, 0.62, 12, 16)),
                "lock" => input.send(Cmd::Action("lock")),
                "mute" => {
                    let m = !ui.get_muted();
                    ui.set_muted(m);
                    push_audio(&api, &ui.get_prefs(), m);
                }
                "rotate" => {
                    let mut p = ui.get_prefs();
                    p.rotation = match p.rotation.as_str() {
                        "auto" => "90",
                        "90" => "180",
                        "180" => "270",
                        "270" => "0",
                        _ => "auto",
                    }
                    .into();
                    ui.set_prefs(p.clone());
                    prefs::save(&p);
                    APP.with_borrow_mut(|a| a.fitted = None);
                }
                _ => {}
            }
        });
    }
    {
        let api = api.clone();
        let weak = ui.as_weak();
        ui.on_choose_mode(move |mode, link, remember| {
            APP.with_borrow_mut(|a| a.mode_pending = true);
            let (api, w) = (api.clone(), weak.clone());
            let (mode, link) = (mode.to_string(), link.to_string());
            std::thread::spawn(move || {
                // The backend may still be starting (the chooser shows at
                // launch): wait for it, up to a minute.
                let t0 = Instant::now();
                let mut cur = api.get("/api/connection");
                while cur.is_none() && t0.elapsed() < Duration::from_secs(60) {
                    std::thread::sleep(Duration::from_millis(500));
                    cur = api.get("/api/connection");
                }
                let ok_link = cur.is_some()
                    && (cur
                        .as_ref()
                        .and_then(|c| c.get("link"))
                        .and_then(Value::as_str)
                        == Some(link.as_str())
                        || api
                            .post("/api/connection", &json!({ "link": link }))
                            .is_some());
                // A refused choice answers HTTP 400 (post() gives None). The
                // reply is the whole app state, whose "error" field is
                // usually null: not a failure.
                let failed = !ok_link
                    || api
                        .post(
                            "/api/connection",
                            &json!({ "mode": mode, "remember": remember }),
                        )
                        .is_none();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = w.upgrade() else { return };
                    APP.with_borrow_mut(|a| {
                        a.mode_pending = false;
                        a.mode_sent = (!failed).then(Instant::now);
                    });
                    if failed {
                        show_toast(&ui, tr_ui(&ui, "The mode was not saved, try again."));
                        open_chooser(&ui);
                    }
                });
            });
        });
    }
    {
        // Cable / Wi-Fi from Settings (both modes). The backend ends a
        // running session first.
        let api = api.clone();
        let weak = ui.as_weak();
        ui.on_set_link(move |link| {
            let Some(ui) = weak.upgrade() else { return };
            if ui.get_mode_link() == link {
                return;
            }
            ui.set_mode_link(link.clone());
            show_toast(
                &ui,
                if link.as_str() == "wireless" {
                    tr_ui(&ui, "Switching to Wi-Fi…")
                } else {
                    tr_ui(&ui, "Switching to the cable…")
                },
            );
            let (api, w) = (api.clone(), weak.clone());
            let link = link.to_string();
            std::thread::spawn(move || {
                let ok = api
                    .post("/api/connection", &json!({ "link": link }))
                    .is_some();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = w.upgrade() {
                        if !ok {
                            show_toast(
                                &ui,
                                tr_ui(&ui, "The connection could not be changed, try again."),
                            );
                        }
                    }
                });
            });
        });
    }
    {
        let api = api.clone();
        let weak = ui.as_weak();
        ui.on_save_passcode(move |code| {
            if let Some(ui) = weak.upgrade() {
                passcode_query(&ui, &api, Some(code.to_string()));
            }
        });
    }
    {
        ui.set_upd_channel_own(
            match mirror_core::update::Channel::of(&updater::current_version()) {
                mirror_core::update::Channel::Beta => "beta",
                mirror_core::update::Channel::Stable => "stable",
            }
            .into(),
        );
        let weak = ui.as_weak();
        ui.on_update_action(move |a: slint::SharedString| {
            let Some(ui) = weak.upgrade() else { return };
            match a.as_str() {
                "check" => update_check(&ui, true),
                "install" => update_install(&ui),
                "notes" => {
                    let page =
                        UPDATE_OFFER.with_borrow(|o| o.as_ref().map(|(r, _)| r.page.clone()));
                    open_release_page(
                        &page.unwrap_or_else(|| mirror_core::update::RELEASES_PAGE.into()),
                    );
                }
                _ => {}
            }
        });
        // Startup check: a few seconds after start so it never slows the
        // first screen; never in tests (MIRROR_PREFS) unless a test feed is set.
        let feed_test = std::env::var_os("MIRROR_UPDATE_FEED").is_some();
        if ui.get_prefs().update_check && (!test_mode || feed_test) {
            let weak = ui.as_weak();
            slint::Timer::single_shot(
                Duration::from_secs(if feed_test { 1 } else { 8 }),
                move || {
                    if let Some(ui) = weak.upgrade() {
                        update_check(&ui, false);
                    }
                },
            );
        }
    }
    {
        let api = api.clone();
        let weak = ui.as_weak();
        let input_kb = input.clone();
        ui.on_driver_action(move |a: slint::SharedString| {
            if a.as_str() == "keyboard-test" {
                if let Some(ui) = weak.upgrade() {
                    keyboard_test(&ui, &input_kb, KbTest::Start);
                }
                return;
            }
            if matches!(a.as_str(), "restore" | "bind" | "removefilter") {
                if let Some(ui) = weak.upgrade() {
                    confirm_driver(&ui, a.as_str());
                }
                return;
            }
            if a.as_str() == "mode" {
                if let Some(ui) = weak.upgrade() {
                    ui.set_chooser_cancelable(true);
                    open_chooser(&ui);
                }
                return;
            }
            if a.as_str() == "platform" {
                if let Some(ui) = weak.upgrade() {
                    ui.invoke_change_platform();
                }
                return;
            }
            if a.as_str() == "logs" {
                // The log folder in Explorer, for bug reports.
                let dir = mirror_core::log_dir();
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::process::Command::new("explorer.exe").arg(&dir).spawn();
                return;
            }
            if let Some(ui) = weak.upgrade() {
                show_toast(
                    &ui,
                    if a.as_str() == "wifi" {
                        tr_ui(&ui, "Setting up Wi-Fi…")
                    } else {
                        tr_ui(&ui, "Checking drivers…")
                    },
                );
            }
            let (api, w) = (api.clone(), weak.clone());
            let a = a.to_string();
            std::thread::spawn(move || {
                let r = if a == "wifi" {
                    api.post_slow("/api/wifi/prepare", &json!({}), 120)
                } else {
                    api.get("/api/drivers")
                };
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = w.upgrade() else { return };
                    let Some(r) = r else {
                        show_toast(&ui, tr_ui(&ui, "The backend did not answer."));
                        return;
                    };
                    if a == "wifi" {
                        let ok = r.get("ok").and_then(Value::as_bool).unwrap_or(false);
                        let msg = r
                            .get("message")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| {
                                if ok {
                                    tr_ui(&ui, "Wi-Fi is ready. You can unplug the cable.")
                                } else {
                                    tr_ui(&ui, "Wi-Fi could not be set up.")
                                }
                            });
                        show_toast(&ui, msg);
                    } else {
                        let t = drivers_summary(&r);
                        APP.with_borrow_mut(|x| x.drivers_text = Some(t));
                        show_toast(&ui, tr_ui(&ui, "Drivers checked."));
                    }
                });
            });
        });
    }

    // Frameless window: move, resize, minimise, maximise, close.
    {
        let weak = ui.as_weak();
        let last_drag = std::cell::Cell::new(None::<Instant>);
        let input_win = input.clone();
        ui.on_window_action(move |a| {
            use slint::winit_030::winit::window::ResizeDirection as R;
            let Some(ui) = weak.upgrade() else { return };
            let w = ui.window();
            match a.as_str() {
                "close" => close_requested(&ui),
                // The AR / EN mark: same as Alt+G.
                "kb-lang" => switch_device_language(&input_win),
                "coffee" => open_url(SUPPORT_URL),
                // F2 showed or hid the header and dock: fit the window again
                // so they sit around the picture.
                "bars" => APP.with_borrow_mut(|a| a.fitted = None),
                "min" => w.set_minimized(true),
                "max" if is_full() => {}
                "max" => w.set_maximized(!w.is_maximized()),
                "fullscreen" => {
                    if !is_full() {
                        let was_max = w.is_maximized();
                        if was_max {
                            w.set_maximized(false);
                        }
                        if let Some((x, y, mw, mh)) = monitor_rect(&ui) {
                            let (pos, size) = (w.position(), w.size());
                            APP.with_borrow_mut(|a| a.full_restore = Some((pos, size, was_max)));
                            w.set_position(slint::PhysicalPosition::new(x, y));
                            // One pixel taller than the monitor: a GL window that
                            // exactly covers it is switched by the driver to an
                            // opaque full-screen mode that stayed after leaving
                            // (no glass, grey window, until a restart).
                            w.set_size(slint::PhysicalSize::new(mw, mh + 1));
                        }
                    } else if let Some((pos, size, was_max)) =
                        APP.with_borrow_mut(|a| a.full_restore.take())
                    {
                        w.set_position(pos);
                        w.set_size(size);
                        if was_max {
                            w.set_maximized(true);
                        }
                    }
                    ui.set_fullscreen(is_full());
                    // Leaving full screen: fit the window to the picture again.
                    // Corners and glass follow the new state: apply the theme
                    // again now and once the change has settled.
                    APP.with_borrow_mut(|a| {
                        a.fitted = None;
                        a.backdrop = None;
                    });
                    let w2 = ui.as_weak();
                    slint::Timer::single_shot(Duration::from_millis(350), move || {
                        if w2.upgrade().is_some() {
                            APP.with_borrow_mut(|a| a.backdrop = None);
                        }
                    });
                }
                "drag" if is_full() => {}
                "drag" => {
                    // Second press within 400 ms = double-click: maximise / restore.
                    let now = Instant::now();
                    if last_drag
                        .get()
                        .map(|t| now - t < Duration::from_millis(400))
                        .unwrap_or(false)
                    {
                        last_drag.set(None);
                        w.set_maximized(!w.is_maximized());
                    } else {
                        last_drag.set(Some(now));
                        w.with_winit_window(|ww| ww.drag_window());
                    }
                }
                r if r.starts_with("resize-") => {
                    let dir = match &r[7..] {
                        "n" => R::North,
                        "s" => R::South,
                        "w" => R::West,
                        "e" => R::East,
                        "nw" => R::NorthWest,
                        "ne" => R::NorthEast,
                        "sw" => R::SouthWest,
                        _ => R::SouthEast,
                    };
                    w.with_winit_window(|ww| ww.drag_resize_window(dir));
                }
                _ => {}
            }
        });
    }

    // PC keyboard to Android (Settings › PC keyboard).
    {
        let weak = ui.as_weak();
        let input_keys = input.clone();
        ui.on_key_input(move |text, ctrl, shift, alt| {
            let Some(ui) = weak.upgrade() else {
                return false;
            };
            let android_screen = ui.get_screen().as_str() == "android";
            let keyboard = ui.get_prefs().keyboard;
            let apple_live = !android_screen && APP.with_borrow(|a| a.streaming);
            let typing = keyboard
                && (if android_screen {
                    android::active().is_some()
                } else {
                    apple_live
                });
            // Shortcuts go by key, whatever layout Windows types with: on the
            // Arabic layout Ctrl+C arrives as U+0624, so map it back to "c".
            let route_text: String = if ctrl || alt {
                let mut cs = text.chars();
                match (cs.next(), cs.next()) {
                    (Some(c), None) => shortcuts::us_key(c)
                        .map(String::from)
                        .unwrap_or_else(|| text.to_string()),
                    _ => text.to_string(),
                }
            } else {
                text.to_string()
            };
            // Alt+G: switch the iPhone/iPad keyboard language by hand, for
            // when it no longer matches what the PC types.
            if alt && !ctrl && apple_live && keyboard && route_text.eq_ignore_ascii_case("g") {
                switch_device_language(&input_keys);
                return true;
            }
            if let Some(s) = shortcuts::route(android_screen, &route_text, ctrl, shift, alt, typing)
            {
                run_shortcut(&ui, &input_keys, s, android_screen, apple_live);
                return true;
            }
            if !keyboard {
                return false;
            }
            if !android_screen {
                // Apple Driver mode: the PC keyboard as a hardware keyboard.
                if !apple_live || ctrl || alt {
                    return false;
                }
                // Every character: the Arabic "لا" key types two.
                let layout = shortcuts::KbLayout::from_pref(ui.get_prefs().ios_kb_layout.as_str());
                let (sent, _) = type_on_apple(&input_keys, &text, layout);
                return sent;
            }
            let Some(sh) = android::active() else {
                return false;
            };
            match android::key_input(&route_text, ctrl, shift, alt) {
                android::KeyInput::Send(m) => sh.send(&m),
                android::KeyInput::Paste => {
                    if let Some(t) = pc_clipboard_text() {
                        sh.paste(t);
                    }
                }
                android::KeyInput::Ignore => return false,
            }
            true
        });
    }

    // Frame timer: native view placement, window fitting, stats.
    let timer = slint::Timer::default();
    {
        let weak = ui.as_weak();
        let input = input.clone();
        let (t, g) = (touch_on.clone(), gestures_on.clone());
        let clamped = std::cell::Cell::new(false);
        // Startup: the window is re-centred while its size still settles
        // (fit to the picture), until the user moves it or 3 s pass.
        let centring = std::cell::Cell::new(None::<(Instant, slint::PhysicalPosition)>);
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
            let Some(ui) = weak.upgrade() else { return };
            // The AR / EN mark: what 3zT Mirroring expects the device
            // keyboard to type (Apple Driver / Hybrid, PC keyboard on).
            let kb_lang = if ui.get_screen().as_str() != "android"
                && ui.get_prefs().keyboard
                && APP.with_borrow(|a| a.streaming)
                && ui.get_mode_active().as_str() != "usbdirect"
            {
                if IOS_ARABIC.load(Relaxed) {
                    "AR"
                } else {
                    "EN"
                }
            } else {
                ""
            };
            if ui.get_ios_kb_lang().as_str() != kb_lang {
                ui.set_ios_kb_lang(kb_lang.into());
            }
            // The first tick sizes the window for the screen; the settings
            // slide below waits a tick, else it would resize with the old
            // height (seen: the window stayed 900 px high).
            let first = !clamped.replace(true);
            if first {
                clamp_to_work_area(&ui);
                centring.set(Some((Instant::now(), ui.window().position())));
            } else if let Some((at, pos)) = centring.get() {
                let w = ui.window();
                let sliding = APP.with_borrow(|a| a.slide.is_some());
                if at.elapsed() > Duration::from_secs(3) || w.position() != pos {
                    centring.set(None); // settled, or the user moved it
                } else if !sliding {
                    if let Some(wa) = work_area(&ui) {
                        let size = w.size().to_logical(w.scale_factor());
                        let (x, y) = centered(wa, (size.width, size.height));
                        let want = slint::LogicalPosition::new(x, y).to_physical(w.scale_factor());
                        if (want.x - pos.x).abs() > 1 || (want.y - pos.y).abs() > 1 {
                            w.set_position(want);
                            centring.set(Some((at, want)));
                        }
                    }
                }
            }
            // Auto unlock, a moment after the picture started.
            let unlock = APP.with_borrow_mut(|a| {
                let due = a.unlock_at.is_some_and(|at| Instant::now() >= at);
                if due {
                    a.unlock_at = None;
                }
                due
            });
            if unlock && ui.get_phase().as_str() == "streaming" && ui.get_passcode_set() {
                unlock_device(&ui, &input, true);
            }
            APP.with_borrow_mut(|a| {
                if a.view.is_none() {
                    let Some(h) = hwnd_of(ui.window()) else { return };
                    a.backdrop = None;
                    match VideoView::new(h.0 as isize) {
                        Ok(v) => {
                            let input = input.clone();
                            let (t, g) = (t.clone(), g.clone());
                            let weak = ui.as_weak();
                            v.on_pointer(move |e| {
                                if matches!(e, PointerEvent::Down(..)) {
                                    // Keys go to the device (and the window's
                                    // shortcuts work) after a click on it.
                                    let weak = weak.clone();
                                    let _ = slint::invoke_from_event_loop(move || {
                                        // A click on the picture may close a
                                        // device overlay: forget it.
                                        APP.with_borrow_mut(|a| a.panel = None);
                                        if let Some(ui) = weak.upgrade() {
                                            ui.invoke_focus_keys();
                                        }
                                    });
                                }
                                // Android: the scrcpy control socket (touch,
                                // scroll, right click = BACK, middle = HOME).
                                if let Some(sh) = android::active() {
                                    if t.load(Relaxed) {
                                        sh.pointer(e, input.rotation.load(Relaxed));
                                    }
                                    return;
                                }
                                match e {
                                PointerEvent::Down(u, v) if t.load(Relaxed) => input.send(Cmd::Down(u, v)),
                                PointerEvent::Move(u, v) if t.load(Relaxed) => input.send(Cmd::Move(u, v)),
                                PointerEvent::Up(u, v) if t.load(Relaxed) => input.send(Cmd::Up(u, v)),
                                PointerEvent::Back if g.load(Relaxed) => input.send(Cmd::Stroke(0.0, 0.5, 0.65, 0.5, 14, 16)),
                                PointerEvent::Wheel(u, v, d) if g.load(Relaxed) => {
                                    let end = (v + 0.12 * d).clamp(0.02, 0.98);
                                    input.send(Cmd::Stroke(u, v, u, end, 5, 10));
                                }
                                _ => {}
                                }
                            });
                            a.view = Some(v);
                            if a.streaming {
                                a.view.as_mut().unwrap().start(Api::new());
                            } else if let Some(src) = a.android_pending.take() {
                                a.view.as_mut().unwrap().start_source(src);
                            }
                        }
                        Err(e) => {
                            ui.set_message(format!("Video: {e}").into());
                        }
                    }
                }
            });
            if APP.with_borrow(|a| a.backdrop.is_none()) {
                apply_look(&ui);
            }
            let p = ui.get_prefs();
            let sf = ui.window().scale_factor();
            let lw = ui.window().size().width as f32 / sf;
            if (lw < 640.0) != ui.get_compact() {
                ui.set_compact(lw < 640.0);
            }
            let (maxed, full) = (ui.window().is_maximized(), is_full());
            if maxed != ui.get_is_maximized() || full != ui.get_fullscreen() {
                // Window style changed: corners and glass go again.
                APP.with_borrow_mut(|a| a.backdrop = None);
            }
            ui.set_is_maximized(maxed);
            ui.set_fullscreen(full);
            // Settings slide: window width follows the animated chrome so
            // the picture keeps its size; ends when the slide settles.
            // MIRROR_SETTINGS_DEMO=1 (tests): open and close Settings every 1.5 s,
            // so the slide can be measured without touching the mouse.
            if std::env::var_os("MIRROR_SETTINGS_DEMO").is_some() {
                thread_local!(static DEMO: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) });
                let due = DEMO.with(|d| {
                    let at = d.get().unwrap_or_else(Instant::now);
                    let due = at.elapsed() >= Duration::from_millis(1500);
                    d.set(Some(if due { Instant::now() } else { at }));
                    due
                });
                if due {
                    let open = !ui.get_settings_open();
                    ui.set_settings_open(open);
                    ui.invoke_settings_toggled(open);
                }
            }
            if std::env::var_os("MIRROR_SETTINGS_DEMO").is_some() {
                log::info!(
                    "tick t={} window={lw:.1} stage_w={:.1} chrome={:.1} hold={:?}",
                    DEMO_T0.get_or_init(Instant::now).elapsed().as_millis(),
                    ui.get_stage_w(),
                    ui.get_chrome_w(),
                    APP.with_borrow(|a| a.hold_stage)
                );
            }
            // Settings slide: progress and window width change on the same
            // tick, so the layout never sees a panel wider than the window.
            if let Some((from, to, at, dur)) = APP.with_borrow(|a| a.slide) {
                let k = (at.elapsed().as_secs_f32() / dur.as_secs_f32()).min(1.0);
                ui.set_sp(from + (to - from) * slide_ease(k));
                if k >= 1.0 {
                    APP.with_borrow_mut(|a| a.slide = None);
                }
            }
            if let Some(stage) = APP.with_borrow(|a| a.hold_stage) {
                // Held before the window had a monitor: size it for this screen.
                let stage = if stage == IDLE_STAGE_W {
                    idle_stage_w(&ui)
                } else {
                    stage
                };
                if std::env::var_os("MIRROR_STATS_LOG").is_some() {
                    log::info!(
                        "slide window={lw:.1} stage_w={:.1} chrome={:.1} held={stage:.1}",
                        ui.get_stage_w(),
                        ui.get_chrome_w()
                    );
                }
                let chrome = ui.get_chrome_w();
                let target = (stage + chrome).max(min_window(&ui).0);
                let mut lh = ui.window().size().height as f32 / sf;
                if let Some((h, at)) = APP.with_borrow(|a| a.start_h) {
                    if at.elapsed() < Duration::from_secs(2) {
                        lh = lh.max(h);
                    }
                }
                if !first && (lw - target).abs() > 0.5 && !ui.window().is_maximized() && !is_full() {
                    let size = LogicalSize::new(target, lh);
                    ui.window().set_size(size);
                    if let Some(wa) = work_area(&ui) {
                        keep_on_screen(&ui, wa, size);
                    }
                }
                let end = if ui.get_settings_open() { 16.0 + 368.0 } else { 16.0 };
                if (chrome - end).abs() < 0.01 || ui.get_clean() {
                    APP.with_borrow_mut(|a| {
                        a.hold_stage = None;
                        a.fit_set = Some((target, lh));
                    });
                }
            }
            // Labelled dock is ~690 px wide (measured): icons only below that.
            let bar = ui.get_stage_w() < 720.0;
            if bar != ui.get_compact_bar() {
                ui.set_compact_bar(bar);
            }
            let on_apple = ui.get_screen().as_str() == "apple";
            let on_android = ui.get_screen().as_str() == "android";
            if on_android {
                android_tick(&ui);
            }
            APP.with_borrow_mut(|a| {
                let streaming = (a.streaming && on_apple) || (a.android_live && on_android);
                if ui.get_clean() && streaming {
                    if !a.clean_hint {
                        a.clean_hint = true;
                        let weak = ui.as_weak();
                        // After the window has settled around the picture.
                        slint::Timer::single_shot(Duration::from_millis(900), move || {
                            if let Some(ui) = weak.upgrade() {
                                show_toast(&ui, "F2 shows the controls · Alt + drag moves the window".into());
                            }
                        });
                    }
                } else if !streaming {
                    a.clean_hint = false;
                }
                // scrcpy sends the picture already turned; Apple reports the angle separately.
                let device_rotation = if on_android { 0 } else { a.device_rotation };
                let Some(v) = a.view.as_ref() else { return };
                v.set_crop(p.crop_l as u32, p.crop_t as u32, p.crop_r as u32, p.crop_b as u32);
                v.set_fit(match p.fit.as_str() {
                    "cover" => Fit::Cover,
                    "fill" => Fit::Fill,
                    _ => Fit::Contain,
                });
                // Rotation decision uses the unrotated frame size (read
                // directly: briefly setting rotation 0 let the render thread
                // draw a wrong frame).
                let raw = v.source_size();
                let rot = effective_rotation(&p, raw, device_rotation);
                v.set_rotation(rot);
                input.rotation.store(rot, Relaxed);
                v.set_background(0x000000);
                let (mut x, mut y, mut w, mut h) = (ui.get_stage_x() * sf, ui.get_stage_y() * sf, ui.get_stage_w() * sf, ui.get_stage_h() * sf);
                // Auto-Fit: shrink the native view to the picture itself, so the
                // glass stage shows around it instead of black letterbox bars.
                // Even inset on every side inside the glass stage.
                let inset = pic_inset(&ui) * sf;
                if p.fit != "fill" {
                    x += inset;
                    y += inset;
                    w = (w - 2.0 * inset).max(1.0);
                    h = (h - 2.0 * inset).max(1.0);
                }
                let pic = v.shown_size();
                if p.fit != "cover" && p.fit != "fill" && pic.0 > 0 && pic.1 > 0 && w > 0.0 && h > 0.0 {
                    let aspect = pic.0 as f32 / pic.1 as f32;
                    let (pw, ph) = if w / h > aspect { (h * aspect, h) } else { (w, w / aspect) };
                    x += (w - pw) / 2.0;
                    y += (h - ph) / 2.0;
                    (w, h) = (pw, ph);
                }
                // Toast and tooltip stay visible over the picture.
                let mut holes = Vec::new();
                let px = |v: f32| (v * sf).round() as i32;
                if ui.get_tip_on() {
                    holes.push((px(ui.get_tip_x()), px(ui.get_tip_y()), px(ui.get_tip_w()), px(ui.get_tip_h())));
                }
                if !ui.get_toast().is_empty() {
                    // 36 px high, rising 16 px into place above the dock.
                    let (ww, wh) = (ui.window().size().to_logical(sf).width, ui.window().size().to_logical(sf).height);
                    let tw = ui.get_toast_w();
                    holes.push((px((ww - tw) / 2.0), px(wh - 104.0), px(tw), px(52.0)));
                }
                if !ui.get_transfer().is_empty() {
                    let (ww, wh) = (ui.window().size().to_logical(sf).width, ui.window().size().to_logical(sf).height);
                    let tw = ui.get_transfer_w();
                    holes.push((px((ww - tw) / 2.0), px(wh - 150.0), px(tw), px(40.0)));
                }
                if (ui.get_android_apps_open() && on_android) || ui.get_drop_hover() || ui.get_files_open() {
                    holes.push((
                        px(ui.get_launcher_x()),
                        px(ui.get_launcher_y()),
                        px(ui.get_launcher_w()),
                        px(ui.get_launcher_h()),
                    ));
                }
                v.set_holes(&holes);
                v.set_bounds(x.round() as i32, y.round() as i32, w.round() as i32, h.round() as i32, streaming, if ui.get_clean() { 0 } else { ((22.0 - PIC_INSET) * sf).round() as i32 });
                let shown = v.shown_size();
                // The clean view waits for a real frame: until then the header
                // (Restart, Stop) stays.
                let framed = streaming && shown.0 > 0;
                if framed != ui.get_has_frame() {
                    ui.set_has_frame(framed);
                }
                if a.fit_prefs.is_none() {
                    a.fit_prefs = Some(fit_prefs_key(&p));
                }
                let free = a.full_restore.is_none() && !ui.window().is_maximized();
                if streaming && p.fit_window && shown.0 > 0 && free {
                    let key = (shown.0, shown.1, ui.get_settings_open(), ui.get_clean());
                    if a.fitted != Some(key) {
                        a.fitted = Some(key);
                        a.fit_set = fit_window(&ui, shown, a.user_scale);
                    } else if a.hold_stage.is_none() {
                        // Not a size 3zT Mirroring set: the user resized the window.
                        // Remember how big they want the picture.
                        let cur = ui.window().size().to_logical(sf);
                        let moved = a.fit_set.map(|(w, h)| (cur.width - w).abs() > 2.0 || (cur.height - h).abs() > 2.0).unwrap_or(false);
                        if moved {
                            if let Some((dw, dh)) = default_fit(&ui, shown) {
                                let cw = ui.get_chrome_w() + 2.0 * pic_inset(&ui);
                                let ch = ui.get_chrome_h() + 2.0 * pic_inset(&ui);
                                let k = ((cur.width - cw) / dw).min((cur.height - ch) / dh);
                                if k.is_finite() && k > 0.0 {
                                    a.user_scale = k.clamp(0.2, 4.0);
                                }
                            }
                            a.fit_set = Some((cur.width, cur.height));
                        }
                    }
                }
                // Dot field: ~30 fps, only while there is no picture.
                if !streaming && p.dots && !tray::HIDDEN.load(Relaxed) {
                    let fresh = a.dots_at.map(|t| t.elapsed() >= Duration::from_millis(33)).unwrap_or(true);
                    if fresh {
                        a.dots_at = Some(Instant::now());
                        let t = START.with(|s| s.elapsed().as_secs_f32());
                        let mouse = ui.get_stage_hover().then(|| (ui.get_stage_mouse_x(), ui.get_stage_mouse_y()));
                        let dark = a.dark;
                        let img = a.dots.get_or_insert_with(dots::Dots::new).render(ui.get_stage_w(), ui.get_stage_h(), sf, t, mouse, dark);
                        ui.set_dots_image(img);
                    }
                } else if a.dots.take().is_some() {
                    ui.set_dots_image(slint::Image::default());
                }
                let due = a.meter_at.map(|t| t.elapsed() >= Duration::from_secs(1)).unwrap_or(true);
                if due {
                    a.meter_at = Some(Instant::now());
                    let s = v.stats();
                    // Apple: the backend says "streaming" but no frame came.
                    // Say what to do instead of an empty window, and log it.
                    if streaming && s.width > 0 {
                        a.live_since = None;
                    } else if let Some(t) = a.live_since.filter(|t| streaming && !on_android && t.elapsed() > Duration::from_secs(10)) {
                        a.live_since = None;
                        log::warn!("apple: no picture {:.0} s after streaming started (decoded {:.1} fps, hardware {})", t.elapsed().as_secs_f32(), s.decoded_fps, s.hardware);
                        let msg = tr_ui_static(&ui, "No picture from the iPhone/iPad yet. Unlock the device and press Restart. If it stays empty, send the logs (Settings › About › Open log folder).");
                        ui.set_bars_hidden(false);
                        show_toast(&ui, msg.to_string());
                    }
                    if streaming && s.width > 0 && std::env::var_os("MIRROR_STATS_LOG").is_some() {
                        // Measuring frame pacing (tests): once a second, to the log.
                        log::info!("stats decoded={:.1} presented={:.1} dropped_total={}", s.decoded_fps, s.presented_fps, s.replaced);
                    }
                    if streaming && s.width > 0 {
                        // Android: sound latency (queue + output) next to the video numbers.
                        let sound = a
                            .android
                            .as_ref()
                            .filter(|_| on_android)
                            .and_then(|sh| sh.audio_ms())
                            .map(|ms| format!("  ·  {} {ms} ms", tr_ui_static(&ui, "sound")))
                            .unwrap_or_default();
                        ui.set_stats_line(format!("{}×{}  ·  {:.0} fps  ·  {:.1} Mbps  ·  decode {:.1} ms{sound}", s.width, s.height, s.presented_fps.min(s.decoded_fps), s.mbps, s.decode_ms).into());
                        ui.set_stats_full(
                            format!(
                                "Resolution    {}×{}\nDecode        {:.1} fps ({})\nPresented     {:.1} fps\nSkipped       {}\nDecode frame  {:.1} ms\nBitrate       {:.1} Mbps",
                                s.width,
                                s.height,
                                s.decoded_fps,
                                if s.hardware { "GPU" } else { "CPU" },
                                s.presented_fps,
                                s.replaced,
                                s.decode_ms,
                                s.mbps
                            )
                            .into(),
                        );
                    } else {
                        ui.set_stats_line("".into());
                        ui.set_stats_full("—".into());
                    }
                }
            });
        });
    }

    // Follow the Windows theme while "System" is selected.
    let theme_timer = slint::Timer::default();
    {
        let weak = ui.as_weak();
        theme_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_secs(2),
            move || {
                if let Some(ui) = weak.upgrade() {
                    if ui.get_prefs().theme.as_str() == "system"
                        && system_dark() != APP.with_borrow(|a| a.dark)
                    {
                        apply_look(&ui);
                    }
                }
            },
        );
    }

    apply_look(&ui);
    // "Start with Windows" (--background): into the tray when "Close to the
    // tray" is on, else minimised. The window exists once the loop runs.
    let start_timer = slint::Timer::default();
    if background {
        let weak = ui.as_weak();
        let tries = std::cell::Cell::new(0);
        start_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(15),
            move || {
                let Some(ui) = weak.upgrade() else { return };
                if tries.get() >= 1000 {
                    return; // done
                }
                tries.set(tries.get() + 1);
                if hwnd_of(ui.window()).is_none() && tries.get() < 200 {
                    return;
                }
                if ui.get_prefs().close_to_tray {
                    hide_to_tray(&ui);
                } else {
                    ui.window().set_minimized(true);
                }
                tries.set(1000);
            },
        );
    }
    // Until quit: the window may be hidden in the tray.
    let r = ui.show().and_then(|_| slint::run_event_loop_until_quit());
    drop(start_timer);
    tray::set_icon(false, "", "");
    let _ = ui.hide();
    APP.with_borrow_mut(|a| a.view = None);
    if let Some(m) = SIDECAR.get() {
        m.lock().unwrap().take();
    }
    r
}

#[cfg(test)]
mod launcher_tests {
    use super::*;

    #[test]
    fn tile_letters() {
        assert_eq!(app_letters("Google Maps"), "GM");
        assert_eq!(app_letters("Chrome"), "C");
        assert_eq!(app_letters("  (Beta) app store x"), "BA");
        assert_eq!(app_letters("ñandu"), "Ñ");
        assert_eq!(app_letters(""), "");
    }

    #[test]
    fn tile_colour_is_fixed_per_package() {
        assert_eq!(
            app_tint("com.android.chrome"),
            app_tint("com.android.chrome")
        );
        let hues: std::collections::HashSet<_> = ["a.b", "c.d", "e.f", "g.h", "i.j", "k.l"]
            .iter()
            .map(|p| app_tint(p).as_argb_encoded())
            .collect();
        assert!(hues.len() > 1);
    }
}

#[cfg(test)]
mod icon_tests {
    #[test]
    fn windows_10_falls_back_to_mdl2() {
        assert_eq!(super::pick_icon_font(false), "Segoe MDL2 Assets");
        assert_eq!(super::pick_icon_font(true), "Segoe Fluent Icons");
    }
}

#[cfg(test)]
mod window_tests {
    use super::centered;

    #[test]
    fn first_window_opens_in_the_middle_of_the_work_area() {
        // 2560 x 1392 work area (taskbar at the bottom), 1024 x 1183 window.
        assert_eq!(
            centered((0.0, 0.0, 2560.0, 1392.0), (1024.0, 1183.0)),
            (768.0, 105.0)
        );
        // Second monitor left of the primary one.
        assert_eq!(
            centered((-1920.0, 148.0, 1920.0, 1032.0), (1000.0, 900.0)),
            (-1460.0, 214.0)
        );
        // Never above or left of the work area.
        assert_eq!(
            centered((0.0, 0.0, 800.0, 600.0), (900.0, 700.0)),
            (0.0, 0.0)
        );
    }
}

#[cfg(test)]
mod slide_tests {
    use super::slide_ease;

    #[test]
    fn slide_curve_matches_the_css_bezier() {
        assert!(slide_ease(0.0).abs() < 1e-4);
        assert!((slide_ease(1.0) - 1.0).abs() < 1e-4);
        // Fast start, slow end (ease-out like the Slint curve it replaces).
        assert!(slide_ease(0.25) > 0.6, "{}", slide_ease(0.25));
        let mut last = 0.0;
        for i in 1..=100 {
            let y = slide_ease(i as f32 / 100.0);
            assert!(y >= last - 1e-5, "monotonic at {i}");
            last = y;
        }
    }
}
