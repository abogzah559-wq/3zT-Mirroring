//! Screenshots: the picture at the stream's own size as PNG in
//! `Pictures\3zT Screenshots`, also copied to the clipboard; optionally
//! inside the iPhone frame (drawn here in software, same proportions as
//! frame.rs).

use std::path::{Path, PathBuf};

use crate::frame::{self, Finish};

/// A BGRA picture, rows top to bottom.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// `Pictures\3zT Screenshots`.
pub fn folder() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join("Pictures").join("3zT Screenshots")
}

/// A free file name with the local date and time.
pub fn file_name(dir: &Path, stamp: &str, ext: &str) -> PathBuf {
    let mut p = dir.join(format!("3zT {stamp}.{ext}"));
    let mut n = 1;
    while p.exists() {
        n += 1;
        p = dir.join(format!("3zT {stamp} ({n}).{ext}"));
    }
    p
}

pub fn date_text(t: i64) -> String {
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

/// Local "YYYY-MM-DD HH-MM-SS" for file names.
pub fn stamp_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let secs = (now + local_offset_secs()).rem_euclid(60);
    format!("{}-{secs:02}", date_text(now)).replace(':', "-")
}

/// Coverage (0..1) of a rounded rectangle at pixel centre (px, py), with one
/// pixel of anti-aliasing.
fn rrect(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let (hx, hy) = (w / 2.0 - r, h / 2.0 - r);
    let dx = ((px - cx).abs() - hx).max(0.0);
    let dy = ((py - cy).abs() - hy).max(0.0);
    let outside = (dx * dx + dy * dy).sqrt() - r;
    let inside_edge = ((px - cx).abs() - w / 2.0).max((py - cy).abs() - h / 2.0);
    let d = if dx > 0.0 && dy > 0.0 {
        outside
    } else {
        inside_edge.max(outside)
    };
    (0.5 - d).clamp(0.0, 1.0)
}

fn rgb(c: u32) -> [f32; 3] {
    [
        ((c >> 16) & 255) as f32,
        ((c >> 8) & 255) as f32,
        (c & 255) as f32,
    ]
}

/// The screenshot inside the iPhone frame, on a transparent background,
/// with the phone's notch or Dynamic Island (`cut`) as in the window.
pub fn framed(p: &Picture, finish: Finish, cut: Option<frame::CutSpec>) -> Picture {
    let (ow, oh) = frame::outer_size(p.width as f32, p.height as f32);
    let l = frame::fit(0.0, 0.0, ow, oh, p.width as f32 / p.height as f32);
    let (w, h) = (ow.ceil() as u32, oh.ceil() as u32);
    let mut out = vec![0u8; (w * h * 4) as usize];
    let u = l.unit;
    let (sx, sy, sw, sh) = l.screen;
    let border = 15.5 * u;
    let (bx, by) = (sx - border, sy - border);
    let (bw, bh) = (sw + 2.0 * border, sh + 2.0 * border);
    let band = rgb(finish.band);
    let shine = rgb(finish.shine);
    let buttons = rgb(finish.buttons);
    let seam: [f32; 3] = std::array::from_fn(|i| buttons[i] * 0.6);
    let cut = cut.map(|c| (c.island, frame::place_cutout(&c, l.screen)));
    // Side buttons, as in the window (PhoneFrame in app.slint).
    let mut keys = Vec::new();
    for (left, at, len) in [
        (true, 0.205, 0.055),
        (true, 0.29, 0.1),
        (true, 0.415, 0.1),
        (false, 0.31, 0.15),
    ] {
        let along_len = if l.landscape { bw } else { bh };
        let (a, s) = (along_len * at, along_len * len);
        let r = if l.landscape {
            let y = if left { by + bh - u } else { 0.0 };
            (bx + bw - a - s, y, s, 4.0 * u)
        } else {
            let x = if left { 0.0 } else { bx + bw - u };
            (x, by + a, 4.0 * u, s)
        };
        keys.push(r);
    }
    for yy in 0..h {
        for xx in 0..w {
            let (px, py) = (xx as f32 + 0.5, yy as f32 + 0.5);
            let mut c = [0.0f32; 3];
            let mut a = 0.0f32;
            let mut over = |col: [f32; 3], cov: f32| {
                if cov <= 0.0 {
                    return;
                }
                for i in 0..3 {
                    c[i] = col[i] * cov + c[i] * (1.0 - cov);
                }
                a = cov + a * (1.0 - cov);
            };
            for &(kx, ky, kw, kh) in &keys {
                over(buttons, rrect(px, py, kx, ky, kw, kh, 1.5 * u));
            }
            // Band with a brighter rim, then the black border.
            let body = rrect(px, py, bx, by, bw, bh, l.radius + border);
            let t = ((px - bx) / bw + (py - by) / bh) / 2.0;
            let rim_mix = (1.0 - (t - 0.5).abs() * 2.0).clamp(0.0, 1.0);
            let rim: [f32; 3] = std::array::from_fn(|i| shine[i] + (band[i] - shine[i]) * rim_mix);
            over(rim, body);
            // Polished band: light sweeping across it near two corners.
            let d = (((px - bx) / bw) * 0.34 + ((py - by) / bh) * 0.94) / 1.28;
            let glint = (1.0 - (d - 0.14).abs() / 0.14)
                .max(1.0 - (d - 0.86).abs() / 0.14)
                .clamp(0.0, 1.0);
            let metal: [f32; 3] = std::array::from_fn(|i| band[i] + (shine[i] - band[i]) * glint);
            over(
                metal,
                rrect(
                    px,
                    py,
                    bx + u,
                    by + u,
                    bw - 2.0 * u,
                    bh - 2.0 * u,
                    l.radius + border - u,
                ),
            );
            over(
                seam,
                rrect(
                    px,
                    py,
                    bx + 5.3 * u,
                    by + 5.3 * u,
                    sw + 20.4 * u,
                    sh + 20.4 * u,
                    l.radius + 10.2 * u,
                ),
            );
            over(
                [0.0; 3],
                rrect(
                    px,
                    py,
                    bx + 6.0 * u,
                    by + 6.0 * u,
                    sw + 19.0 * u,
                    sh + 19.0 * u,
                    l.radius + 9.5 * u,
                ),
            );
            // The picture, with the screen's rounded corners.
            let scov = rrect(px, py, sx, sy, sw, sh, l.radius);
            if scov > 0.0 {
                let ix = ((px - sx) / sw * p.width as f32).clamp(0.0, p.width as f32 - 1.0) as u32;
                let iy =
                    ((py - sy) / sh * p.height as f32).clamp(0.0, p.height as f32 - 1.0) as u32;
                let o = ((iy * p.width + ix) * 4) as usize;
                over(
                    [p.bgra[o + 2] as f32, p.bgra[o + 1] as f32, p.bgra[o] as f32],
                    scov,
                );
            }
            // Notch or Dynamic Island: black glass, earpiece and camera.
            if let Some((island, (cx, cy, cw, ch, cr))) = cut {
                let glass = if island {
                    rrect(px, py, cx, cy, cw, ch, cr)
                } else {
                    rrect(px, py, cx, cy - cr, cw, ch + cr, cr)
                };
                over([0.0; 3], glass.min(scov));
                if glass > 0.0 {
                    if !island {
                        let (gw, gh) = (cw * 0.2, (cw * 0.026).max(2.0));
                        over(
                            [29.0, 29.0, 32.0],
                            rrect(
                                px,
                                py,
                                cx + (cw - gw) / 2.0,
                                cy + ch * 0.2,
                                gw,
                                gh,
                                gh / 2.0,
                            ),
                        );
                    }
                    let dia = if island {
                        ch * 0.44
                    } else {
                        (cw * 0.055).max(4.0)
                    };
                    let (lx, ly) = if island {
                        (cx + cw - ch / 2.0, cy + ch / 2.0)
                    } else {
                        (cx + cw * 0.71, cy + ch * 0.42)
                    };
                    let rr = dia / 2.0;
                    over(
                        [32.0, 40.0, 58.0],
                        rrect(px, py, lx - rr, ly - rr, dia, dia, rr),
                    );
                    let ri = rr * 0.84;
                    over(
                        [14.0, 17.0, 28.0],
                        rrect(px, py, lx - ri, ly - ri, 2.0 * ri, 2.0 * ri, ri),
                    );
                    let rh = dia * 0.11;
                    over(
                        [74.0, 95.0, 154.0],
                        0.55 * rrect(
                            px,
                            py,
                            lx - rr * 0.45 - rh,
                            ly - rr * 0.45 - rh,
                            2.0 * rh,
                            2.0 * rh,
                            rh,
                        ),
                    );
                }
            }
            let o = ((yy * w + xx) * 4) as usize;
            // Straight (not premultiplied) BGRA.
            out[o] = c[2].round() as u8;
            out[o + 1] = c[1].round() as u8;
            out[o + 2] = c[0].round() as u8;
            out[o + 3] = (a * 255.0).round() as u8;
        }
    }
    Picture {
        width: w,
        height: h,
        bgra: out,
    }
}

/// PNG bytes (RGBA, alpha kept for the framed picture's corners).
pub fn png(p: &Picture) -> Result<Vec<u8>, String> {
    let mut rgba = p.bgra.clone();
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, p.width, p.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(&rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Puts the picture on the clipboard as a bitmap (CF_DIB, alpha dropped on
/// black), so it pastes into chats and editors.
#[cfg(windows)]
pub fn to_clipboard(p: &Picture) -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    const CF_DIB: u32 = 8;
    let row = (p.width * 4) as usize;
    let size = 40 + row * p.height as usize;
    unsafe {
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, size) else {
            return false;
        };
        let dst = GlobalLock(mem) as *mut u8;
        if dst.is_null() {
            return false;
        }
        let mut hdr = Vec::with_capacity(40);
        hdr.extend_from_slice(&40u32.to_le_bytes());
        hdr.extend_from_slice(&(p.width as i32).to_le_bytes());
        hdr.extend_from_slice(&(p.height as i32).to_le_bytes()); // bottom-up
        hdr.extend_from_slice(&1u16.to_le_bytes());
        hdr.extend_from_slice(&32u16.to_le_bytes());
        hdr.extend_from_slice(&0u32.to_le_bytes());
        hdr.extend_from_slice(&((row * p.height as usize) as u32).to_le_bytes());
        hdr.extend_from_slice(&[0u8; 16]);
        std::ptr::copy_nonoverlapping(hdr.as_ptr(), dst, 40);
        for y in 0..p.height as usize {
            let src = &p.bgra[y * row..(y + 1) * row];
            let d = dst.add(40 + (p.height as usize - 1 - y) * row);
            for x in 0..p.width as usize {
                let a = src[x * 4 + 3] as u32;
                for i in 0..3 {
                    *d.add(x * 4 + i) = ((src[x * 4 + i] as u32 * a) / 255) as u8;
                }
                *d.add(x * 4 + 3) = 255;
            }
        }
        let _ = GlobalUnlock(mem);
        if OpenClipboard(None).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let ok = SetClipboardData(CF_DIB, Some(HANDLE(mem.0))).is_ok();
        let _ = CloseClipboard();
        ok
    }
}

#[cfg(not(windows))]
pub fn to_clipboard(_p: &Picture) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framed_picture_keeps_the_screen_and_clears_the_corners() {
        let p = Picture {
            width: 40,
            height: 87,
            bgra: [10u8, 20, 30, 255].repeat(40 * 87),
        };
        let f = framed(&p, frame::finish("red"), None);
        let (ow, oh) = frame::outer_size(40.0, 87.0);
        assert_eq!((f.width, f.height), (ow.ceil() as u32, oh.ceil() as u32));
        // Top-left corner: outside the rounded body.
        assert_eq!(f.bgra[3], 0);
        // Centre: the picture itself.
        let c = (((f.height / 2) * f.width + f.width / 2) * 4) as usize;
        assert_eq!(&f.bgra[c..c + 4], &[10, 20, 30, 255]);
        let png = png(&f).unwrap();
        assert_eq!(&png[1..4], b"PNG");
    }

    #[test]
    fn framed_picture_shows_the_notch() {
        let (pw, ph) = (428u32, 926u32);
        let p = Picture {
            width: pw,
            height: ph,
            bgra: [200u8, 200, 200, 255].repeat((pw * ph) as usize),
        };
        let cut = frame::cutout("iPhone 12 Pro Max", "", pw as f32 / ph as f32);
        let f = framed(&p, frame::finish("graphite"), cut);
        let l = frame::fit(
            0.0,
            0.0,
            f.width as f32,
            f.height as f32,
            pw as f32 / ph as f32,
        );
        let (sx, sy, sw, _) = l.screen;
        let at = |x: f32, y: f32| {
            let o = ((y as u32 * f.width + x as u32) * 4) as usize;
            f.bgra[o..o + 4].to_vec()
        };
        // Under the notch, left of the speaker: black glass.
        assert_eq!(at(sx + sw * 0.32, sy + 8.0), vec![0, 0, 0, 255]);
        // Beside the notch: the picture.
        assert_eq!(at(sx + sw * 0.15, sy + 8.0), vec![200, 200, 200, 255]);
        // Below the notch: the picture.
        assert_eq!(at(sx + sw * 0.5, sy + 40.0), vec![200, 200, 200, 255]);
    }
}
