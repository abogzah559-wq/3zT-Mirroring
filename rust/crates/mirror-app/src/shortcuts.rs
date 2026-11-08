//! Keyboard shortcuts of the window, for both platforms.
//!
//! The set the README and Settings list. Pure logic: `route`
//! decides, `main.rs` performs. F11 / Esc (full screen) stay in Slint.
//!
//! Every app shortcut uses Alt or Ctrl: plain keys
//! (Space, M...) always go to the device, so typing never stops the
//! mirroring or mutes the sound by accident.

/// What a key press does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    /// Alt+M: sound on the PC on / off.
    Mute,
    /// Ctrl+, : open / close Settings.
    Settings,
    /// Alt+H: rotation Auto → 90° → 270°.
    Rotate,
    /// Alt+Left: Back (Apple: swipe from the left edge).
    Back,
    /// Alt+L: lock (Android: power button).
    Lock,
    /// Alt+C: copy the text on the device screen to the PC (Windows OCR).
    CopyText,
    /// Alt+R: forget the copied-text history (the next Alt+C starts over).
    ClearText,
    /// Apple Driver mode actions (`/api/keyboard/action`).
    Apple(&'static str),
    /// Android buttons (`android::Shared::action`).
    Android(&'static str),
}

/// Slint key codes (private-use characters).
const LEFT: char = '\u{F702}';
const UP: char = '\u{F700}';
const DOWN: char = '\u{F701}';

/// `typing`: the PC keyboard currently types on a live device.
pub fn route(
    android: bool,
    text: &str,
    ctrl: bool,
    shift: bool,
    alt: bool,
    typing: bool,
) -> Option<Shortcut> {
    let mut chars = text.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return None;
    };
    let lower = c.to_ascii_lowercase();
    if alt && !ctrl {
        return match (lower, android) {
            ('h', _) => Some(Shortcut::Rotate),
            ('m', _) => Some(Shortcut::Mute),
            ('c', _) => Some(Shortcut::CopyText),
            (LEFT, _) => Some(Shortcut::Back),
            ('l', _) => Some(Shortcut::Lock),
            ('u', false) => Some(Shortcut::Apple("wake")),
            ('d', false) => Some(Shortcut::Apple("stealth")),
            ('v', false) => Some(Shortcut::Apple("paste-pc")),
            ('n', _) => Some(Shortcut::Android("notifications")),
            // Like scrcpy: MOD+S = app switch; Quick Settings on Q. On
            // iPhone / iPad, Q opens Control Center (the window's action
            // tracks whether it is open, so a second press closes it).
            ('q', true) => Some(Shortcut::Android("quick-settings")),
            ('q', false) => Some(Shortcut::Android("control-center")),
            ('s', true) => Some(Shortcut::Android("switcher")),
            ('r', _) => Some(Shortcut::ClearText),
            (UP, true) => Some(Shortcut::Android("vol-up")),
            (DOWN, true) => Some(Shortcut::Android("vol-down")),
            _ => None,
        };
    }
    if ctrl && !alt {
        if c == ',' {
            return Some(Shortcut::Settings);
        }
        // Apple: Ctrl+letter = Cmd+letter on the device; Ctrl+K keyboard.
        // (Android maps Ctrl+letter itself in `android::key_input`.)
        if !android && !shift {
            return match lower {
                'k' => Some(Shortcut::Apple("toggle-keyboard")),
                'c' => Some(Shortcut::Apple("copy")),
                'v' => Some(Shortcut::Apple("paste")),
                'x' => Some(Shortcut::Apple("cut")),
                'a' => Some(Shortcut::Apple("select-all")),
                'z' => Some(Shortcut::Apple("undo")),
                _ => None,
            };
        }
        return None;
    }
    let _ = typing; // plain keys are never shortcuts
    None
}

/// HID keyboard usage (and whether Shift is needed) for a typed character,
/// US layout, for the Apple hardware-keyboard path (`/cast/key`). The
/// iPhone's own layout setting decides the final character.
pub fn hid_usage(c: char) -> Option<(u8, bool)> {
    const SHIFTED_DIGITS: &str = "!@#$%^&*()";
    Some(match c {
        'a'..='z' => (0x04 + (c as u8 - b'a'), false),
        'A'..='Z' => (0x04 + (c as u8 - b'A'), true),
        '1'..='9' => (0x1e + (c as u8 - b'1'), false),
        '0' => (0x27, false),
        '\n' | '\r' => (0x28, false),
        '\u{1b}' => (0x29, false),
        '\u{8}' => (0x2a, false),
        '\t' => (0x2b, false),
        ' ' => (0x2c, false),
        '-' => (0x2d, false),
        '_' => (0x2d, true),
        '=' => (0x2e, false),
        '+' => (0x2e, true),
        '[' => (0x2f, false),
        '{' => (0x2f, true),
        ']' => (0x30, false),
        '}' => (0x30, true),
        '\\' => (0x31, false),
        '|' => (0x31, true),
        ';' => (0x33, false),
        ':' => (0x33, true),
        '\'' => (0x34, false),
        '"' => (0x34, true),
        '`' => (0x35, false),
        '~' => (0x35, true),
        ',' => (0x36, false),
        '<' => (0x36, true),
        '.' => (0x37, false),
        '>' => (0x37, true),
        '/' => (0x38, false),
        '?' => (0x38, true),
        '\u{7f}' => (0x4c, false),
        '\u{F729}' => (0x4a, false),
        '\u{F72B}' => (0x4d, false),
        '\u{F72C}' => (0x4b, false),
        '\u{F72D}' => (0x4e, false),
        '\u{F703}' => (0x4f, false),
        LEFT => (0x50, false),
        DOWN => (0x51, false),
        UP => (0x52, false),
        c => match SHIFTED_DIGITS.find(c) {
            // ! = Shift+1 ... ) = Shift+0
            Some(i) => (if i == 9 { 0x27 } else { 0x1e + i as u8 }, true),
            None => return None,
        },
    })
}

/// Left Shift HID usage.
pub const HID_SHIFT: u8 = 0xe1;

/// Left Control HID usage.
pub const HID_CTRL: u8 = 0xe0;

/// HID usage (and Shift) of the key that types `c` on the Arabic PC layout
/// (Windows "Arabic (101)"; on the iPhone/iPad: Settings › General ›
/// Keyboard › Hardware Keyboard › Arabic › "Arabic – PC"). Only Arabic
/// characters are listed; everything else goes through [`hid_usage`].
pub fn arabic_usage(c: char) -> Option<(u8, bool)> {
    Some(match c {
        // Top letter row: q w e r t y u i o p [ ]
        'ض' => (0x14, false),
        'ص' => (0x1a, false),
        'ث' => (0x08, false),
        'ق' => (0x15, false),
        'ف' => (0x17, false),
        'غ' => (0x1c, false),
        'ع' => (0x18, false),
        'ه' => (0x0c, false),
        'خ' => (0x12, false),
        'ح' => (0x13, false),
        'ج' => (0x2f, false),
        'د' => (0x30, false),
        // Home row: a s d f g h j k l ; '
        'ش' => (0x04, false),
        'س' => (0x16, false),
        'ي' => (0x07, false),
        'ب' => (0x09, false),
        'ل' => (0x0a, false),
        'ا' => (0x0b, false),
        'ت' => (0x0d, false),
        'ن' => (0x0e, false),
        'م' => (0x0f, false),
        'ك' => (0x33, false),
        'ط' => (0x34, false),
        // Bottom row: z x c v (b = لا, typed as ل + ا) n m , . /
        'ئ' => (0x1d, false),
        'ء' => (0x1b, false),
        'ؤ' => (0x06, false),
        'ر' => (0x19, false),
        'ى' => (0x11, false),
        'ة' => (0x10, false),
        'و' => (0x36, false),
        'ز' => (0x37, false),
        'ظ' => (0x38, false),
        // ` key
        'ذ' => (0x35, false),
        // Shifted: harakat and the hamza / madda alifs.
        '\u{064e}' => (0x14, true), // fatha
        '\u{064b}' => (0x1a, true), // fathatan
        '\u{064f}' => (0x08, true), // damma
        '\u{064c}' => (0x15, true), // dammatan
        '\u{0650}' => (0x04, true), // kasra
        '\u{064d}' => (0x16, true), // kasratan
        '\u{0652}' => (0x1b, true), // sukun
        '\u{0651}' => (0x35, true), // shadda
        'إ' => (0x1c, true),
        'أ' => (0x0b, true),
        'آ' => (0x11, true),
        'ـ' => (0x0d, true),
        '،' => (0x0e, true),
        '؛' => (0x13, true),
        '؟' => (0x38, true),
        _ => return None,
    })
}

/// ASCII punctuation that the Arabic PC layout has in a different place
/// than the US layout: while the device types Arabic, these keys give the
/// same character as on the PC.
pub fn arabic_pc_ascii(c: char) -> Option<(u8, bool)> {
    Some(match c {
        '[' => (0x09, true), // Shift+F
        ']' => (0x07, true), // Shift+D
        '{' => (0x19, true), // Shift+V
        '}' => (0x06, true), // Shift+C
        '<' => (0x2f, true), // Shift+[
        '>' => (0x30, true), // Shift+]
        '/' => (0x0f, true), // Shift+L
        ':' => (0x33, true), // Shift+;
        '"' => (0x34, true), // Shift+'
        ',' => (0x36, true), // Shift+,
        '.' => (0x37, true), // Shift+.
        '~' => (0x1d, true), // Shift+Z
        '÷' => (0x0c, true), // Shift+I
        '×' => (0x12, true), // Shift+O
        '‘' => (0x18, true), // Shift+U
        '’' => (0x10, true), // Shift+M
        _ => return None,
    })
}

/// How a character must be typed on the device's hardware keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    /// Needs the device's Arabic (PC) layout.
    Arabic,
    /// Needs the device's English layout (Latin letters, and punctuation the
    /// Arabic layout does not have in the US place).
    English,
    /// Same key in both layouts (digits, space, Enter, Backspace, arrows...).
    Either,
}

/// Which device layout `c` needs, and its key on that layout. `arabic`: the
/// device keyboard is Arabic now (decides keys that exist in both).
pub fn device_key(c: char, arabic: bool) -> Option<(Script, u8, bool)> {
    if let Some((u, s)) = arabic_usage(c) {
        return Some((Script::Arabic, u, s));
    }
    if arabic {
        if let Some((u, s)) = arabic_pc_ascii(c) {
            return Some((Script::Arabic, u, s));
        }
    }
    let (u, s) = hid_usage(c)?;
    // Digits, the shifted digits, - = and the control keys (Enter, Esc,
    // Backspace, Tab, Space, Delete, arrows...) are the same in both layouts.
    let either =
        c.is_ascii_digit() || "!@#$%^&*-_=+".contains(c) || (0x28..=0x2c).contains(&u) || u >= 0x4a;
    let script = if either {
        Script::Either
    } else {
        Script::English
    };
    Some((script, u, s))
}

/// The US-layout character of the key that typed `c` on the Arabic layout,
/// so Ctrl / Alt shortcuts work whichever layout Windows uses.
pub fn us_key(c: char) -> Option<char> {
    let (u, _) = arabic_usage(c)?;
    Some(match u {
        0x04..=0x1d => (b'a' + (u - 0x04)) as char,
        0x2f => '[',
        0x30 => ']',
        0x33 => ';',
        0x34 => '\'',
        0x35 => '`',
        0x36 => ',',
        0x37 => '.',
        0x38 => '/',
        _ => return None,
    })
}

/// A 32-bit top-down BMP (BGRA rows) for Windows OCR, which reads any
/// image Windows can decode; BMP needs no encoder in 3zT Mirroring.
pub fn bmp(width: u32, height: u32, bgra: &[u8]) -> Vec<u8> {
    let size = 54 + bgra.len() as u32;
    let mut b = Vec::with_capacity(size as usize);
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&size.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&54u32.to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&(width as i32).to_le_bytes());
    b.extend_from_slice(&(-(height as i32)).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    b.extend_from_slice(&(bgra.len() as u32).to_le_bytes());
    b.extend_from_slice(&2835u32.to_le_bytes());
    b.extend_from_slice(&2835u32.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(bgra);
    b
}

/// Standard base64 (RFC 4648), for the OCR request body.
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_keys_are_never_shortcuts_and_alt_m_mutes() {
        for typing in [false, true] {
            for android in [false, true] {
                assert_eq!(route(android, " ", false, false, false, typing), None);
                assert_eq!(route(android, "m", false, false, false, typing), None);
                assert_eq!(
                    route(android, "m", false, false, true, typing),
                    Some(Shortcut::Mute)
                );
                assert_eq!(
                    route(android, "M", false, true, true, typing),
                    Some(Shortcut::Mute)
                );
            }
        }
    }

    #[test]
    fn alt_shortcuts_work_while_typing() {
        assert_eq!(
            route(false, "h", false, false, true, true),
            Some(Shortcut::Rotate)
        );
        assert_eq!(
            route(true, "H", false, true, true, true),
            Some(Shortcut::Rotate)
        );
        assert_eq!(
            route(true, "\u{F702}", false, false, true, true),
            Some(Shortcut::Back)
        );
        assert_eq!(
            route(false, "l", false, false, true, true),
            Some(Shortcut::Lock)
        );
        assert_eq!(
            route(true, "l", false, false, true, true),
            Some(Shortcut::Lock)
        );
        assert_eq!(
            route(true, ",", true, false, false, true),
            Some(Shortcut::Settings)
        );
    }

    #[test]
    fn platform_specific_shortcuts() {
        assert_eq!(
            route(false, "u", false, false, true, true),
            Some(Shortcut::Apple("wake"))
        );
        assert_eq!(route(true, "u", false, false, true, true), None);
        assert_eq!(
            route(true, "n", false, false, true, true),
            Some(Shortcut::Android("notifications"))
        );
        // iPhone / iPad: Alt+N / Alt+Q open Notification / Control Center.
        assert_eq!(
            route(false, "n", false, false, true, true),
            Some(Shortcut::Android("notifications"))
        );
        assert_eq!(
            route(false, "q", false, false, true, true),
            Some(Shortcut::Android("control-center"))
        );
        assert_eq!(
            route(false, "k", true, false, false, true),
            Some(Shortcut::Apple("toggle-keyboard"))
        );
        // Android Ctrl+letter goes to android::key_input, not here.
        assert_eq!(route(true, "c", true, false, false, true), None);
        assert_eq!(
            route(false, "c", true, false, false, true),
            Some(Shortcut::Apple("copy"))
        );
    }

    #[test]
    fn copy_text_on_both_platforms() {
        assert_eq!(
            route(false, "c", false, false, true, true),
            Some(Shortcut::CopyText)
        );
        assert_eq!(
            route(true, "c", false, false, true, true),
            Some(Shortcut::CopyText)
        );
        assert_eq!(
            route(false, "r", false, false, true, true),
            Some(Shortcut::ClearText)
        );
        assert_eq!(
            route(true, "r", false, false, true, true),
            Some(Shortcut::ClearText)
        );
        assert_eq!(
            route(true, "s", false, false, true, true),
            Some(Shortcut::Android("switcher"))
        );
        assert_eq!(
            route(true, "q", false, false, true, true),
            Some(Shortcut::Android("quick-settings"))
        );
    }

    #[test]
    fn arabic_letters_map_to_the_arabic_pc_layout() {
        assert_eq!(arabic_usage('ش'), Some((0x04, false)));
        assert_eq!(arabic_usage('ل'), Some((0x0a, false)));
        assert_eq!(arabic_usage('ا'), Some((0x0b, false)));
        assert_eq!(arabic_usage('أ'), Some((0x0b, true)));
        assert_eq!(arabic_usage('؟'), Some((0x38, true)));
        assert_eq!(arabic_usage('a'), None);
        // Shortcuts on the Arabic layout: Ctrl+C types "ؤ", Alt+M types "ة".
        assert_eq!(us_key('ؤ'), Some('c'));
        assert_eq!(us_key('ة'), Some('m'));
        assert_eq!(us_key('و'), Some(','));
        // Punctuation while the device types Arabic.
        assert_eq!(device_key('.', true), Some((Script::Arabic, 0x37, true)));
        assert_eq!(device_key('.', false), Some((Script::English, 0x37, false)));
        assert_eq!(device_key('ش', false), Some((Script::Arabic, 0x04, false)));
        assert_eq!(device_key('a', true), Some((Script::English, 0x04, false)));
        assert_eq!(device_key('5', true), Some((Script::Either, 0x22, false)));
        assert_eq!(device_key(' ', true), Some((Script::Either, 0x2c, false)));
        assert_eq!(device_key('\n', true), Some((Script::Either, 0x28, false)));
        assert_eq!(device_key(';', true), Some((Script::English, 0x33, false)));
        // Every Arabic letter of the alphabet has a key.
        for c in "ابتثجحخدذرزسشصضطظعغفقكلمنهوي".chars() {
            assert!(arabic_usage(c).is_some(), "{c}");
        }
    }

    #[test]
    fn bmp_has_the_right_header_and_size() {
        let px = vec![1u8, 2, 3, 255, 4, 5, 6, 255];
        let b = bmp(2, 1, &px);
        assert_eq!(&b[..2], b"BM");
        assert_eq!(
            u32::from_le_bytes(b[2..6].try_into().unwrap()) as usize,
            b.len()
        );
        assert_eq!(b.len(), 54 + 8);
        // Top-down rows: negative height.
        assert_eq!(i32::from_le_bytes(b[22..26].try_into().unwrap()), -1);
        assert_eq!(&b[54..], &px[..]);
    }

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn hid_usages_match_the_classic_table() {
        assert_eq!(hid_usage('a'), Some((0x04, false)));
        assert_eq!(hid_usage('Z'), Some((0x1d, true)));
        assert_eq!(hid_usage('1'), Some((0x1e, false)));
        assert_eq!(hid_usage('0'), Some((0x27, false)));
        assert_eq!(hid_usage('!'), Some((0x1e, true)));
        assert_eq!(hid_usage(')'), Some((0x27, true)));
        assert_eq!(hid_usage(' '), Some((0x2c, false)));
        assert_eq!(hid_usage('\u{8}'), Some((0x2a, false)));
        assert_eq!(hid_usage('?'), Some((0x38, true)));
        assert_eq!(hid_usage('\u{F702}'), Some((0x50, false)));
        assert_eq!(hid_usage('é'), None);
    }
}
