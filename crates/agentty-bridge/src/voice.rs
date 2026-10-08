//! On-device speech-to-text for voice prompts, using a local whisper.cpp model.
//!
//! Nothing audio ever leaves the machine: the model file is downloaded once (pinned by SHA-256)
//! into the data folder, and transcription runs in-process through `whisper-rs`. The phone's
//! browser records a short clip and uploads it over the already-authenticated remote channel; the
//! Mac transcribes it and sends the text back for the person to review and send.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

use crate::fsutil::data_dir;

/// A whisper model the setup flow can offer. Kept small and pinned: the download is verified
/// against `sha256` before it is trusted, so a tampered or truncated file is never loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    /// Stable id used in settings and the API (`tiny`, `base`).
    pub id: &'static str,
    /// File name as stored in the data folder and served by Hugging Face.
    pub file: &'static str,
    /// Where the pinned file is fetched from (TLS, verified by SHA-256 afterwards).
    pub url: &'static str,
    /// Lowercase hex SHA-256 of the exact file at `url`.
    pub sha256: &'static str,
    /// Approximate download size, for the setup screen.
    pub bytes: u64,
}

/// The models the setup flow offers, smallest first. `base` is the default: noticeably better than
/// `tiny` for non-English (e.g. Korean) while still a single modest file.
pub const MODELS: &[Model] = &[
    Model {
        id: "tiny",
        file: "ggml-tiny.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        bytes: 77_691_713,
    },
    Model {
        id: "base",
        file: "ggml-base.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        bytes: 147_951_465,
    },
];

/// The default model id when none is chosen.
pub const DEFAULT_MODEL: &str = "base";

/// Longest clip accepted, in seconds — a voice prompt, not a recording session. At 16 kHz mono
/// f32 this also bounds memory and transcription time.
#[cfg(feature = "voice")]
const MAX_SECONDS: usize = 120;
/// Sample rate whisper expects.
#[cfg(feature = "voice")]
const SAMPLE_RATE: u32 = 16_000;

pub fn model(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.id == id)
}

/// `~/.agentty/voice` (or the `AGENTTY_DATA_DIR` equivalent), created on demand.
fn voice_dir() -> PathBuf {
    data_dir().join("voice")
}

/// Where a model's verified file lives once installed.
pub fn model_path(m: &Model) -> PathBuf {
    voice_dir().join(m.file)
}

/// Whether a model is installed and its file still matches the pinned hash. The hash check guards
/// against a partial download left behind by a crash; it reads the file, so callers that only need
/// a cheap "probably there" can check [`model_present`] instead.
pub fn model_ready(m: &Model) -> bool {
    let path = model_path(m);
    matches!(sha256_file(&path), Ok(h) if h == m.sha256)
}

/// Cheap existence-and-size check for UI that must not block on hashing a large file.
pub fn model_present(m: &Model) -> bool {
    std::fs::metadata(model_path(m)).map(|meta| meta.len() == m.bytes).unwrap_or(false)
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Download a model to a temp file next to its final path, verifying the pinned SHA-256 before
/// moving it into place. `progress` is called with (downloaded, total) bytes as it streams, so the
/// setup screen can show a bar. A download that fails verification is deleted, never loaded.
pub fn download_model(m: &Model, mut progress: impl FnMut(u64, u64)) -> Result<()> {
    let dir = voice_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let final_path = model_path(m);
    if model_ready(m) {
        return Ok(());
    }
    let tmp = dir.join(format!("{}.part", m.file));
    let _ = std::fs::remove_file(&tmp);

    let agent = crate::http::agent_builder().timeout(Duration::from_secs(600)).build();
    let response = agent.get(m.url).set("User-Agent", "Agentty").call().with_context(|| format!("download {}", m.url))?;
    let total: u64 = response.header("content-length").and_then(|v| v.parse().ok()).unwrap_or(m.bytes);

    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 128 * 1024];
    let mut done: u64 = 0;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        use std::io::Write;
        file.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        done += n as u64;
        progress(done, total);
    }
    file.sync_all().ok();
    drop(file);

    let got = hex(&hasher.finalize());
    if got != m.sha256 {
        let _ = std::fs::remove_file(&tmp);
        bail!("downloaded {} failed checksum (got {got})", m.file);
    }
    std::fs::rename(&tmp, &final_path).with_context(|| format!("install {}", final_path.display()))?;
    Ok(())
}

/// A loaded whisper model, kept alive so repeated prompts do not reload the file each time. Loading
/// is the slow part; transcription of a short clip is quick once it is in memory. The context is an
/// `Arc` so a transcription can clone it and run with the cache lock released — otherwise every
/// request would serialize behind one lock held for the whole (slow) transcription.
#[cfg(feature = "voice")]
struct Loaded {
    path: PathBuf,
    ctx: std::sync::Arc<whisper_rs::WhisperContext>,
}

#[cfg(feature = "voice")]
static LOADED: std::sync::Mutex<Option<Loaded>> = std::sync::Mutex::new(None);

/// Transcribe a mono clip of 16 kHz f32 samples to text. `lang` is an ISO code (`ko`, `en`) or
/// `None` to let whisper detect it. The model must already be installed.
#[cfg(feature = "voice")]
pub fn transcribe(model: &Model, samples: &[f32], lang: Option<&str>) -> Result<String> {
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    if samples.is_empty() {
        return Ok(String::new());
    }
    let path = model_path(model);
    if !model_present(model) {
        bail!("voice model {} is not installed", model.id);
    }

    // Load (or reload, if the model changed) under the lock, then clone the `Arc` and release it,
    // so the slow transcription below runs without blocking other requests.
    let ctx = {
        let mut guard = LOADED.lock().unwrap_or_else(|e| e.into_inner());
        if guard.as_ref().map(|l| l.path != path).unwrap_or(true) {
            let ctx = WhisperContext::new_with_params(&path.to_string_lossy() as &str, WhisperContextParameters::default())
                .map_err(|e| anyhow::anyhow!("load voice model: {e}"))?;
            *guard = Some(Loaded { path: path.clone(), ctx: std::sync::Arc::new(ctx) });
        }
        guard.as_ref().expect("just set").ctx.clone()
    };

    let mut state = ctx.create_state().map_err(|e| anyhow::anyhow!("voice state: {e}"))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    if let Some(l) = lang {
        params.set_language(Some(l));
    }
    params.set_translate(false);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    state.full(params, samples).map_err(|e| anyhow::anyhow!("transcribe: {e}"))?;

    let n = state.full_n_segments();
    let mut out = String::new();
    for i in 0..n {
        if let Some(seg) = state.get_segment(i) {
            if let Ok(text) = seg.to_str_lossy() {
                out.push_str(&text);
            }
        }
    }
    Ok(out.trim().to_string())
}

#[cfg(not(feature = "voice"))]
pub fn transcribe(_model: &Model, _samples: &[f32], _lang: Option<&str>) -> Result<String> {
    bail!("this build has no voice support")
}

/// Decode a WAV clip (as uploaded by the browser) into mono 16 kHz f32 samples whisper can take.
/// Accepts 16-bit PCM or 32-bit float, any channel count, and resamples to 16 kHz if needed. Caps
/// the length so an oversized upload cannot exhaust memory.
#[cfg(feature = "voice")]
pub fn wav_to_samples(bytes: &[u8]) -> Result<Vec<f32>> {
    use hound::{SampleFormat, WavReader};

    let mut reader = WavReader::new(std::io::Cursor::new(bytes)).context("read wav")?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let max_in = MAX_SECONDS * spec.sample_rate.max(1) as usize * channels;

    let interleaved: Vec<f32> = match spec.sample_format {
        SampleFormat::Float => reader.samples::<f32>().take(max_in).collect::<Result<_, _>>()?,
        SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().take(max_in).map(|s| s.map(|v| v as f32 * scale)).collect::<Result<_, _>>()?
        }
    };

    // Downmix to mono.
    let mono: Vec<f32> =
        if channels <= 1 { interleaved } else { interleaved.chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect() };

    Ok(resample_to_16k(&mono, spec.sample_rate))
}

#[cfg(not(feature = "voice"))]
pub fn wav_to_samples(_bytes: &[u8]) -> Result<Vec<f32>> {
    bail!("this build has no voice support")
}

/// Linear resample to 16 kHz. Clips come from the browser already at 16 kHz, so this is usually a
/// no-op; it only runs if a client sends another rate.
#[cfg(feature = "voice")]
fn resample_to_16k(input: &[f32], rate: u32) -> Vec<f32> {
    if rate == SAMPLE_RATE || input.is_empty() {
        return input.to_vec();
    }
    let ratio = SAMPLE_RATE as f64 / rate as f64;
    let out_len = (input.len() as f64 * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f64 / ratio;
        let left = src.floor() as usize;
        let frac = (src - left as f64) as f32;
        let a = input.get(left).copied().unwrap_or(0.0);
        let b = input.get(left + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * frac);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_are_well_formed() {
        assert!(model(DEFAULT_MODEL).is_some(), "default model must be in the registry");
        for m in MODELS {
            assert_eq!(m.sha256.len(), 64, "{} sha256 is not 64 hex chars", m.id);
            assert!(m.sha256.bytes().all(|b| b.is_ascii_hexdigit()), "{} sha256 not hex", m.id);
            assert!(m.url.starts_with("https://"), "{} url must be https", m.id);
            assert!(m.bytes > 0, "{} size must be known", m.id);
        }
        for (i, a) in MODELS.iter().enumerate() {
            for b in &MODELS[i + 1..] {
                assert_ne!(a.id, b.id, "duplicate model id {}", a.id);
            }
        }
    }

    #[cfg(feature = "voice")]
    #[test]
    fn wav_decodes_to_16k_mono() {
        // A 0.5 s 16 kHz mono 16-bit PCM clip, round-tripped through the decoder.
        let spec = hound::WavSpec { channels: 1, sample_rate: 16_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = hound::WavWriter::new(&mut buf, spec).unwrap();
            for i in 0..8_000 {
                let v = ((i as f32 * 0.05).sin() * 10_000.0) as i16;
                w.write_sample(v).unwrap();
            }
            w.finalize().unwrap();
        }
        let samples = wav_to_samples(buf.get_ref()).unwrap();
        assert_eq!(samples.len(), 8_000);
        assert!(samples.iter().all(|s| s.abs() <= 1.0));
    }

    #[cfg(feature = "voice")]
    #[test]
    fn stereo_44k_is_downmixed_and_resampled() {
        let spec = hound::WavSpec { channels: 2, sample_rate: 44_100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = hound::WavWriter::new(&mut buf, spec).unwrap();
            for _ in 0..44_100 {
                w.write_sample(1_000i16).unwrap();
                w.write_sample(3_000i16).unwrap();
            }
            w.finalize().unwrap();
        }
        let samples = wav_to_samples(buf.get_ref()).unwrap();
        // 1 s at 44.1 kHz resampled to 16 kHz.
        assert!((samples.len() as i64 - 16_000).abs() <= 2, "got {}", samples.len());
    }
}
