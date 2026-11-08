//! Interface language. The Slint UI uses `@tr()` with the translations
//! bundled at build time; text made in Rust (toasts, device rows, tray
//! menu) and messages from the service go through `tr()`, which reads the
//! same `.po` file. English is the source text, so anything without a
//! translation simply shows in English.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

static ARABIC: AtomicBool = AtomicBool::new(false);

const AR_PO: &str = include_str!("../translations/ar/LC_MESSAGES/mirror-app.po");

struct Table {
    exact: HashMap<String, String>,
    /// Entries with `{}` placeholders: literal pieces and the translation,
    /// longest literal text first so the most specific pattern wins.
    patterns: Vec<(Vec<String>, String)>,
}

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| {
        let mut exact = HashMap::new();
        let mut patterns = Vec::new();
        for (id, s) in parse_po(AR_PO) {
            if id.is_empty() || s.is_empty() {
                continue;
            }
            if id.contains("{}") {
                let pieces: Vec<String> = id.split("{}").map(str::to_string).collect();
                patterns.push((pieces, s.clone()));
            }
            exact.insert(id, s);
        }
        patterns.sort_by_key(|(p, _)| std::cmp::Reverse(p.iter().map(String::len).sum::<usize>()));
        Table { exact, patterns }
    })
}

/// msgid/msgstr pairs of a `.po` file (no plural forms, no context).
fn parse_po(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let (mut id, mut s) = (String::new(), String::new());
    let mut cur: Option<bool> = None; // Some(true) = msgid, Some(false) = msgstr
    let flush = |id: &mut String, s: &mut String, out: &mut Vec<(String, String)>| {
        if !id.is_empty() || !s.is_empty() {
            out.push((std::mem::take(id), std::mem::take(s)));
        }
    };
    for line in src.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("msgid ") {
            flush(&mut id, &mut s, &mut out);
            id = unquote(rest);
            cur = Some(true);
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            s = unquote(rest);
            cur = Some(false);
        } else if line.starts_with('"') {
            match cur {
                Some(true) => id += &unquote(line),
                Some(false) => s += &unquote(line),
                None => {}
            }
        } else if line.is_empty() {
            cur = None;
        }
    }
    flush(&mut id, &mut s, &mut out);
    out
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s);
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

/// "" = follow Windows, "en", "ar".
pub fn apply(choice: &str) {
    let ar = match choice {
        "ar" => true,
        "en" => false,
        _ => windows_is_arabic(),
    };
    ARABIC.store(ar, Relaxed);
    let _ = slint::select_bundled_translation(if ar { "ar" } else { "" });
}

pub fn is_arabic() -> bool {
    ARABIC.load(Relaxed)
}

#[cfg(windows)]
fn windows_is_arabic() -> bool {
    // PRIMARYLANGID(LANG_ARABIC) = 0x01.
    let lang = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    lang & 0x3ff == 0x01
}

#[cfg(not(windows))]
fn windows_is_arabic() -> bool {
    std::env::var("LANG").is_ok_and(|l| l.starts_with("ar"))
}

/// The text in the chosen language.
pub fn tr(en: &str) -> String {
    if !is_arabic() || en.is_empty() {
        return en.to_string();
    }
    lookup(en).unwrap_or_else(|| by_sentence(en))
}

/// `tr` for places that need a `&'static str` (tray menu).
pub fn tr_static(en: &'static str) -> &'static str {
    if !is_arabic() {
        return en;
    }
    static CACHE: Mutex<Option<HashMap<&'static str, &'static str>>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let map = c.get_or_insert_with(HashMap::new);
    map.entry(en)
        .or_insert_with(|| Box::leak(tr(en).into_boxed_str()))
}

fn lookup(en: &str) -> Option<String> {
    let t = table();
    if let Some(s) = t.exact.get(en) {
        return Some(s.clone());
    }
    for (pieces, s) in &t.patterns {
        if let Some(args) = matches(pieces, en) {
            let mut out = String::new();
            let mut rest = s.as_str();
            let mut args = args.into_iter();
            while let Some(i) = rest.find("{}") {
                out += &rest[..i];
                let a = args.next().unwrap_or_default();
                // A captured word may be translatable on its own ("ready").
                out += &t.exact.get(a.as_str()).cloned().unwrap_or(a);
                rest = &rest[i + 2..];
            }
            out += rest;
            return Some(out);
        }
    }
    None
}

/// Literal pieces around `{}` placeholders -> the captured texts.
fn matches(pieces: &[String], text: &str) -> Option<Vec<String>> {
    let first = pieces.first()?;
    let last = pieces.last()?;
    if pieces.len() < 2 || pieces.iter().map(String::len).sum::<usize>() < 3 {
        return None;
    }
    let mut rest = text.strip_prefix(first.as_str())?;
    let mut caps = Vec::new();
    for p in &pieces[1..pieces.len() - 1] {
        if p.is_empty() {
            return None;
        }
        let i = rest.find(p.as_str())?;
        if i == 0 {
            return None;
        }
        caps.push(rest[..i].to_string());
        rest = &rest[i + p.len()..];
    }
    let tail = rest.strip_suffix(last.as_str())?;
    if tail.is_empty() {
        return None;
    }
    caps.push(tail.to_string());
    Some(caps)
}

/// Messages from the service often join a few sentences: translate each
/// one that has a translation and keep the others as they are.
fn by_sentence(en: &str) -> String {
    let mut out = String::new();
    let mut start = 0;
    let b = en.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let end = (b[i] == b'.' || b[i] == b'?' || b[i] == b'!' || b[i] == b'\n')
            && (i + 1 == b.len() || b[i + 1] == b' ' || b[i + 1] == b'\n');
        if end {
            let piece = &en[start..=i];
            let trimmed = piece.trim_start();
            let lead = &piece[..piece.len() - trimmed.len()];
            out += lead;
            out += &lookup(trimmed).unwrap_or_else(|| trimmed.to_string());
            start = i + 1;
        }
        i += 1;
    }
    if start < en.len() {
        let piece = &en[start..];
        let trimmed = piece.trim_start();
        out += &piece[..piece.len() - trimmed.len()];
        out += &lookup(trimmed).unwrap_or_else(|| trimmed.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_translation_file_parses() {
        let t = table();
        assert!(t.exact.len() > 300, "{}", t.exact.len());
        for (id, s) in parse_po(AR_PO) {
            if id.is_empty() {
                continue;
            }
            assert_eq!(
                id.matches("{}").count(),
                s.matches("{}").count(),
                "placeholders differ: {id}"
            );
        }
    }

    #[test]
    fn exact_patterns_and_sentences() {
        ARABIC.store(true, Relaxed);
        assert_eq!(tr("Settings"), table().exact["Settings"]);
        let opening = tr("Opening Photos…");
        assert!(
            opening.contains("Photos") && !opening.starts_with("Opening"),
            "{opening}"
        );
        let two = tr("Start mirroring first. Something unknown here.");
        assert!(two.ends_with("Something unknown here."), "{two}");
        assert!(!two.starts_with("Start"), "{two}");
        assert_eq!(tr("no translation for this"), "no translation for this");
        ARABIC.store(false, Relaxed);
        assert_eq!(tr("Settings"), "Settings");
    }
}
