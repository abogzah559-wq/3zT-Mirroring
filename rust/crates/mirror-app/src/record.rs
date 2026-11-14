//! Recording (iPhone/iPad, Apple Driver / Hybrid): a second reader of the
//! backend's stream writes the device's own H.264 / HEVC samples into an
//! MP4 without re-encoding, so the mirrored picture keeps its pace; the
//! sound (48 kHz PCM from /cast/audio.bin) is encoded to AAC with Windows'
//! own encoder (Media Foundation). Fragments are written about once a
//! second, so a recording cut short (unplugged, app closed) still plays.
//! A new picture configuration (the device turned) ends the part and the
//! next one goes to "name (2).mp4".

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mirror_core::api::Codec;
use mirror_core::stream::{open_raw, Record, StreamReader};

use crate::mp4mux::{aac_lc_config, AudioTrack, Mp4Writer, VideoCodec, VIDEO_TIMESCALE};

/// A running recording.
pub struct Recording {
    stop: Arc<AtomicBool>,
    pub started: Instant,
    pub path: PathBuf,
    /// Set by the writer thread when it ends: files written, error if any.
    pub result: Arc<Mutex<Option<(Vec<PathBuf>, Option<String>)>>>,
}

impl Recording {
    pub fn stop(&self) {
        self.stop.store(true, Relaxed);
    }

    pub fn finished(&self) -> Option<(Vec<PathBuf>, Option<String>)> {
        self.result.lock().ok()?.clone()
    }
}

/// `Videos\3zT Recordings`.
pub fn folder() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join("Videos").join("3zT Recordings")
}

/// Starts recording into a new file in [`folder`]. `size`: the picture's
/// size now (the track header needs it).
pub fn start(size: (u32, u32), with_sound: bool) -> Result<Recording, String> {
    let dir = folder();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = crate::shot::file_name(&dir, &crate::shot::stamp_now(), "mp4");
    let stop = Arc::new(AtomicBool::new(false));
    let result = Arc::new(Mutex::new(None));
    let (s, r, p) = (stop.clone(), result.clone(), path.clone());
    std::thread::Builder::new()
        .name("recording".into())
        .spawn(move || {
            let out = run(&p, size, with_sound, &s);
            if let Ok(mut g) = r.lock() {
                *g = Some(out);
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Recording {
        stop,
        started: Instant::now(),
        path,
        result,
    })
}

/// Sound reader: AAC frames (encoded on this thread) into a shared queue.
fn sound_thread(stop: Arc<AtomicBool>, queue: Arc<Mutex<Vec<Vec<u8>>>>) {
    std::thread::Builder::new()
        .name("recording-sound".into())
        .spawn(move || {
            let mut enc = match aac::Encoder::new(48_000, 2) {
                Ok(e) => e,
                Err(e) => {
                    log::warn!("recording: no AAC encoder: {e}");
                    return;
                }
            };
            let Ok(mut body) = open_raw("/cast/audio.bin") else {
                return;
            };
            let mut head = [0u8; 4];
            while !stop.load(Relaxed) {
                use std::io::Read;
                if body.read_exact(&mut head).is_err() {
                    break;
                }
                let n = u32::from_be_bytes(head) as usize;
                if n > 1 << 20 {
                    break;
                }
                let mut pcm = vec![0u8; n];
                if body.read_exact(&mut pcm).is_err() {
                    break;
                }
                match enc.encode(&pcm) {
                    Ok(frames) => {
                        if let Ok(mut q) = queue.lock() {
                            q.extend(frames);
                        }
                    }
                    Err(e) => {
                        log::warn!("recording: AAC: {e}");
                        break;
                    }
                }
            }
        })
        .ok();
}

fn run(
    first: &std::path::Path,
    size: (u32, u32),
    with_sound: bool,
    stop: &AtomicBool,
) -> (Vec<PathBuf>, Option<String>) {
    let mut files = Vec::new();
    let mut reader = match StreamReader::open() {
        Ok(r) => r,
        Err(e) => return (files, Some(e)),
    };
    let codec = match mirror_core::api::Api::new().codec() {
        Some(c) => c,
        None => return (files, Some("no picture".into())),
    };
    let sound = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let sound_stop = Arc::new(AtomicBool::new(false));
    if with_sound {
        sound_thread(sound_stop.clone(), sound.clone());
    }
    let audio = with_sound.then(|| AudioTrack {
        rate: 48_000,
        channels: 2,
        asc: aac_lc_config(48_000, 2),
    });
    let mut config = codec;
    let mut part = 1;
    let mut path = first.to_path_buf();
    let mut error = None;
    'parts: loop {
        let vc = match config.codec {
            Codec::Hevc => VideoCodec::Hevc,
            _ => VideoCodec::H264,
        };
        let file = match std::fs::File::create(&path) {
            Ok(f) => f,
            Err(e) => {
                error = Some(e.to_string());
                break;
            }
        };
        let mut w = match Mp4Writer::new(
            std::io::BufWriter::new(file),
            vc,
            &config.description,
            size.0,
            size.1,
            audio.as_ref(),
        ) {
            Ok(w) => w,
            Err(e) => {
                error = Some(e.to_string());
                break;
            }
        };
        files.push(path.clone());
        if let Ok(mut q) = sound.lock() {
            q.clear(); // sound from before this part's first picture
        }
        let mut started = false;
        let mut last: Option<(Vec<u8>, bool, Instant)> = None;
        let mut flushed = Instant::now();
        loop {
            if stop.load(Relaxed) {
                break 'parts;
            }
            let Some(rec) = reader.next_record() else {
                error = Some("the picture stopped".into());
                break 'parts;
            };
            match rec {
                Record::Config(c) => {
                    if c.description != config.description {
                        // Turned or resized: this part ends, a new file starts.
                        if let Some((d, k, _)) = last.take() {
                            w.video(d, 3000, k);
                        }
                        if let Err(e) = w.finish() {
                            error = Some(e.to_string());
                            break 'parts;
                        }
                        config = c;
                        part += 1;
                        path = numbered(first, part);
                        continue 'parts;
                    }
                }
                Record::Frame { key, data, .. } => {
                    // Start on a keyframe so the file opens on a picture.
                    if !started && !key {
                        continue;
                    }
                    started = true;
                    let now = Instant::now();
                    if let Some((d, k, at)) = last.take() {
                        let ticks = ((now - at).as_secs_f64() * VIDEO_TIMESCALE as f64).round();
                        w.video(d, ticks.clamp(1.0, VIDEO_TIMESCALE as f64) as u32, k);
                    }
                    last = Some((data, key, now));
                    if flushed.elapsed() >= Duration::from_secs(1) {
                        if let Ok(mut q) = sound.lock() {
                            for f in q.drain(..) {
                                w.audio(f);
                            }
                        }
                        if let Err(e) = w.flush() {
                            error = Some(e.to_string());
                            break 'parts;
                        }
                        flushed = Instant::now();
                    }
                }
            }
        }
    }
    sound_stop.store(true, Relaxed);
    (files, error)
}

fn numbered(first: &std::path::Path, n: u32) -> PathBuf {
    let stem = first
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    first.with_file_name(format!("{stem} ({n}).mp4"))
}

/// AAC-LC encoding with Windows' Media Foundation AAC encoder.
#[cfg(windows)]
mod aac {
    use std::mem::ManuallyDrop;

    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };

    pub struct Encoder {
        mft: IMFTransform,
        out_size: u32,
        rate: u32,
        block: u32,
        time: i64,
    }

    // The encoder is created and used on one thread only.
    unsafe impl Send for Encoder {}

    impl Encoder {
        pub fn new(rate: u32, channels: u32) -> windows::core::Result<Self> {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                MFStartup(MF_VERSION, MFSTARTUP_LITE)?;
                let mft: IMFTransform =
                    CoCreateInstance(&CLSID_AACMFTEncoder, None, CLSCTX_INPROC_SERVER)?;
                let block = channels * 2;
                let input = MFCreateMediaType()?;
                input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
                input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
                input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)?;
                input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block)?;
                input.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * block)?;
                let output = MFCreateMediaType()?;
                output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                output.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
                output.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                output.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
                output.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)?;
                output.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)?; // 192 kbps
                output.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?; // raw AAC frames
                                                               // The AAC encoder wants the output type first; some
                                                               // Windows builds take them the other way round.
                if mft.SetOutputType(0, &output, 0).is_ok() {
                    mft.SetInputType(0, &input, 0)?;
                } else {
                    mft.SetInputType(0, &input, 0)?;
                    mft.SetOutputType(0, &output, 0)?;
                }
                let info = mft.GetOutputStreamInfo(0)?;
                mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
                Ok(Self {
                    mft,
                    out_size: info.cbSize.max(8192),
                    rate,
                    block,
                    time: 0,
                })
            }
        }

        /// Feeds PCM (s16le interleaved); returns the AAC frames ready.
        pub fn encode(&mut self, pcm: &[u8]) -> windows::core::Result<Vec<Vec<u8>>> {
            let mut out = Vec::new();
            if pcm.is_empty() {
                return Ok(out);
            }
            unsafe {
                let buf = MFCreateMemoryBuffer(pcm.len() as u32)?;
                let mut p = std::ptr::null_mut();
                buf.Lock(&mut p, None, None)?;
                std::ptr::copy_nonoverlapping(pcm.as_ptr(), p, pcm.len());
                buf.Unlock()?;
                buf.SetCurrentLength(pcm.len() as u32)?;
                let sample = MFCreateSample()?;
                sample.AddBuffer(&buf)?;
                let frames = pcm.len() as i64 / self.block as i64;
                let dur = frames * 10_000_000 / self.rate as i64;
                sample.SetSampleTime(self.time)?;
                sample.SetSampleDuration(dur)?;
                self.time += dur;
                self.mft.ProcessInput(0, &sample, 0)?;
                loop {
                    let ob = MFCreateMemoryBuffer(self.out_size)?;
                    let os = MFCreateSample()?;
                    os.AddBuffer(&ob)?;
                    let mut data = [MFT_OUTPUT_DATA_BUFFER {
                        dwStreamID: 0,
                        pSample: ManuallyDrop::new(Some(os.clone())),
                        dwStatus: 0,
                        pEvents: ManuallyDrop::new(None),
                    }];
                    let mut status = 0u32;
                    let r = self.mft.ProcessOutput(0, &mut data, &mut status);
                    ManuallyDrop::drop(&mut data[0].pSample);
                    ManuallyDrop::drop(&mut data[0].pEvents);
                    match r {
                        Ok(()) => {
                            let b = os.ConvertToContiguousBuffer()?;
                            let mut q = std::ptr::null_mut();
                            let mut len = 0u32;
                            b.Lock(&mut q, None, Some(&mut len))?;
                            out.push(std::slice::from_raw_parts(q, len as usize).to_vec());
                            b.Unlock()?;
                        }
                        Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => break,
                        Err(e) => return Err(e),
                    }
                }
            }
            Ok(out)
        }
    }
}

#[cfg(not(windows))]
mod aac {
    pub struct Encoder;
    impl Encoder {
        pub fn new(_rate: u32, _channels: u32) -> Result<Self, String> {
            Err("Windows only".into())
        }
        pub fn encode(&mut self, _pcm: &[u8]) -> Result<Vec<Vec<u8>>, String> {
            Ok(Vec::new())
        }
    }
}
