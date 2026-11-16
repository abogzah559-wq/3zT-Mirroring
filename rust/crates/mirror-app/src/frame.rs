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

/// The twelve finishes: id, English name, colours. The colours follow
/// Apple's own finishes (polished stainless steel on the Pro models,
/// anodised aluminium on the others), bright like on the real phone.
/// "white" is kept for frames saved before (it shows as Silver).
pub const FINISHES: [(&str, &str, Finish); 12] = [
    ("graphite", "Graphite", f(0x5b5a57, 0xb4b2ad, 0x4a4946)),
    ("silver", "Silver", f(0xdcdde0, 0xffffff, 0xbfc1c5)),
    ("gold", "Gold", f(0xe8c48f, 0xfff0d2, 0xc9a26b)),
    (
        "pacific-blue",
        "Pacific Blue",
        f(0x2f5d78, 0x86b4cc, 0x234a61),
    ),
    (
        "space-black",
        "Space Black",
        f(0x3a3937, 0x8d8b88, 0x2b2a28),
    ),
    (
        "deep-purple",
        "Deep Purple",
        f(0x5f4b75, 0xa892c2, 0x4a3a5c),
    ),
    ("deep-blue", "Deep Blue", f(0x2c4370, 0x7d97c9, 0x213459)),
    (
        "cosmic-orange",
        "Cosmic Orange",
        f(0xf26b1d, 0xffb27a, 0xcf5510),
    ),
    ("red", "(PRODUCT)RED", f(0xd31f33, 0xff6b78, 0xa8141f)),
    ("pink", "Pink", f(0xf3a6cc, 0xffe0f0, 0xd987b0)),
    ("yellow", "Yellow", f(0xf6d84a, 0xfff3a6, 0xd8b92c)),
    ("sage", "Sage", f(0x9cb38c, 0xd6e5c9, 0x7f976f)),
];

/// Ids saved by older versions, and what they show now.
const OLD_IDS: [(&str, &str); 1] = [("white", "silver")];

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
    let id = canonical(id);
    FINISHES
        .iter()
        .find(|(k, _, _)| *k == id)
        .map(|(_, _, c)| *c)
        .unwrap_or(FINISHES[0].2)
}

/// A finish id as it is called now (older ids are renamed).
pub fn canonical(id: &str) -> &str {
    OLD_IDS
        .iter()
        .find(|(old, _)| *old == id)
        .map_or(id, |(_, new)| new)
}

/// Whether `id` is something [`finish`] knows (kept when read from a file).
pub fn valid(id: &str) -> bool {
    parse_hex(id).is_some()
        || FINISHES.iter().any(|(k, _, _)| *k == id)
        || OLD_IDS.iter().any(|(k, _)| *k == id)
}

fn parse_hex(s: &str) -> Option<u32> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(h, 16).ok()
}

/// A custom colour: the band is the colour itself; the shine is it mixed
/// 50% toward white and the buttons 18% toward black, like the presets.
pub fn custom(rgb: u32) -> Finish {
    Finish {
        band: rgb,
        shine: mix(rgb, 0xffffff, 0.5),
        buttons: mix(rgb, 0x000000, 0.18),
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
    let (s, v) = (0.68, 0.86);
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

/// The camera cut-out at the top of the screen, in points of the phone's
/// own screen (from Apple's design resources).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CutSpec {
    /// A pill floating under the top edge (iPhone 14 Pro and later)
    /// rather than a notch hanging from it.
    pub island: bool,
    pub width: f32,
    pub height: f32,
    /// Gap between the screen's top edge and an island.
    pub top: f32,
    /// Round corners (bottom corners of a notch, the ends of an island).
    pub radius: f32,
    /// The screen's width in points.
    pub screen: f32,
}

/// The cut-out of the iPhone shown, from its model name and UDID, or
/// `None` (iPad, iPhone SE / 8, landscape). `aspect` is the picture's
/// width / height.
pub fn cutout(model: &str, udid: &str, aspect: f32) -> Option<CutSpec> {
    // Phones with a cut-out are taller than 9:19 and in portrait; the
    // iPhone SE / 8 (9:16) and iPads have none.
    if !(aspect > 0.0 && aspect < 0.5) {
        return None;
    }
    let n = model.to_ascii_lowercase();
    let has = |k: &str| n.contains(k);
    // The UDID starts with the chip: 8120 (A16) and later have the island,
    // except the iPhone 16e (A18, notch).
    let cpid = udid
        .strip_prefix("0000")
        .and_then(|r| r.get(..4))
        .unwrap_or("");
    let island = if has("16e") {
        false
    } else if matches!(cpid, "8120" | "8130" | "8140" | "8150") {
        true
    } else if matches!(cpid, "8020" | "8030" | "8101" | "8110") {
        false
    } else {
        ["14 pro", "15", "16", "17", "air"].iter().any(|k| has(k))
    };
    // The iPhone 13, 14 and 16e notch is a fifth narrower than before.
    let small_notch = has("16e") || cpid == "8110" || (has("13") || has("14")) && !has("14 pro");
    let screen = screen_points(&n, island);
    Some(if island {
        CutSpec {
            island,
            width: 126.0,
            height: 37.33,
            top: 11.33,
            radius: 18.67,
            screen,
        }
    } else if small_notch {
        CutSpec {
            island,
            width: 162.0,
            height: 33.0,
            top: 0.0,
            radius: 20.0,
            screen,
        }
    } else {
        CutSpec {
            island,
            width: 210.0,
            height: 31.0,
            top: 0.0,
            radius: 20.0,
            screen,
        }
    })
}

/// Screen width in points by model name (a usual size when unknown).
fn screen_points(n: &str, island: bool) -> f32 {
    let has = |k: &str| n.contains(k);
    if has("mini") {
        375.0
    } else if has("air") {
        420.0
    } else if has("16 pro max") || has("17 pro max") {
        440.0
    } else if has("xs max") || has("11 pro max") || has("xr") || n.trim_end() == "iphone 11" {
        414.0
    } else if has("pro max") || has("plus") {
        if island {
            430.0
        } else {
            428.0
        }
    } else if has("16 pro") || has("17") {
        402.0
    } else if has("iphone x") || has("11 pro") {
        375.0
    } else if island {
        393.0
    } else {
        390.0
    }
}

/// The cut-out placed on a screen `(x, y, width, height)`: x, y, width,
/// height and radius in the screen's units.
pub fn place_cutout(spec: &CutSpec, screen: (f32, f32, f32, f32)) -> (f32, f32, f32, f32, f32) {
    let k = screen.2 / spec.screen;
    let (w, h) = (spec.width * k, spec.height * k);
    (
        screen.0 + (screen.2 - w) / 2.0,
        screen.1 + spec.top * k,
        w,
        h,
        spec.radius * k,
    )
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
        assert_eq!(finish("graphite").band, 0x5b5a57);
        assert_eq!(finish("deep-purple").buttons, 0x4a3a5c);
        assert_eq!(finish("white"), finish("silver"));
        assert!(valid("white"));
        assert_eq!(finish("nonsense"), FINISHES[0].2);
        assert_eq!(finish("#ff0000").band, 0xff0000);
        assert!(valid("#00ff7f") && valid("gold") && !valid("#12345") && !valid("blue"));
        let c = custom(0x336699);
        assert!(c.shine > c.band && c.buttons < c.band);
        assert_eq!(custom_from_hue(0.0), "#db4646");
        assert_eq!(custom_from_hue(360.0), custom_from_hue(0.0));
        assert!(valid(&custom_from_hue(200.0)));
        // Ids are unique.
        for (i, a) in FINISHES.iter().enumerate() {
            assert!(FINISHES[i + 1..].iter().all(|b| b.0 != a.0));
        }
    }

    #[test]
    fn cutouts_by_model() {
        let tall = 1284.0 / 2778.0;
        // iPhone 12 Pro Max: wide notch on a 428-point screen.
        let c = cutout("iPhone 12 Pro Max", "00008101-0001415A1E60001E", tall).unwrap();
        assert!(!c.island && c.width == 210.0 && c.screen == 428.0);
        // Unknown name: the chip decides.
        assert!(!cutout("My phone", "00008101-00", tall).unwrap().island);
        assert!(cutout("My phone", "00008120-00", tall).unwrap().island);
        assert!(cutout("iPhone 15 Pro", "", tall).unwrap().island);
        let e = cutout("iPhone 16e", "00008140-00", tall).unwrap();
        assert!(!e.island && e.width == 162.0);
        // No cut-out on 16:9 phones, iPads or in landscape.
        assert!(cutout("iPhone SE", "", 9.0 / 16.0).is_none());
        assert!(cutout("iPhone 12 Pro Max", "", 2778.0 / 1284.0).is_none());
        // Placed centred at the top, scaled with the screen.
        let (x, y, w, h, r) = place_cutout(&c, (10.0, 20.0, 856.0, 1852.0));
        assert!((w - 420.0).abs() < 0.01 && (h - 62.0).abs() < 0.01 && (r - 40.0).abs() < 0.01);
        assert!((x - (10.0 + (856.0 - 420.0) / 2.0)).abs() < 0.01 && y == 20.0);
    }
}
