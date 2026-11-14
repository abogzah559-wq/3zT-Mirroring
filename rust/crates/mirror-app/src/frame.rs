//! The iPhone frame drawn around the picture: geometry and finishes.
//!
//! Proportions and the twelve finishes follow terminal-phone 0.4.0
//! (`PhoneDisplay.swift`, MIT, see THIRD_PARTY_NOTICES.md), re-implemented
//! here: on a screen 402 points wide the black border is 9.5, the metal band
//! 6 and the side buttons stick out 3; the screen corners have a radius of
//! 0.137 × the screen width. Everything scales with the picture shown.

/// Units of the 402-point reference screen.
const REF: f32 = 402.0;
const BLACK: f32 = 9.5;
const BAND: f32 = 6.0;
const BUTTON: f32 = 3.0;
const CORNER: f32 = 0.137;

/// Band, shine (edge highlight) and button colours, 0xRRGGBB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Finish {
    pub band: u32,
    pub shine: u32,
    pub buttons: u32,
}

/// The twelve finishes: id, English name, colours.
pub const FINISHES: [(&str, &str, Finish); 12] = [
    ("graphite", "Graphite", f(0x4a4a50, 0x8e8e96, 0x3a3a40)),
    (
        "space-black",
        "Space Black",
        f(0x2a2a2d, 0x5e5e64, 0x1f1f22),
    ),
    ("silver", "Silver", f(0xc9cbce, 0xf6f7f8, 0xa9abaf)),
    ("white", "White", f(0xe9e7e1, 0xffffff, 0xcfccc4)),
    ("gold", "Gold", f(0xd9c39b, 0xf8ead0, 0xbea77d)),
    (
        "cosmic-orange",
        "Cosmic Orange",
        f(0xe0692a, 0xffae78, 0xbf541b),
    ),
    ("red", "Red", f(0xb01e2b, 0xe8636d, 0x8c1621)),
    ("pink", "Pink", f(0xedc3c9, 0xfdeaec, 0xd6a5ad)),
    ("yellow", "Yellow", f(0xefd468, 0xfff1ad, 0xd3b74c)),
    ("sage", "Sage", f(0x6b8468, 0xa9c0a6, 0x566c54)),
    ("deep-blue", "Deep Blue", f(0x2d3f5e, 0x7186ad, 0x22314a)),
    (
        "deep-purple",
        "Deep Purple",
        f(0x564773, 0x9a89bf, 0x43375c),
    ),
];

const fn f(band: u32, shine: u32, buttons: u32) -> Finish {
    Finish {
        band,
        shine,
        buttons,
    }
}

/// The default finish.
pub const DEFAULT: &str = "graphite";

/// A saved finish: one of [`FINISHES`] by id, or a custom colour "#rrggbb".
/// Anything else falls back to the default.
pub fn finish(id: &str) -> Finish {
    if let Some(rgb) = parse_hex(id) {
        return custom(rgb);
    }
    FINISHES
        .iter()
        .find(|(k, _, _)| *k == id)
        .map(|(_, _, c)| *c)
        .unwrap_or(FINISHES[0].2)
}

/// Whether `id` is something [`finish`] knows (kept when read from a file).
pub fn valid(id: &str) -> bool {
    parse_hex(id).is_some() || FINISHES.iter().any(|(k, _, _)| *k == id)
}

fn parse_hex(s: &str) -> Option<u32> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(h, 16).ok()
}

/// A custom colour: the band is the colour itself; the shine is it mixed
/// 45% toward white and the buttons 20% toward black, like the presets.
pub fn custom(rgb: u32) -> Finish {
    Finish {
        band: rgb,
        shine: mix(rgb, 0xffffff, 0.45),
        buttons: mix(rgb, 0x000000, 0.2),
    }
}

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |c: u32, s: u32| ((c >> s) & 0xff) as f32;
    let one = |s: u32| ((ch(a, s) + (ch(b, s) - ch(a, s)) * t).round() as u32).min(255) << s;
    one(16) | one(8) | one(0)
}

/// "#rrggbb" for a colour picked from the hue slider (0..360°).
pub fn custom_from_hue(hue: f32) -> String {
    // Saturation and value close to the coloured finishes.
    let (s, v) = (0.55, 0.72);
    let h = hue.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let to = |u: f32| ((u + m) * 255.0).round().clamp(0.0, 255.0) as u32;
    format!("#{:02x}{:02x}{:02x}", to(r), to(g), to(b))
}

/// Where the frame and the screen go inside a box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// The whole frame, buttons included: x, y, width, height.
    pub outer: (f32, f32, f32, f32),
    /// The screen (the picture): x, y, width, height.
    pub screen: (f32, f32, f32, f32),
    /// Screen corner radius.
    pub radius: f32,
    /// One reference point (the frame's widths are multiples of it).
    pub unit: f32,
    /// Wider than high: the buttons are on the top and bottom edges.
    pub landscape: bool,
}

/// Extra size the frame adds around a picture of `w` × `h` (any unit):
/// width and height of the whole frame.
pub fn outer_size(w: f32, h: f32) -> (f32, f32) {
    let unit = w.min(h) / REF;
    let border = (BLACK + BAND) * unit;
    let button = BUTTON * unit;
    let landscape = w > h;
    (
        w + 2.0 * border + if landscape { 0.0 } else { 2.0 * button },
        h + 2.0 * border + if landscape { 2.0 * button } else { 0.0 },
    )
}

/// The largest frame with a picture of aspect `aspect` (width / height)
/// that fits in the box `bx, by, bw, bh`, centred.
pub fn fit(bx: f32, by: f32, bw: f32, bh: f32, aspect: f32) -> Layout {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        9.0 / 19.5
    };
    let landscape = aspect > 1.0;
    let border = (BLACK + BAND) / REF;
    let side = (BLACK + BAND + BUTTON) / REF;
    // Screen size from the short side `s` (outer sizes are linear in it).
    let s = if landscape {
        // w = s·aspect, h = s
        (bw / (aspect + 2.0 * border)).min(bh / (1.0 + 2.0 * side))
    } else {
        // w = s, h = s / aspect
        (bw / (1.0 + 2.0 * side)).min(bh / (1.0 / aspect + 2.0 * border))
    }
    .max(1.0);
    let (sw, sh) = if landscape {
        (s * aspect, s)
    } else {
        (s, s / aspect)
    };
    let (ow, oh) = outer_size(sw, sh);
    let ox = bx + (bw - ow) / 2.0;
    let oy = by + (bh - oh) / 2.0;
    let unit = s / REF;
    let border_px = (BLACK + BAND) * unit;
    let button_px = BUTTON * unit;
    let (sx, sy) = if landscape {
        (ox + border_px, oy + button_px + border_px)
    } else {
        (ox + button_px + border_px, oy + border_px)
    };
    Layout {
        outer: (ox, oy, ow, oh),
        screen: (sx, sy, sw, sh),
        radius: s * CORNER,
        unit,
        landscape,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_screen_has_the_reference_proportions() {
        // A 402 × 874 screen in a box that fits it exactly.
        let (ow, oh) = outer_size(402.0, 874.0);
        assert_eq!((ow, oh), (402.0 + 31.0 + 6.0, 874.0 + 31.0));
        let l = fit(0.0, 0.0, ow, oh, 402.0 / 874.0);
        assert!((l.screen.2 - 402.0).abs() < 0.01, "{l:?}");
        assert!((l.screen.3 - 874.0).abs() < 0.01, "{l:?}");
        assert!((l.radius - 402.0 * 0.137).abs() < 0.01);
        assert!((l.unit - 1.0).abs() < 1e-4);
        assert!((l.screen.0 - (3.0 + 15.5)).abs() < 0.01);
        assert!((l.screen.1 - 15.5).abs() < 0.01);
    }

    #[test]
    fn the_frame_always_fits_its_box_and_is_centred() {
        for (bw, bh) in [(300.0, 900.0), (900.0, 300.0), (500.0, 500.0), (1.0, 1.0)] {
            for aspect in [9.0 / 19.5, 19.5 / 9.0, 0.75, 1.0] {
                let l = fit(10.0, 20.0, bw, bh, aspect);
                let (ox, oy, ow, oh) = l.outer;
                assert!(
                    ow <= bw + 0.01 && oh <= bh + 0.01 || bw < 2.0,
                    "{bw}x{bh} {aspect} {l:?}"
                );
                assert!((ox - 10.0 - (bw - ow) / 2.0).abs() < 0.01);
                assert!((oy - 20.0 - (bh - oh) / 2.0).abs() < 0.01);
                // The screen keeps the picture's aspect ratio.
                let (_, _, sw, sh) = l.screen;
                assert!((sw / sh - aspect).abs() < 1e-3, "{aspect} {l:?}");
                assert_eq!(l.landscape, aspect > 1.0);
            }
        }
    }

    #[test]
    fn landscape_puts_the_buttons_on_the_long_edges() {
        let (ow, oh) = outer_size(874.0, 402.0);
        assert_eq!((ow, oh), (874.0 + 31.0, 402.0 + 31.0 + 6.0));
        let l = fit(0.0, 0.0, ow, oh, 874.0 / 402.0);
        assert!((l.screen.0 - 15.5).abs() < 0.01);
        assert!((l.screen.1 - (3.0 + 15.5)).abs() < 0.01);
    }

    #[test]
    fn finishes_and_custom_colours() {
        assert_eq!(finish("graphite").band, 0x4a4a50);
        assert_eq!(finish("deep-purple").buttons, 0x43375c);
        assert_eq!(finish("nonsense"), FINISHES[0].2);
        assert_eq!(finish("#ff0000").band, 0xff0000);
        assert!(valid("#00ff7f") && valid("gold") && !valid("#12345") && !valid("blue"));
        let c = custom(0x336699);
        assert!(c.shine > c.band && c.buttons < c.band);
        assert_eq!(custom_from_hue(0.0), "#b85353");
        assert_eq!(custom_from_hue(360.0), custom_from_hue(0.0));
        assert!(valid(&custom_from_hue(200.0)));
        // Ids are unique.
        for (i, a) in FINISHES.iter().enumerate() {
            assert!(FINISHES[i + 1..].iter().all(|b| b.0 != a.0));
        }
    }
}
