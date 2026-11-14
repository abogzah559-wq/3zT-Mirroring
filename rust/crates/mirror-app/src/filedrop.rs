//! Files dropped on the window: while an Android phone is
//! mirrored, an `.apk` is installed and any other file goes to the phone's
//! Download folder (`mirror_android::files`). One file at a time, in a
//! background thread; the window shows a progress pill.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};

use mirror_android::adb::{AdbClient, Target};
use mirror_android::files::{self, Installed, Kind};
use slint::winit_030::winit::event::WindowEvent;
use slint::winit_030::{EventResult, WinitWindowAccessor};
use slint::ComponentHandle;

use crate::{show_toast, tr_ui, AppWindow, APP};

/// One dropped file and the phone it goes to.
struct Job {
    path: PathBuf,
    serial: String,
    transport_id: Option<u64>,
    device: String,
}

fn queue(ui: &AppWindow) -> &'static Mutex<Sender<Job>> {
    static Q: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    Q.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        let weak = ui.as_weak();
        std::thread::spawn(move || {
            for job in rx {
                run(&weak, job);
            }
        });
        Mutex::new(tx)
    })
}

/// Sets the pill from any thread (empty hides it), `progress` 0..1.
fn pill(weak: &slint::Weak<AppWindow>, text: &str, progress: f32) {
    let en = text.to_string();
    let w = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = w.upgrade() {
            ui.set_transfer(tr_ui(&ui, &en).into());
            ui.set_transfer_p(progress);
        }
    });
}

fn finished(weak: &slint::Weak<AppWindow>, en: String) {
    let w = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = w.upgrade() {
            ui.set_transfer("".into());
            ui.set_transfer_p(-1.0);
            let t = tr_ui(&ui, &en);
            show_toast(&ui, t);
        }
    });
}

fn run(weak: &slint::Weak<AppWindow>, job: Job) {
    let name = job
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let size = std::fs::metadata(&job.path).map(|m| m.len()).unwrap_or(0);
    let kind = files::kind(&job.path);
    let start = match kind {
        Kind::Install => format!("Installing {name}…"),
        Kind::Copy => format!("Sending {name}…"),
    };
    pill(weak, &start, 0.0);
    let client = AdbClient::default();
    let target = match job.transport_id {
        Some(id) => Target::TransportId(id),
        None => Target::Serial(&job.serial),
    };
    let mut last = std::time::Instant::now();
    let w = weak.clone();
    let mut progress = move |n: u64| {
        if last.elapsed().as_millis() >= 100 && size > 0 {
            last = std::time::Instant::now();
            pill(&w, &start, (n as f64 / size as f64).min(1.0) as f32);
        }
    };
    let t = std::time::Instant::now();
    let device = &job.device;
    match kind {
        Kind::Copy => match files::copy_to_download(&client, &target, &job.path, &mut progress) {
            Ok(saved) => {
                // File names stay out of the log (they can be private).
                log::info!(
                    "drop: a file ({size} bytes) to Download in {:?}",
                    t.elapsed()
                );
                finished(weak, format!("Saved on {device} › Download: {saved}"));
            }
            Err(e) => {
                log::warn!("drop: sending a file ({size} bytes) failed: {e}");
                finished(weak, format!("Could not send {name}: {e}"));
            }
        },
        Kind::Install => {
            // The copy is most of the time; the install itself has no progress.
            let r = files::install_apk(&client, &target, &job.path, &mut progress);
            match r {
                Ok(Installed::Ok) => {
                    log::info!("drop: installed an APK in {:?}", t.elapsed());
                    finished(weak, format!("{name} is installed on {device}."));
                }
                Ok(Installed::Failed(why)) => {
                    log::warn!("drop: APK install failed: {why}");
                    finished(weak, format!("{name} was not installed: {why}"));
                }
                Err(e) => {
                    log::warn!("drop: APK install failed: {e}");
                    finished(weak, format!("{name} was not installed: {e}"));
                }
            }
        }
    }
    pill(weak, "", -1.0);
}

/// The phone files go to: the one being mirrored, if any.
fn phone() -> Option<(String, Option<u64>, String)> {
    APP.with_borrow(|a| {
        a.android.as_ref()?;
        let d = a.apps_device.as_ref()?;
        Some((
            d.primary.serial.clone(),
            d.primary.transport_id,
            d.name.display.clone(),
        ))
    })
}

/// Watches the window for files dragged in from Explorer.
pub fn install(ui: &AppWindow) {
    install_ios(ui);
    let weak = ui.as_weak();
    ui.window().on_winit_window_event(move |_, ev| {
        let Some(ui) = weak.upgrade() else {
            return EventResult::Propagate;
        };
        match ev {
            WindowEvent::HoveredFile(_) => {
                let apple = ui.get_screen().as_str() != "android";
                ui.set_drop_hover(true);
                ui.set_drop_apple(apple);
                ui.set_drop_ok(if apple {
                    ui.get_mode_active().as_str() == "apple"
                } else {
                    phone().is_some()
                });
                EventResult::PreventDefault
            }
            WindowEvent::HoveredFileCancelled => {
                ui.set_drop_hover(false);
                EventResult::PreventDefault
            }
            WindowEvent::DroppedFile(path) => {
                ui.set_drop_hover(false);
                drop_file(&ui, path.clone());
                EventResult::PreventDefault
            }
            _ => EventResult::Propagate,
        }
    });
}

fn drop_file(ui: &AppWindow, path: PathBuf) {
    if ui.get_screen().as_str() != "android" {
        ios::dropped(ui, path);
        return;
    }
    let Some((serial, transport_id, device)) = phone() else {
        show_toast(
            ui,
            tr_ui(
                ui,
                "Start mirroring an Android phone first, then drop the file here.",
            ),
        );
        return;
    };
    if !path.is_file() {
        show_toast(ui, tr_ui(ui, "Folders cannot be sent yet: drop the files."));
        return;
    }
    let _ = queue(ui).lock().unwrap().send(Job {
        path,
        serial,
        transport_id,
        device,
    });
}

/// Registers the iPhone/iPad app picker's callbacks.
fn install_ios(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.on_files_pick(move |i| {
        if let Some(ui) = weak.upgrade() {
            ios::pick(&ui, i);
        }
    });
    let weak = ui.as_weak();
    ui.on_files_close(move || {
        if let Some(ui) = weak.upgrade() {
            ios::close(&ui);
        }
    });
    ui.on_transfer_cancel(|| {
        std::thread::spawn(|| {
            mirror_core::api::Api::new().post("/api/files/cancel", &serde_json::json!({}))
        });
    });
    let weak = ui.as_weak();
    ui.on_import_toggle(move |i| {
        if let Some(ui) = weak.upgrade() {
            ios::import_toggle(&ui, i);
        }
    });
    let weak = ui.as_weak();
    ui.on_import_all(move |on| {
        if let Some(ui) = weak.upgrade() {
            ios::import_all(&ui, on);
        }
    });
    let weak = ui.as_weak();
    ui.on_import_set_source(move |v| {
        if let Some(ui) = weak.upgrade() {
            ios::import_source(&ui, v.as_str());
        }
    });
    let weak = ui.as_weak();
    ui.on_import_change_dest(move || {
        if let Some(ui) = weak.upgrade() {
            ios::import_change_dest(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_import_start(move || {
        if let Some(ui) = weak.upgrade() {
            ios::import_start(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_import_close(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_import_open(false);
        }
    });
}

/// "Send to the phone…": Windows' Open dialog, then the same path as a drop.
pub fn send_dialog(ui: &AppWindow) {
    if ui.get_screen().as_str() == "android" {
        return;
    }
    if ui.get_mode_active().as_str() != "apple" {
        ios::need_apple_mode(ui);
        return;
    }
    let owner = crate::hwnd_of(ui.window())
        .map(|h| h.0 as isize)
        .unwrap_or(0);
    let weak = ui.as_weak();
    crate::dialogs::open(
        owner,
        crate::dialogs::Pick::Files,
        tr_ui(ui, "Send to the phone"),
        None,
        move |paths| {
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = weak.upgrade() {
                    for p in paths {
                        ios::dropped(&ui, p);
                    }
                }
            });
        },
    );
}

/// "Import from the phone…".
pub fn import_open(ui: &AppWindow) {
    if ui.get_mode_active().as_str() != "apple" {
        ios::need_apple_mode(ui);
        return;
    }
    ios::import_open(ui);
}

/// iPhone/iPad (Apple Driver mode): the backend streams the files into the
/// Documents folder of an app with File Sharing (mirroring/apple/files.py).
mod ios {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::time::Duration;

    use mirror_core::api::Api;
    use serde_json::{json, Value};
    use slint::ComponentHandle;

    use super::pill;
    use crate::{app_letters, app_tint, show_toast, tr_ui, AppRow, AppWindow, ImportRow};

    thread_local! {
        /// Files waiting for the user to pick an app.
        static PENDING: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
        /// (bundle id, name) of the apps shown.
        static APPS: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
    }

    fn msg(v: &Value) -> String {
        v.get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }

    fn set_title(ui: &AppWindow) {
        let n = PENDING.with_borrow(Vec::len);
        let t = tr_ui(
            ui,
            &format!("Send {n} item(s) to an app on the iPhone/iPad"),
        );
        ui.set_files_title(t.into());
    }

    fn list_error(ui: &AppWindow, v: Option<&Value>) -> String {
        let m = v.map(msg).unwrap_or_default();
        if !m.is_empty() {
            return m;
        }
        tr_ui(ui, "Could not read the iPhone/iPad apps. Check that the device is connected in Apple Driver mode and trusted.")
    }

    pub fn need_apple_mode(ui: &AppWindow) {
        show_toast(
            ui,
            tr_ui(
                ui,
                "Files to and from an iPhone/iPad need Apple Driver mode (Studio mode cannot).",
            ),
        );
    }

    pub fn dropped(ui: &AppWindow, path: PathBuf) {
        if ui.get_mode_active().as_str() != "apple" {
            need_apple_mode(ui);
            return;
        }
        if !path.exists() {
            return;
        }
        let first = PENDING.with_borrow_mut(|p| {
            p.push(path);
            p.len() == 1
        });
        set_title(ui);
        if !first || ui.get_files_open() {
            return;
        }
        ui.set_files_open(true);
        ui.set_files_busy(true);
        ui.set_files_apps(std::rc::Rc::new(slint::VecModel::<AppRow>::default()).into());
        ui.set_files_status(tr_ui(ui, "Reading the iPhone/iPad apps…").into());
        let weak = ui.as_weak();
        std::thread::spawn(move || {
            let r = Api::new().get_slow("/api/files/apps", 30);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else { return };
                ui.set_files_busy(false);
                let ok = r
                    .as_ref()
                    .and_then(|v| v.get("ok"))
                    .and_then(Value::as_bool)
                    == Some(true);
                if !ok {
                    let m = list_error(&ui, r.as_ref());
                    ui.set_files_status(m.into());
                    return;
                }
                let apps: Vec<(String, String)> = r
                    .as_ref()
                    .and_then(|v| v.get("apps"))
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| {
                                Some((
                                    x.get("bundle")?.as_str()?.to_string(),
                                    x.get("name")?.as_str()?.to_string(),
                                ))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let rows: Vec<AppRow> = apps
                    .iter()
                    .map(|(b, n)| AppRow {
                        name: n.as_str().into(),
                        sub: b.as_str().into(),
                        letter: app_letters(n).into(),
                        tint: app_tint(b),
                    })
                    .collect();
                let status = if rows.is_empty() {
                    tr_ui(&ui, "No app accepts files (File Sharing). Install e.g. VLC or Documents from the App Store.")
                } else {
                    tr_ui(
                        &ui,
                        "Pick an app: the files go to its folder and show in Files › On My iPhone.",
                    )
                };
                ui.set_files_status(status.into());
                APPS.with_borrow_mut(|a| *a = apps);
                ui.set_files_apps(std::rc::Rc::new(slint::VecModel::from(rows)).into());
            });
        });
    }

    pub fn close(ui: &AppWindow) {
        ui.set_files_open(false);
        PENDING.with_borrow_mut(Vec::clear);
    }

    pub fn pick(ui: &AppWindow, index: i32) {
        let Some((bundle, app)) = APPS.with_borrow(|a| a.get(index.max(0) as usize).cloned())
        else {
            return;
        };
        let paths = PENDING.with_borrow_mut(std::mem::take);
        ui.set_files_open(false);
        let weak = ui.as_weak();
        std::thread::spawn(move || {
            let api = Api::new();
            let body = json!({
                "paths": paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
                "bundle": bundle,
                "app": app,
            });
            let r = api.post_slow("/api/files/send", &body, 15);
            if r.as_ref()
                .and_then(|v| v.get("ok"))
                .and_then(Value::as_bool)
                != Some(true)
            {
                let m = r.as_ref().map(msg).unwrap_or_default();
                let en = if m.is_empty() {
                    "The files were not sent.".to_string()
                } else {
                    m
                };
                done_toast(&weak, en);
                return;
            }
            let j = follow(&weak, &api, "send");
            let state = j.get("state").and_then(Value::as_str).unwrap_or("");
            let sent = j.get("done").and_then(Value::as_u64).unwrap_or(0);
            let total = j.get("count").and_then(Value::as_u64).unwrap_or(0);
            let en = match state {
                "done" => {
                    format!("{sent} file(s) sent to {app}. Open {app} or Files › On My iPhone.")
                }
                "cancelled" => format!(
                    "Cancelled: {sent} of {total} file(s) sent; the unfinished one was removed."
                ),
                _ => format!("{sent} of {total} file(s) sent to {app}. {}", msg(&j)),
            };
            done_toast(&weak, en);
        });
    }

    fn done_toast(weak: &slint::Weak<AppWindow>, en: String) {
        let w = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = w.upgrade() {
                ui.set_transfer_can_cancel(false);
                let t = tr_ui(&ui, &en);
                show_toast(&ui, t);
            }
        });
    }

    /// "12.5 MB" style size.
    pub fn size_text(n: u64) -> String {
        let n = n as f64;
        if n >= 1e9 {
            format!("{:.1} GB", n / 1e9)
        } else if n >= 1e6 {
            format!("{:.1} MB", n / 1e6)
        } else if n >= 1e3 {
            format!("{:.0} KB", n / 1e3)
        } else {
            format!("{n:.0} B")
        }
    }

    /// Shows the backend's transfer in the pill until it ends; returns the
    /// final job. The pill: name · (3/12) · 37% · 4.2 MB/s, and Cancel.
    fn follow(weak: &slint::Weak<AppWindow>, api: &Api, direction: &str) -> Value {
        let w = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = w.upgrade() {
                ui.set_transfer_can_cancel(true);
            }
        });
        let mut misses = 0;
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let Some(j) = api.get("/api/files/job") else {
                misses += 1;
                if misses > 40 {
                    pill(weak, "", -1.0);
                    return json!({"state": "error", "message": "The 3zT Mirroring service stopped answering."});
                }
                continue;
            };
            misses = 0;
            let state = j.get("state").and_then(Value::as_str).unwrap_or("");
            if state != "running" {
                pill(weak, "", -1.0);
                return j;
            }
            let num = |k: &str| j.get(k).and_then(Value::as_u64).unwrap_or(0);
            let (done, size) = (num("sent"), num("total"));
            let p = if size > 0 {
                done as f32 / size as f32
            } else {
                0.0
            };
            let name = j.get("name").and_then(Value::as_str).unwrap_or("");
            let speed = num("speed");
            let verb = if direction == "send" {
                "Sending"
            } else {
                "Importing"
            };
            let label = format!(
                "{verb} {name} ({}/{}) · {:.0}%{}",
                num("index").max(1),
                num("count").max(1),
                p * 100.0,
                if speed > 0 {
                    format!(" · {}/s", size_text(speed))
                } else {
                    String::new()
                }
            );
            pill(weak, &label, p);
        }
    }

    // ---- Import from the phone -------------------------------------------

    thread_local! {
        /// Device paths (or app bundle ids in the app list) behind the rows.
        static IMPORT_KEYS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        /// "media" or an app's bundle id: where the listed files come from.
        static IMPORT_FROM: RefCell<String> = RefCell::new("media".into());
        static IMPORT_DEST: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    fn dest() -> PathBuf {
        IMPORT_DEST
            .with_borrow(|d| d.clone())
            .unwrap_or_else(crate::dialogs::default_import_folder)
    }

    pub fn import_open(ui: &AppWindow) {
        ui.set_import_open(true);
        ui.set_import_dest(dest().to_string_lossy().to_string().into());
        import_source(ui, "media");
    }

    fn set_rows(ui: &AppWindow, rows: Vec<ImportRow>, keys: Vec<String>) {
        IMPORT_KEYS.with_borrow_mut(|k| *k = keys);
        ui.set_import_picked(rows.iter().filter(|r| r.checked).count() as i32);
        ui.set_import_items(std::rc::Rc::new(slint::VecModel::from(rows)).into());
    }

    fn date_text(t: i64) -> String {
        // Local date and time from a Unix time, without a date crate:
        // days since 1970 → civil date (Howard Hinnant's algorithm).
        let offset = local_offset_secs();
        let t = t + offset;
        let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        format!(
            "{y}-{m:02}-{d:02} {:02}:{:02}",
            secs / 3600,
            secs % 3600 / 60
        )
    }

    #[cfg(windows)]
    fn local_offset_secs() -> i64 {
        use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
        let mut tz = TIME_ZONE_INFORMATION::default();
        let r = unsafe { GetTimeZoneInformation(&mut tz) };
        let bias = tz.Bias + if r == 2 { tz.DaylightBias } else { 0 };
        -(bias as i64) * 60
    }

    #[cfg(not(windows))]
    fn local_offset_secs() -> i64 {
        0
    }

    pub fn import_source(ui: &AppWindow, source: &str) {
        ui.set_import_source(if source == "apps" { "apps" } else { "media" }.into());
        set_rows(ui, Vec::new(), Vec::new());
        ui.set_import_busy(true);
        ui.set_import_status(
            tr_ui(
                ui,
                if source == "apps" {
                    "Reading the iPhone/iPad apps…"
                } else {
                    "Reading the photos and videos on the device…"
                },
            )
            .into(),
        );
        let weak = ui.as_weak();
        let apps = source == "apps";
        std::thread::spawn(move || {
            let r = Api::new().get_slow(
                if apps {
                    "/api/files/apps"
                } else {
                    "/api/files/media"
                },
                120,
            );
            let _ =
                slint::invoke_from_event_loop(move || {
                    let Some(ui) = weak.upgrade() else { return };
                    ui.set_import_busy(false);
                    let ok = r
                        .as_ref()
                        .and_then(|v| v.get("ok"))
                        .and_then(Value::as_bool)
                        == Some(true);
                    if !ok {
                        let m = list_error(&ui, r.as_ref());
                        ui.set_import_status(m.into());
                        return;
                    }
                    let v = r.unwrap_or_default();
                    if apps {
                        let list: Vec<(String, String)> = v
                            .get("apps")
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(|x| {
                                        Some((
                                            x.get("bundle")?.as_str()?.to_string(),
                                            x.get("name")?.as_str()?.to_string(),
                                        ))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        let rows = list
                            .iter()
                            .map(|(b, n)| ImportRow {
                                name: n.as_str().into(),
                                sub: b.as_str().into(),
                                kind: "app".into(),
                                checked: false,
                            })
                            .collect();
                        ui.set_import_status(tr_ui(&ui, if list.is_empty() {
                        "No app shares files (File Sharing). Apps like VLC or Documents do."
                    } else {
                        "Pick an app to see the files it shares."
                    }).into());
                        set_rows(&ui, rows, list.into_iter().map(|(b, _)| b).collect());
                    } else {
                        IMPORT_FROM.with_borrow_mut(|f| *f = "media".into());
                        show_files(&ui, &v, true);
                    }
                });
        });
    }

    fn show_files(ui: &AppWindow, v: &Value, media: bool) {
        let items = v
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut rows = Vec::new();
        let mut keys = Vec::new();
        for it in &items {
            let (Some(path), Some(name)) = (
                it.get("path").and_then(Value::as_str),
                it.get("name").and_then(Value::as_str),
            ) else {
                continue;
            };
            let size = it.get("size").and_then(Value::as_u64).unwrap_or(0);
            let time = it.get("time").and_then(Value::as_i64).unwrap_or(0);
            rows.push(ImportRow {
                name: name.into(),
                sub: format!("{} · {}", date_text(time), size_text(size)).into(),
                kind: it
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("other")
                    .into(),
                checked: false,
            });
            keys.push(path.to_string());
        }
        let status = if rows.is_empty() {
            tr_ui(
                ui,
                if media {
                    "No photos or videos on the device. Photos kept only in iCloud are not on the device and can't be imported here."
                } else {
                    "This app shares no files."
                },
            )
        } else if media {
            tr_ui(ui, "Newest first. Photos kept only in iCloud are not listed. Files come as they are (HEIC stays HEIC).")
        } else {
            tr_ui(ui, "Newest first. Files come as they are.")
        };
        ui.set_import_status(status.into());
        ui.set_import_source(if media { "media" } else { "app" }.into());
        set_rows(ui, rows, keys);
    }

    pub fn import_toggle(ui: &AppWindow, i: i32) {
        let i = i.max(0) as usize;
        if ui.get_import_source().as_str() == "apps" {
            // An app in the list: show its files.
            let Some(bundle) = IMPORT_KEYS.with_borrow(|k| k.get(i).cloned()) else {
                return;
            };
            IMPORT_FROM.with_borrow_mut(|f| *f = bundle.clone());
            set_rows(ui, Vec::new(), Vec::new());
            ui.set_import_busy(true);
            ui.set_import_status(tr_ui(ui, "Reading the app's files…").into());
            let weak = ui.as_weak();
            std::thread::spawn(move || {
                let r = Api::new().get_slow(&format!("/api/files/app-files?bundle={bundle}"), 120);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = weak.upgrade() else { return };
                    ui.set_import_busy(false);
                    match r.filter(|v| v.get("ok").and_then(Value::as_bool) == Some(true)) {
                        Some(v) => show_files(&ui, &v, false),
                        None => ui.set_import_status(list_error(&ui, None).into()),
                    }
                });
            });
            return;
        }
        let model = ui.get_import_items();
        if let Some(mut row) = slint::Model::row_data(&model, i) {
            row.checked = !row.checked;
            slint::Model::set_row_data(&model, i, row);
            let n = slint::Model::iter(&model).filter(|r| r.checked).count();
            ui.set_import_picked(n as i32);
        }
    }

    pub fn import_all(ui: &AppWindow, on: bool) {
        let model = ui.get_import_items();
        for i in 0..slint::Model::row_count(&model) {
            if let Some(mut row) = slint::Model::row_data(&model, i) {
                row.checked = on;
                slint::Model::set_row_data(&model, i, row);
            }
        }
        ui.set_import_picked(if on {
            slint::Model::row_count(&model) as i32
        } else {
            0
        });
    }

    pub fn import_change_dest(ui: &AppWindow) {
        let owner = crate::hwnd_of(ui.window())
            .map(|h| h.0 as isize)
            .unwrap_or(0);
        let weak = ui.as_weak();
        crate::dialogs::open(
            owner,
            crate::dialogs::Pick::Folder,
            tr_ui(ui, "Folder for files from the phone"),
            Some(dest()),
            move |paths| {
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = weak.upgrade() else { return };
                    if let Some(p) = paths.into_iter().next() {
                        ui.set_import_dest(p.to_string_lossy().to_string().into());
                        IMPORT_DEST.with_borrow_mut(|d| *d = Some(p));
                    }
                });
            },
        );
    }

    pub fn import_start(ui: &AppWindow) {
        let model = ui.get_import_items();
        let keys = IMPORT_KEYS.with_borrow(|k| k.clone());
        let picked: Vec<String> = slint::Model::iter(&model)
            .enumerate()
            .filter(|(_, r)| r.checked)
            .filter_map(|(i, _)| keys.get(i).cloned())
            .collect();
        if picked.is_empty() {
            return;
        }
        let source = IMPORT_FROM.with_borrow(|f| f.clone());
        let dest = dest();
        ui.set_import_open(false);
        let weak = ui.as_weak();
        std::thread::spawn(move || {
            let api = Api::new();
            let body = json!({"source": source, "items": picked, "dest": dest.to_string_lossy()});
            let r = api.post_slow("/api/files/pull", &body, 15);
            if r.as_ref()
                .and_then(|v| v.get("ok"))
                .and_then(Value::as_bool)
                != Some(true)
            {
                let m = r.as_ref().map(msg).unwrap_or_default();
                done_toast(
                    &weak,
                    if m.is_empty() {
                        "Nothing was imported.".into()
                    } else {
                        m
                    },
                );
                return;
            }
            let j = follow(&weak, &api, "import");
            let state = j
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let got = j.get("done").and_then(Value::as_u64).unwrap_or(0);
            let total = j.get("count").and_then(Value::as_u64).unwrap_or(0);
            let message = msg(&j);
            let w = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = w.upgrade() else { return };
                ui.set_transfer_can_cancel(false);
                let body = match state.as_str() {
                    "done" => tr_ui(&ui, &format!("{got} file(s) saved in {}.", dest.display())),
                    "cancelled" => tr_ui(&ui, &format!("Cancelled: {got} of {total} file(s) saved; the unfinished one was removed.")),
                    _ => tr_ui(&ui, &format!("{got} of {total} file(s) saved. {message}")),
                };
                crate::import_done_sheet(&ui, body, got > 0, dest);
            });
        });
    }
}
