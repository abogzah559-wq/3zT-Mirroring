//! Fragmented MP4 writer for recordings: the stream's own H.264 / HEVC
//! samples (already length-prefixed, with their avcC / hvcC record) and AAC
//! audio, written as they come without re-encoding. Every fragment is
//! complete on its own, so a file cut short (unplugged, app closed) still
//! plays up to its last fragment.

use std::io::{self, Write};

pub const VIDEO_TIMESCALE: u32 = 90_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
}

/// AAC-LC audio track (raw AAC frames of 1024 samples).
#[derive(Clone, Debug)]
pub struct AudioTrack {
    pub rate: u32,
    pub channels: u16,
    /// AudioSpecificConfig.
    pub asc: Vec<u8>,
}

struct Sample {
    data: Vec<u8>,
    duration: u32,
    key: bool,
}

pub struct Mp4Writer<W: Write> {
    out: W,
    has_audio: bool,
    seq: u32,
    video: Vec<Sample>,
    audio: Vec<Sample>,
    video_time: u64,
    audio_time: u64,
}

/// A box: size, type, body.
fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(body.len() + 8);
    v.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

/// A full box (version + flags).
fn fbx(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(body.len() + 4);
    b.push(version);
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    bx(kind, &b)
}

fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

const MATRIX: [u32; 9] = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];

fn u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_be_bytes()).collect()
}

fn tkhd(id: u32, audio: bool, w: u32, h: u32) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&u32s(&[0, 0, id, 0, 0])); // times, id, reserved, duration
    b.extend_from_slice(&[0; 8]);
    b.extend_from_slice(&0u16.to_be_bytes()); // layer
    b.extend_from_slice(&0u16.to_be_bytes()); // alternate group
    b.extend_from_slice(&(if audio { 0x0100u16 } else { 0 }).to_be_bytes());
    b.extend_from_slice(&0u16.to_be_bytes());
    b.extend_from_slice(&u32s(&MATRIX));
    b.extend_from_slice(&u32s(&[w << 16, h << 16]));
    fbx(b"tkhd", 0, 3, &b)
}

fn mdhd(timescale: u32) -> Vec<u8> {
    let mut b = u32s(&[0, 0, timescale, 0]);
    b.extend_from_slice(&0x55c4u16.to_be_bytes()); // "und"
    b.extend_from_slice(&0u16.to_be_bytes());
    fbx(b"mdhd", 0, 0, &b)
}

fn hdlr(kind: &[u8; 4], name: &str) -> Vec<u8> {
    let mut b = u32s(&[0]);
    b.extend_from_slice(kind);
    b.extend_from_slice(&[0; 12]);
    b.extend_from_slice(name.as_bytes());
    b.push(0);
    fbx(b"hdlr", 0, 0, &b)
}

fn dinf() -> Vec<u8> {
    let url = fbx(b"url ", 0, 1, &[]);
    let mut dref = u32s(&[1]);
    dref.extend_from_slice(&url);
    bx(b"dinf", &fbx(b"dref", 0, 0, &dref))
}

fn empty_tables() -> Vec<u8> {
    cat(&[
        fbx(b"stts", 0, 0, &u32s(&[0])),
        fbx(b"stsc", 0, 0, &u32s(&[0])),
        fbx(b"stsz", 0, 0, &u32s(&[0, 0])),
        fbx(b"stco", 0, 0, &u32s(&[0])),
    ])
}

fn video_entry(codec: VideoCodec, config: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend_from_slice(&1u16.to_be_bytes()); // data reference index
    b.extend_from_slice(&[0; 16]);
    b.extend_from_slice(&(w as u16).to_be_bytes());
    b.extend_from_slice(&(h as u16).to_be_bytes());
    b.extend_from_slice(&u32s(&[0x0048_0000, 0x0048_0000, 0]));
    b.extend_from_slice(&1u16.to_be_bytes()); // frame count
    b.extend_from_slice(&[0; 32]); // compressor name
    b.extend_from_slice(&0x18u16.to_be_bytes()); // depth
    b.extend_from_slice(&(-1i16).to_be_bytes());
    let (kind, cfg) = match codec {
        VideoCodec::H264 => (b"avc1", b"avcC"),
        VideoCodec::Hevc => (b"hvc1", b"hvcC"),
    };
    b.extend_from_slice(&bx(cfg, config));
    bx(kind, &b)
}

/// MPEG-4 descriptor with its length (4-byte form).
fn desc(tag: u8, body: &[u8]) -> Vec<u8> {
    let n = body.len() as u32;
    let mut v = vec![
        tag,
        0x80 | ((n >> 21) & 0x7f) as u8,
        0x80 | ((n >> 14) & 0x7f) as u8,
        0x80 | ((n >> 7) & 0x7f) as u8,
        (n & 0x7f) as u8,
    ];
    v.extend_from_slice(body);
    v
}

fn audio_entry(a: &AudioTrack) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend_from_slice(&1u16.to_be_bytes());
    b.extend_from_slice(&[0; 8]);
    b.extend_from_slice(&a.channels.to_be_bytes());
    b.extend_from_slice(&16u16.to_be_bytes());
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&(a.rate << 16).to_be_bytes());
    let mut dcd = vec![0x40, 0x15, 0, 0x18, 0]; // AAC, audio stream, buffer 6 KB
    dcd.extend_from_slice(&u32s(&[192_000, 192_000]));
    dcd.extend_from_slice(&desc(0x05, &a.asc));
    let mut es = vec![0, 1, 0]; // ES id 1, no flags
    es.extend_from_slice(&desc(0x04, &dcd));
    es.extend_from_slice(&desc(0x06, &[0x02]));
    b.extend_from_slice(&fbx(b"esds", 0, 0, &desc(0x03, &es)));
    bx(b"mp4a", &b)
}

fn trak(
    id: u32,
    timescale: u32,
    handler: &[u8; 4],
    header: Vec<u8>,
    entry: Vec<u8>,
    w: u32,
    h: u32,
) -> Vec<u8> {
    let audio = handler == b"soun";
    let mut stsd = u32s(&[1]);
    stsd.extend_from_slice(&entry);
    let stbl = bx(b"stbl", &cat(&[fbx(b"stsd", 0, 0, &stsd), empty_tables()]));
    let minf = bx(b"minf", &cat(&[header, dinf(), stbl]));
    let mdia = bx(
        b"mdia",
        &cat(&[
            mdhd(timescale),
            hdlr(handler, if audio { "3zT Sound" } else { "3zT Video" }),
            minf,
        ]),
    );
    bx(b"trak", &cat(&[tkhd(id, audio, w, h), mdia]))
}

fn trex(id: u32) -> Vec<u8> {
    fbx(b"trex", 0, 0, &u32s(&[id, 1, 0, 0, 0]))
}

impl<W: Write> Mp4Writer<W> {
    /// Writes the header. `config`: the avcC / hvcC record; `w`, `h`: the
    /// picture size.
    pub fn new(
        mut out: W,
        codec: VideoCodec,
        config: &[u8],
        w: u32,
        h: u32,
        audio: Option<&AudioTrack>,
    ) -> io::Result<Self> {
        let brand = match codec {
            VideoCodec::H264 => b"avc1",
            VideoCodec::Hevc => b"hvc1",
        };
        let mut ftyp = b"isom".to_vec();
        ftyp.extend_from_slice(&0x200u32.to_be_bytes());
        for b in [b"isom", b"iso5", b"iso6", b"mp41", brand] {
            ftyp.extend_from_slice(b);
        }
        let mut mvhd = u32s(&[0, 0, 1000, 0, 0x10000]);
        mvhd.extend_from_slice(&0x0100u16.to_be_bytes());
        mvhd.extend_from_slice(&[0; 10]);
        mvhd.extend_from_slice(&u32s(&MATRIX));
        mvhd.extend_from_slice(&[0; 24]);
        mvhd.extend_from_slice(&u32s(&[if audio.is_some() { 3 } else { 2 }]));
        let vmhd = fbx(b"vmhd", 0, 1, &[0; 8]);
        let mut parts = vec![
            fbx(b"mvhd", 0, 0, &mvhd),
            trak(
                1,
                VIDEO_TIMESCALE,
                b"vide",
                vmhd,
                video_entry(codec, config, w, h),
                w,
                h,
            ),
        ];
        let mut mvex = trex(1);
        if let Some(a) = audio {
            let smhd = fbx(b"smhd", 0, 0, &[0; 4]);
            parts.push(trak(2, a.rate, b"soun", smhd, audio_entry(a), 0, 0));
            mvex.extend_from_slice(&trex(2));
        }
        parts.push(bx(b"mvex", &mvex));
        out.write_all(&bx(b"ftyp", &ftyp))?;
        out.write_all(&bx(b"moov", &cat(&parts)))?;
        out.flush()?;
        Ok(Self {
            out,
            has_audio: audio.is_some(),
            seq: 0,
            video: Vec::new(),
            audio: Vec::new(),
            video_time: 0,
            audio_time: 0,
        })
    }

    /// A video sample (length-prefixed NAL units) shown for `duration`
    /// ticks of 1/90000 s.
    pub fn video(&mut self, data: Vec<u8>, duration: u32, key: bool) {
        self.video.push(Sample {
            data,
            duration,
            key,
        });
    }

    /// One raw AAC frame (1024 samples).
    pub fn audio(&mut self, data: Vec<u8>) {
        if self.has_audio {
            self.audio.push(Sample {
                data,
                duration: 1024,
                key: true,
            });
        }
    }

    /// Video samples waiting for the next fragment.
    pub fn pending_video(&self) -> usize {
        self.video.len()
    }

    /// Writes what is waiting as one fragment (moof + mdat).
    pub fn flush(&mut self) -> io::Result<()> {
        if self.video.is_empty() && self.audio.is_empty() {
            return Ok(());
        }
        self.seq += 1;
        let tracks: Vec<(u32, &Vec<Sample>, u64, bool)> = [
            (1, &self.video, self.video_time, true),
            (2, &self.audio, self.audio_time, false),
        ]
        .into_iter()
        .filter(|t| !t.1.is_empty())
        .collect();
        // Build twice: the first pass measures the moof for data offsets.
        let build = |offsets: &[u32]| -> Vec<u8> {
            let mut trafs = Vec::new();
            for (i, (id, samples, base, video)) in tracks.iter().enumerate() {
                let tfhd = fbx(b"tfhd", 0, 0x02_0000, &u32s(&[*id]));
                let tfdt = fbx(b"tfdt", 1, 0, &base.to_be_bytes());
                let flags = 0x1 | 0x100 | 0x200 | if *video { 0x400 } else { 0 };
                let mut trun = u32s(&[samples.len() as u32, offsets.get(i).copied().unwrap_or(0)]);
                for s in samples.iter() {
                    trun.extend_from_slice(&s.duration.to_be_bytes());
                    trun.extend_from_slice(&(s.data.len() as u32).to_be_bytes());
                    if *video {
                        let f: u32 = if s.key { 0x0200_0000 } else { 0x0101_0000 };
                        trun.extend_from_slice(&f.to_be_bytes());
                    }
                }
                trafs.push(bx(
                    b"traf",
                    &cat(&[tfhd, tfdt, fbx(b"trun", 0, flags, &trun)]),
                ));
            }
            let mut body = fbx(b"mfhd", 0, 0, &u32s(&[self.seq]));
            for t in trafs {
                body.extend_from_slice(&t);
            }
            bx(b"moof", &body)
        };
        let moof_len = build(&[]).len() as u32;
        let mut offsets = Vec::new();
        let mut at = moof_len + 8;
        for (_, samples, _, _) in &tracks {
            offsets.push(at);
            at += samples.iter().map(|s| s.data.len() as u32).sum::<u32>();
        }
        let moof = build(&offsets);
        let mut mdat_len = 8u32;
        for (_, samples, _, _) in &tracks {
            mdat_len += samples.iter().map(|s| s.data.len() as u32).sum::<u32>();
        }
        self.out.write_all(&moof)?;
        self.out.write_all(&mdat_len.to_be_bytes())?;
        self.out.write_all(b"mdat")?;
        for (_, samples, _, _) in &tracks {
            for s in samples.iter() {
                self.out.write_all(&s.data)?;
            }
        }
        self.out.flush()?;
        self.video_time += self.video.iter().map(|s| s.duration as u64).sum::<u64>();
        self.audio_time += self.audio.iter().map(|s| s.duration as u64).sum::<u64>();
        self.video.clear();
        self.audio.clear();
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        self.flush()?;
        Ok(self.out)
    }
}

/// AudioSpecificConfig for AAC-LC.
pub fn aac_lc_config(rate: u32, channels: u16) -> Vec<u8> {
    const RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    let idx = RATES.iter().position(|r| *r == rate).unwrap_or(3) as u16;
    let v: u16 = (2 << 11) | (idx << 7) | ((channels & 0xf) << 3);
    v.to_be_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asc_for_48k_stereo() {
        assert_eq!(aac_lc_config(48000, 2), vec![0x11, 0x90]);
        assert_eq!(aac_lc_config(44100, 2), vec![0x12, 0x10]);
    }

    #[test]
    fn header_and_fragments_are_well_formed_boxes() {
        let a = AudioTrack {
            rate: 48000,
            channels: 2,
            asc: aac_lc_config(48000, 2),
        };
        let mut w =
            Mp4Writer::new(Vec::new(), VideoCodec::H264, &[1, 2, 3], 640, 360, Some(&a)).unwrap();
        w.video(vec![0, 0, 0, 1, 0x65], 3000, true);
        w.video(vec![0, 0, 0, 1, 0x41], 3000, false);
        w.audio(vec![1, 2, 3, 4]);
        w.flush().unwrap();
        let bytes = w.finish().unwrap();
        // Walk the top-level boxes: ftyp, moov, moof, mdat, sizes adding up.
        let mut at = 0;
        let mut kinds = Vec::new();
        while at < bytes.len() {
            let n = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            kinds.push(String::from_utf8_lossy(&bytes[at + 4..at + 8]).to_string());
            assert!(n >= 8);
            at += n;
        }
        assert_eq!(at, bytes.len());
        assert_eq!(kinds, ["ftyp", "moov", "moof", "mdat"]);
    }
}
