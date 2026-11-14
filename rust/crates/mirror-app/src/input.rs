//! Input worker: posts touches, strokes and buttons to the backend in order,
//! off the UI thread. Consecutive moves are coalesced (only the newest is sent).

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use mirror_core::api::{device_point, Api, TouchKind};

#[derive(Clone, Debug)]
pub enum Cmd {
    Down(f32, f32),
    Move(f32, f32),
    Up(f32, f32),
    /// u0, v0, u1, v1, steps, ms per step
    Stroke(f32, f32, f32, f32, u32, u64),
    Home(u32),
    Action(&'static str),
    /// Apple hardware keyboard: one key tap (HID usages held, then none).
    Keys(Vec<u8>),
    /// Apple hardware keyboard: switch the device's keyboard language
    /// (Ctrl+Space), then wait until iOS has switched its layout.
    SwitchLanguage,
}

/// How long iOS needs after Ctrl+Space before the next key lands in the new
/// layout (ms).
pub static SWITCH_WAIT_MS: AtomicU32 = AtomicU32::new(150);

#[derive(Clone)]
pub struct Input {
    tx: Sender<Cmd>,
    /// Rotation the picture is shown with (degrees), for touch mapping.
    pub rotation: Arc<AtomicU32>,
}

impl Input {
    pub fn new(api: Api) -> Self {
        let (tx, rx) = channel();
        let rotation = Arc::new(AtomicU32::new(0));
        let rot = rotation.clone();
        std::thread::Builder::new()
            .name("mirroring-input".into())
            .spawn(move || worker(api, rx, rot))
            .expect("input thread");
        Self { tx, rotation }
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
}

/// The HID reports of one key tap. iOS ignores a modifier that arrives in the
/// same report as the key (tested on iOS 27.2: Shift+A typed "a"), so the
/// modifiers go down first, are held with the key, and come up last.
pub fn key_reports(usages: &[u8]) -> Vec<Vec<u8>> {
    let mods: Vec<u8> = usages.iter().copied().filter(|u| *u >= 0xe0).collect();
    let mut out = Vec::with_capacity(4);
    if !mods.is_empty() && mods.len() < usages.len() {
        out.push(mods.clone());
        out.push(usages.to_vec());
        out.push(mods);
    } else {
        out.push(usages.to_vec());
    }
    out.push(Vec::new());
    out
}

fn worker(api: Api, rx: Receiver<Cmd>, rot: Arc<AtomicU32>) {
    let pt = |u: f32, v: f32| device_point(u, v, rot.load(Relaxed));
    let mut next: Option<Cmd> = None;
    loop {
        let cmd = match next.take() {
            Some(c) => c,
            None => match rx.recv() {
                Ok(c) => c,
                Err(_) => return,
            },
        };
        match cmd {
            Cmd::Move(mut u, mut v) => {
                // Skip to the newest queued move.
                while let Ok(c) = rx.try_recv() {
                    match c {
                        Cmd::Move(a, b) => (u, v) = (a, b),
                        other => {
                            next = Some(other);
                            break;
                        }
                    }
                }
                let (x, y) = pt(u, v);
                api.touch(TouchKind::Contact, x, y);
            }
            Cmd::Down(u, v) => {
                let (x, y) = pt(u, v);
                api.touch(TouchKind::Contact, x, y);
            }
            Cmd::Up(u, v) => {
                let (x, y) = pt(u, v);
                api.touch(TouchKind::Release, x, y);
            }
            Cmd::Stroke(u0, v0, u1, v1, steps, dt) => {
                let (x, y) = pt(u0, v0);
                api.touch(TouchKind::Contact, x, y);
                for i in 1..=steps {
                    std::thread::sleep(Duration::from_millis(dt));
                    let k = i as f32 / steps as f32;
                    let (x, y) = pt(u0 + (u1 - u0) * k, v0 + (v1 - v0) * k);
                    api.touch(TouchKind::Contact, x, y);
                }
                let (x, y) = pt(u1, v1);
                api.touch(TouchKind::Release, x, y);
            }
            Cmd::Home(n) => {
                for i in 0..n {
                    if i > 0 {
                        std::thread::sleep(Duration::from_millis(120));
                    }
                    api.button("home");
                }
            }
            Cmd::Action(a) => api.keyboard_action(a),
            Cmd::Keys(usages) => {
                for report in key_reports(&usages) {
                    api.key(&serde_json::json!({ "usages": report }));
                }
            }
            Cmd::SwitchLanguage => {
                for report in key_reports(&[crate::shortcuts::HID_CTRL, 0x2c]) {
                    api.key(&serde_json::json!({ "usages": report }));
                }
                std::thread::sleep(Duration::from_millis(SWITCH_WAIT_MS.load(Relaxed) as u64));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::key_reports;

    #[test]
    fn modifiers_go_down_before_the_key_and_up_after_it() {
        assert_eq!(key_reports(&[0x04]), vec![vec![0x04], vec![]]);
        assert_eq!(
            key_reports(&[0xe1, 0x04]),
            vec![vec![0xe1], vec![0xe1, 0x04], vec![0xe1], vec![]]
        );
        // A modifier on its own is just pressed and released.
        assert_eq!(key_reports(&[0xe0]), vec![vec![0xe0], vec![]]);
    }
}
