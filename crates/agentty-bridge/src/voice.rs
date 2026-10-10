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
    /// Stable id used in settings and the API (`small`, `large-v3-turbo`).
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

/// The models the setup flow offers, smallest first. `tiny` and `base` were dropped: they get
/// Korean (and English technical words inside Korean sentences) wrong too often to type prompts
/// with. Large-v3 Turbo (5-bit) is the recommended default: close to Large-v3 for about a third of
/// its size, and fast with the GPU (Metal on macOS).
pub const MODELS: &[Model] = &[
    Model {
        id: "small",
        file: "ggml-small.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        bytes: 487_601_967,
    },
    Model {
        id: "large-v3-turbo",
        file: "ggml-large-v3-turbo-q5_0.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        bytes: 574_041_195,
    },
];

/// Files of models earlier versions offered; removed once a supported model is installed.
const RETIRED_FILES: &[&str] = &["ggml-tiny.bin", "ggml-base.bin"];

/// Words a developer says to an agent, given to whisper as its starting context so it spells them
/// the way they are typed (in Korean sentences too) instead of guessing at the sounds.
#[cfg(feature = "voice")]
const VOCABULARY: &str = "Claude, Claude Code, Codex, Agentty, git, commit, push, pull request, PR, merge, \
branch, rebase, worktree, diff, cargo, Rust, npm, TypeScript, React, Python, API, CLI, README, \
refactor, debug, test, build, deploy, CI. git commit하고 PR 올려줘. cargo test 돌리고 refactor해줘.";

/// Whether this build has on-device transcription (the `voice` feature, whisper.cpp) compiled in.
pub const AVAILABLE: bool = cfg!(feature = "voice");

/// Why on-device transcription cannot run on this machine, when it cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// The build has no `voice` feature.
    NotBuilt,
    /// The processor lacks instructions whisper.cpp was compiled with (see [`X86_REQUIRED`]).
    Cpu,
}

/// x86-64 instructions whisper.cpp uses in release packages. The packaging scripts build it with
/// `GGML_NATIVE=OFF`, which keeps ggml's portable x86 baseline: SSE4.2, AVX, AVX2, FMA, F16C and
/// BMI2 (Intel Haswell / AMD Excavator, 2013 on). Running it on a CPU without one of them dies with
/// an illegal instruction, so voice is turned off there instead. A contributor's own build tuned
/// for their machine (`GGML_NATIVE` on) only uses what that machine has.
pub const X86_REQUIRED: &[&str] = &["sse4.2", "avx", "avx2", "fma", "f16c", "bmi2"];

/// `None` when voice can run here, else why not. The CPU is checked once.
pub fn unavailable() -> Option<Unavailable> {
    if !AVAILABLE {
        return Some(Unavailable::NotBuilt);
    }
    static CPU_OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*CPU_OK.get_or_init(|| cpu_supported(cpu_has)) {
        return Some(Unavailable::Cpu);
    }
    None
}

/// Whether on-device transcription is compiled in and can run on this processor.
pub fn supported() -> bool {
    unavailable().is_none()
}

/// Whether a processor with the given features can run the release build of whisper.cpp. Only
/// x86-64 has a check: Arm builds use the target's baseline (Apple M1 on macOS).
fn cpu_supported(has: impl Fn(&str) -> bool) -> bool {
    !cfg!(target_arch = "x86_64") || X86_REQUIRED.iter().all(|feature| has(feature))
}

#[cfg(target_arch = "x86_64")]
fn cpu_has(feature: &str) -> bool {
    match feature {
        "sse4.2" => std::arch::is_x86_feature_detected!("sse4.2"),
        "avx" => std::arch::is_x86_feature_detected!("avx"),
        "avx2" => std::arch::is_x86_feature_detected!("avx2"),
        "fma" => std::arch::is_x86_feature_detected!("fma"),
        "f16c" => std::arch::is_x86_feature_detected!("f16c"),
        "bmi2" => std::arch::is_x86_feature_detected!("bmi2"),
        _ => false,
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn cpu_has(_feature: &str) -> bool {
    true
}

/// The default model id when none is chosen: the recommended one.
pub const DEFAULT_MODEL: &str = "large-v3-turbo";

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

/// The model the user chose as default (Settings → Voice input), if any. The app sets it at start
/// and whenever the choice changes, so every caller of [`installed_model`] follows it.
static PREFERRED: std::sync::Mutex<Option<&'static str>> = std::sync::Mutex::new(None);

/// Use `id` (a [`MODELS`] id) whenever it is installed; `None` or an unknown id goes back to the
/// recommended order.
pub fn set_preferred_model(id: Option<&str>) {
    *PREFERRED.lock().unwrap_or_else(|e| e.into_inner()) = id.and_then(model).map(|m| m.id);
}

/// The model voice input transcribes with: the one the user chose when it is installed, else the
/// recommended one, else any installed one (someone may have set up only `small`). `None` while
/// nothing is installed.
pub fn installed_model() -> Option<&'static Model> {
    let preferred = *PREFERRED.lock().unwrap_or_else(|e| e.into_inner());
    preferred
        .and_then(model)
        .filter(|m| model_present(m))
        .or_else(|| model(DEFAULT_MODEL).filter(|m| model_present(m)))
        .or_else(|| MODELS.iter().find(|m| model_present(m)))
}

/// Deletes an installed model (and a partial download of it). If it is the one loaded in memory it
/// is let go too; a transcription already running keeps its own reference until it ends.
pub fn remove_model(m: &Model) -> Result<()> {
    let path = model_path(m);
    #[cfg(feature = "voice")]
    {
        let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
        if loaded.as_ref().is_some_and(|l| l.path == path) {
            *loaded = None;
        }
    }
    let _ = std::fs::remove_file(voice_dir().join(format!("{}.part", m.file)));
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e).with_context(|| format!("remove {}", path.display())),
        _ => Ok(()),
    }
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

/// Whether the file at `path` hashes to `sha256`, worked out once per process for each file as it
/// is (its size and modification time): a model damaged on disk at the same size would otherwise
/// reach whisper.cpp, whose asserts abort the whole app. Hashing reads the file, so call it off
/// the UI thread; later calls for the unchanged file answer from memory.
fn verified(path: &Path, sha256: &str) -> bool {
    type Stamp = (PathBuf, u64, Option<std::time::SystemTime>);
    static VERDICTS: std::sync::Mutex<Vec<(Stamp, bool)>> = std::sync::Mutex::new(Vec::new());
    let Ok(meta) = std::fs::metadata(path) else { return false };
    let stamp: Stamp = (path.to_path_buf(), meta.len(), meta.modified().ok());
    if let Some((_, ok)) = VERDICTS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(known, _)| *known == stamp) {
        return *ok;
    }
    let ok = matches!(sha256_file(path), Ok(h) if h == sha256);
    let mut verdicts = VERDICTS.lock().unwrap_or_else(|e| e.into_inner());
    verdicts.retain(|((known, _, _), _)| known != path);
    verdicts.push((stamp, ok));
    ok
}

/// Checks a model's file in the background ahead of its first use (a recording started), so the
/// transcription after it doesn't wait on the hash.
pub fn verify_in_background(m: &'static Model) {
    let _ = std::thread::Builder::new().name("voice-verify".into()).spawn(move || {
        verified(&model_path(m), m.sha256);
    });
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
pub fn download_model(m: &Model, progress: impl FnMut(u64, u64)) -> Result<()> {
    // One download at a time across the app (several windows can each offer the setup): a second
    // request waits, then finds the model installed and returns at once.
    static DOWNLOADING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = DOWNLOADING.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = voice_dir().join(format!("{}.part", m.file));
    let result = fetch_model(m, &tmp, progress);
    if result.is_err() {
        // A failed or cut-off download never stays behind.
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn fetch_model(m: &Model, tmp: &Path, mut progress: impl FnMut(u64, u64)) -> Result<()> {
    let dir = voice_dir();
    // The same folder holds voice recordings while they are transcribed: private (0700).
    crate::fsutil::create_private_dir(&dir).with_context(|| format!("create {}", dir.display()))?;
    let final_path = model_path(m);
    if model_ready(m) {
        return Ok(());
    }
    let _ = std::fs::remove_file(tmp);

    // No limit on the whole download (a slow line takes long for 148 MB), only on silence.
    let agent = crate::http::agent_builder().timeout_connect(Duration::from_secs(30)).timeout_read(Duration::from_secs(60)).build();
    let response = agent.get(m.url).set("User-Agent", "Agentty").call().with_context(|| format!("download {}", m.url))?;
    let total: u64 = response.header("content-length").and_then(|v| v.parse().ok()).unwrap_or(m.bytes);

    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(tmp).with_context(|| format!("create {}", tmp.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 128 * 1024];
    let mut done: u64 = 0;
    // A line that trickles a byte a minute never trips the read timeout; give up after this.
    let deadline = std::time::Instant::now() + Duration::from_secs(30 * 60);
    loop {
        if std::time::Instant::now() > deadline {
            bail!("downloading {} took too long", m.file);
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        use std::io::Write;
        file.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        done += n as u64;
        if done > m.bytes {
            bail!("{} is larger than expected", m.file);
        }
        progress(done, total);
    }
    file.sync_all().ok();
    drop(file);

    let got = hex(&hasher.finalize());
    if got != m.sha256 {
        bail!("downloaded {} failed checksum (got {got})", m.file);
    }
    std::fs::rename(tmp, &final_path).with_context(|| format!("install {}", final_path.display()))?;
    // Models earlier versions offered are no longer used: give their space back.
    for retired in RETIRED_FILES {
        let _ = std::fs::remove_file(voice_dir().join(retired));
    }
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
    use whisper_rs::{FullParams, SamplingStrategy};

    if unavailable() == Some(Unavailable::Cpu) {
        bail!("this processor lacks the instructions on-device voice needs ({})", X86_REQUIRED.join(", "));
    }
    // A clip with (almost) no speech in it is not handed to whisper at all: large models invent a
    // sentence for silence ("Thank you.", "감사합니다.") that would otherwise be sent as a prompt.
    if voiced_seconds(samples) < MIN_VOICED_SECONDS {
        return Ok(String::new());
    }
    let ctx = loaded_context(model)?;

    let mut state = ctx.create_state().map_err(|e| anyhow::anyhow!("voice state: {e}"))?;
    // Beam search finds noticeably better wording than taking the likeliest token each step; with
    // the GPU it costs little for a prompt-length clip.
    let mut params = FullParams::new(SamplingStrategy::BeamSearch { beam_size: 5, patience: -1.0 });
    params.set_n_threads(std::thread::available_parallelism().map(|n| n.get().min(8) as i32).unwrap_or(4));
    params.set_initial_prompt(VOCABULARY);
    params.set_suppress_blank(true);
    params.set_suppress_nst(true);
    params.set_no_speech_thold(NO_SPEECH);
    // whisper.cpp's own default is English, which would turn Korean speech into an English
    // translation: no hint means detect the language from the audio.
    params.set_language(Some(lang.unwrap_or("auto")));
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
            // A segment whisper itself rates as probably not speech is left out.
            if seg.no_speech_probability() > NO_SPEECH {
                continue;
            }
            if let Ok(text) = seg.to_str_lossy() {
                out.push_str(&text);
            }
        }
    }
    let text = without_annotations(&out);
    Ok(if is_hallucination(&text) { String::new() } else { text })
}

/// The model, loaded and ready: loaded (or reloaded, if another model was in use) under the lock,
/// whose `Arc` is then cloned so the slow transcription runs without blocking other requests.
#[cfg(feature = "voice")]
fn loaded_context(model: &Model) -> Result<std::sync::Arc<whisper_rs::WhisperContext>> {
    use whisper_rs::{WhisperContext, WhisperContextParameters};

    let path = model_path(model);
    if !model_present(model) {
        bail!("voice model {} is not installed", model.id);
    }
    // Only a file that matches its pinned hash is ever handed to whisper.cpp. A damaged one goes,
    // so the setup offers the download again.
    if !verified(&path, model.sha256) {
        let _ = std::fs::remove_file(&path);
        bail!("voice model {} was damaged and has been removed: download it again", model.id);
    }
    let mut guard = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    // Deleted while this request was on its way (`remove_model` holds this lock): don't load it.
    if !model_present(model) {
        bail!("voice model {} is not installed", model.id);
    }
    if guard.as_ref().map(|l| l.path != path).unwrap_or(true) {
        // The GPU (Metal on macOS) when the build has it: large models need it to answer a prompt
        // in about a second.
        let ctx = WhisperContext::new_with_params(&path.to_string_lossy() as &str, WhisperContextParameters::default())
            .map_err(|e| anyhow::anyhow!("load voice model: {e}"))?;
        // One short pass over silence prepares the GPU pipelines, which the first real prompt
        // would otherwise wait for.
        if let Ok(mut state) = ctx.create_state() {
            let mut params = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
            params.set_language(Some("en"));
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            let _ = state.full(params, &vec![0.0f32; SAMPLE_RATE as usize]);
        }
        *guard = Some(Loaded { path: path.clone(), ctx: std::sync::Arc::new(ctx) });
    }
    Ok(guard.as_ref().expect("just set").ctx.clone())
}

/// Loads the model ahead of a prompt — called when recording starts, so it is ready (and the GPU
/// warmed up) by the time the person stops talking. Slow the first time, nothing afterwards.
#[cfg(feature = "voice")]
pub fn preload(model: &Model) {
    let _ = loaded_context(model);
}

#[cfg(not(feature = "voice"))]
pub fn preload(_model: &Model) {}

/// Lets go of the loaded model. Call it before the process exits: with the Metal backend,
/// whisper.cpp's GPU device is a static that is torn down at exit and asserts (aborting the app)
/// if a context still holds GPU memory then, so a model left loaded turns every quit into a crash.
#[cfg(feature = "voice")]
pub fn unload() {
    LOADED.lock().unwrap_or_else(|e| e.into_inner()).take();
}

#[cfg(not(feature = "voice"))]
pub fn unload() {}

/// Below this much speech in a clip, there is nothing to transcribe.
#[cfg(feature = "voice")]
const MIN_VOICED_SECONDS: f32 = 0.25;
/// A 30 ms frame counts as speech when it is this many times louder (RMS) than the clip's own
/// noise floor — relative, so quiet speech in a quiet room counts and steady noise does not.
#[cfg(feature = "voice")]
const OVER_NOISE: f32 = 3.0;
/// …and at least this loud in absolute terms (samples in -1..1), so a silent clip's tiny
/// fluctuations never count.
#[cfg(feature = "voice")]
const MIN_SPEECH_RMS: f32 = 0.003;
/// whisper's own "no speech" probability above which a segment is dropped.
#[cfg(feature = "voice")]
const NO_SPEECH: f32 = 0.6;

/// How many seconds of a 16 kHz clip stand out from its noise floor as speech.
#[cfg(feature = "voice")]
fn voiced_seconds(samples: &[f32]) -> f32 {
    const FRAME: usize = 480; // 30 ms at 16 kHz
    let rms: Vec<f32> = samples.chunks(FRAME).map(|frame| (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt()).collect();
    if rms.is_empty() {
        return 0.0;
    }
    // The noise floor: the level the quietest fifth of the clip stays under (people pause, so
    // part of every prompt is background).
    let mut sorted = rms.clone();
    sorted.sort_by(f32::total_cmp);
    let floor = sorted[sorted.len() / 5];
    let threshold = (floor * OVER_NOISE).max(MIN_SPEECH_RMS);
    let voiced = rms.iter().filter(|level| **level > threshold).count();
    voiced as f32 * FRAME as f32 / SAMPLE_RATE as f32
}

/// Whether a whole transcript is one of the sentences whisper makes up for noise or silence (it
/// learned them from video subtitles) rather than something said to an agent.
pub fn is_hallucination(text: &str) -> bool {
    const MADE_UP: &[&str] = &[
        "thank you",
        "thank you very much",
        "thanks for watching",
        "thank you for watching",
        "please subscribe",
        "you",
        "so",
        "bye",
        "감사합니다",
        "시청해주셔서 감사합니다",
        "시청해 주셔서 감사합니다",
        "구독과 좋아요 부탁드립니다",
        "고맙습니다",
        "ご視聴ありがとうございました",
        "ありがとうございました",
        "谢谢观看",
        "谢谢",
    ];
    let core: String =
        text.trim().trim_matches(|c: char| c.is_ascii_punctuation() || c == '。' || c == '!' || c.is_whitespace()).to_lowercase();
    core.is_empty() || MADE_UP.contains(&core.as_str())
}

/// The transcript without whisper's notes about the audio — `[BLANK_AUDIO]`, `[inaudible]`,
/// `(music)`, `*laughs*` — which are not words anyone said and must not be typed as a prompt.
pub fn without_annotations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(['[', '(', '*']) {
        let close = match rest.as_bytes()[start] {
            b'[' => ']',
            b'(' => ')',
            _ => '*',
        };
        let inner = &rest[start + 1..];
        // A note stands on its own; `f(x)` or `items[0]` is part of what was said.
        let alone = out.is_empty() && start == 0
            || rest[..start].ends_with(char::is_whitespace)
            || (start == 0 && out.ends_with(char::is_whitespace));
        let is_note = |note: &str| {
            !note.is_empty()
                && note.len() <= 40
                && note.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ' ' || c == '-' || c == '♪')
                // `*laughs*` is a note; `a * b * c` is arithmetic someone said.
                && (close != '*' || !note.contains(' ') && !note.starts_with(' '))
        };
        match inner.find(close) {
            // Only a short note made of letters: real speech keeps its brackets.
            Some(end) if alone && is_note(&inner[..end]) => {
                out.push_str(&rest[..start]);
                rest = &inner[end + close.len_utf8()..];
            }
            _ => {
                out.push_str(&rest[..start + 1]);
                rest = inner;
            }
        }
    }
    out.push_str(rest);
    // `>>` is the speaker-turn marker the tiny model puts in front of a turn (alone or glued to the
    // first word), and `♪` marks music: neither was said.
    let mut words = out.split_whitespace().filter(|word| !word.chars().all(|c| c == '♪') && !word.chars().all(|c| c == '>')).peekable();
    let mut text = String::new();
    while let Some(word) = words.next() {
        let word = if text.is_empty() { word.trim_start_matches(">>") } else { word };
        if word.is_empty() {
            continue;
        }
        text.push_str(word);
        if words.peek().is_some() {
            text.push(' ');
        }
    }
    text.trim_end().to_string()
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
    // A rate of zero would make the resampler's output length infinite, and the integer scale
    // below shifts by the bit depth: refuse anything outside what a microphone records.
    if !(1_000..=384_000).contains(&spec.sample_rate) {
        bail!("unsupported sample rate {}", spec.sample_rate);
    }
    if !(1..=32).contains(&spec.bits_per_sample) {
        bail!("unsupported bit depth {}", spec.bits_per_sample);
    }
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
    fn a_damaged_model_of_the_same_size_fails_verification() {
        let dir = std::env::temp_dir().join(format!("agentty-voice-verify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ggml-example.bin");
        std::fs::write(&path, b"not a real model").unwrap();
        let good = hex(&Sha256::digest(b"not a real model"));
        assert!(verified(&path, &good));
        // Asked again for the unchanged file: answered from memory.
        assert!(verified(&path, &good));
        // Same size, other bytes, a later write: checked again and refused.
        std::fs::write(&path, b"not a real m0del").unwrap();
        let later = std::time::SystemTime::now() + Duration::from_secs(60);
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(later).unwrap();
        assert!(!verified(&path, &good));
        assert!(!verified(&dir.join("missing.bin"), &good));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn made_up_sentences_are_not_prompts() {
        assert!(is_hallucination("Thank you."));
        assert!(is_hallucination(" 감사합니다. "));
        assert!(is_hallucination("."));
        assert!(is_hallucination(""));
        assert!(!is_hallucination("Thank you, now run the tests."));
        assert!(!is_hallucination("git commit하고 PR 올려줘."));
    }

    /// `secs` seconds of a tone at `amp` (peak), standing in for speech.
    #[cfg(feature = "voice")]
    fn tone(secs: f32, amp: f32) -> Vec<f32> {
        (0..(16_000.0 * secs) as usize).map(|i| (i as f32 * 0.05).sin() * amp).collect()
    }

    /// Steady noise at about `amp` RMS (a cheap deterministic generator).
    #[cfg(feature = "voice")]
    fn noise(secs: f32, amp: f32) -> Vec<f32> {
        let mut x: u32 = 12345;
        (0..(16_000.0 * secs) as usize)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((x >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * amp * 1.7
            })
            .collect()
    }

    #[cfg(feature = "voice")]
    #[test]
    fn silence_and_steady_noise_have_no_speech() {
        assert_eq!(voiced_seconds(&vec![0.0f32; 16_000 * 3]), 0.0);
        assert!(voiced_seconds(&noise(3.0, 0.002)) < MIN_VOICED_SECONDS);
        assert!(voiced_seconds(&noise(3.0, 0.012)) < MIN_VOICED_SECONDS);
        assert!(voiced_seconds(&noise(3.0, 0.05)) < MIN_VOICED_SECONDS);
    }

    #[cfg(feature = "voice")]
    #[test]
    fn quiet_speech_counts_against_its_own_background() {
        // A pause, then a second of quiet "speech" (peak 0.01, RMS ~0.007), then a pause.
        let mut quiet = vec![0.0f32; 16_000];
        quiet.extend(tone(1.0, 0.01));
        quiet.extend(vec![0.0f32; 16_000]);
        assert!((voiced_seconds(&quiet) - 1.0).abs() < 0.1, "{}", voiced_seconds(&quiet));
        // Speech over steady room noise.
        let mut noisy = noise(1.0, 0.012);
        let mut speech = noise(1.0, 0.012);
        for (s, t) in speech.iter_mut().zip(tone(1.0, 0.15)) {
            *s += t;
        }
        noisy.extend(speech);
        noisy.extend(noise(1.0, 0.012));
        assert!((voiced_seconds(&noisy) - 1.0).abs() < 0.1, "{}", voiced_seconds(&noisy));
    }

    #[test]
    fn the_recommended_model_is_turbo_and_offered() {
        assert_eq!(DEFAULT_MODEL, "large-v3-turbo");
        assert_eq!(MODELS.iter().map(|m| m.id).collect::<Vec<_>>(), ["small", "large-v3-turbo"]);
        assert!(MODELS.iter().all(|m| !RETIRED_FILES.contains(&m.file)));
    }

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

    #[test]
    fn cpu_check_needs_every_required_feature() {
        assert!(cpu_supported(|_| true));
        if cfg!(target_arch = "x86_64") {
            assert!(!cpu_supported(|_| false));
            for missing in X86_REQUIRED {
                assert!(!cpu_supported(|f| f != *missing), "a CPU without {missing} must not run voice");
            }
        } else {
            assert!(cpu_supported(|_| false), "only x86-64 has a CPU check");
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn every_required_feature_is_detected() {
        // `cpu_has` answers false for a name it does not know; every listed name must be one it checks.
        // CI runners have all of them, so this also proves the names are spelled as the detector wants.
        for feature in X86_REQUIRED {
            assert!(cpu_has(feature), "{feature} not detected on this machine");
        }
    }

    #[test]
    fn unavailable_follows_the_feature() {
        if AVAILABLE {
            assert_ne!(unavailable(), Some(Unavailable::NotBuilt));
        } else {
            assert_eq!(unavailable(), Some(Unavailable::NotBuilt));
            assert!(!supported());
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

    #[test]
    fn annotations_are_not_typed() {
        assert_eq!(without_annotations(" [BLANK_AUDIO]"), "");
        assert_eq!(without_annotations("[inaudible] [inaudible] [BLANK_AUDIO]"), "");
        assert_eq!(without_annotations("(music) list the files *laughs* please"), "list the files please");
        assert_eq!(without_annotations("call f(x) on items[0]"), "call f(x) on items[0]");
        assert_eq!(without_annotations("a [ b"), "a [ b");
        assert_eq!(without_annotations("파일 목록 보여줘"), "파일 목록 보여줘");
        assert_eq!(without_annotations("multiply a * b * c"), "multiply a * b * c");
        assert_eq!(without_annotations(">> list the files"), "list the files");
        assert_eq!(without_annotations(">>list the files"), "list the files");
        assert_eq!(without_annotations(" >> >> hello >> there"), "hello there");
        assert_eq!(without_annotations(">>"), "");
        assert_eq!(without_annotations("a->b"), "a->b");
        assert_eq!(without_annotations("[SPEAKER_00] hello ♪"), "hello");
    }

    #[cfg(feature = "voice")]
    #[test]
    fn zero_sample_rate_is_refused() {
        // A hand-built header: hound itself does not refuse a rate of 0.
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&40u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&0u32.to_le_bytes()); // sample rate
        wav.extend_from_slice(&0u32.to_le_bytes()); // byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&4u32.to_le_bytes());
        wav.extend_from_slice(&[0, 1, 0, 1]);
        assert!(wav_to_samples(&wav).is_err());
    }
}
