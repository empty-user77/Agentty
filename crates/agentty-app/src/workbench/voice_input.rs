//! Voice input from the status bar: the mic (or ⇧⌘M) records a prompt, the installed whisper
//! model transcribes it on this machine, and the text goes to the terminal that was active when
//! the recording started. Enter while recording sends it to an agent as a prompt; stopping with a
//! click or ⇧⌘M only types it in, to read and fix first; Esc throws it away.

use super::{Pane, Workbench};
use crate::i18n::{t, tf};
use crate::launch::PaneKind;
use crate::platform::mic::{self, Permission, Recorder};
use crate::terminal::TerminalView;
use crate::theme::{hex, Chrome};
use crate::ui::{icon, popover, TypeScale};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Focusable, WeakEntity, Window};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// Longest recording: a prompt, not a dictation session (the transcriber caps clips there too).
const MAX_RECORDING: Duration = Duration::from_secs(120);
/// Shorter than this is a stray click: thrown away without transcribing.
const MIN_RECORDING: Duration = Duration::from_millis(400);
/// How often the bar redraws while recording, for the clock and the level meter.
const TICK: Duration = Duration::from_millis(250);
/// How long a note next to the mic ("nothing heard", an error) stays.
const NOTE_FOR: Duration = Duration::from_secs(5);
/// A recording file older than this is left over from a crash, never one in use.
const STALE_RECORDING: Duration = Duration::from_secs(10 * 60);
const MENU_KEY: &str = "status-mic";

/// One microphone for the whole app: every window has a mic button, but only one records at once.
static RECORDING: AtomicBool = AtomicBool::new(false);
/// Numbers each recording: its file name, and which result or ticker still belongs to it.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Whether a model is installed, as last looked up: 0 not yet, 1 no, 2 yes. Shared by every
/// window, so the status bar never touches the disk to draw the mic; refreshed when the mic is
/// clicked, the setup opens or a download finishes.
static INSTALLED: AtomicU8 = AtomicU8::new(0);

/// Looks up again whether a model is installed (a few file lookups) and remembers it.
pub(super) fn refresh_installed() -> bool {
    let installed = agentty_bridge::voice::installed_model().is_some();
    INSTALLED.store(if installed { 2 } else { 1 }, Ordering::Relaxed);
    installed
}

/// Whether a model is installed, as last looked up (the first call looks).
fn installed() -> bool {
    match INSTALLED.load(Ordering::Relaxed) {
        0 => refresh_installed(),
        known => known == 2,
    }
}

/// Whether this Mac build can record and transcribe at all (voice compiled in, a processor that
/// runs it, and a microphone recorder for the platform).
pub fn available() -> bool {
    mic::HAS_MIC && agentty_bridge::voice::supported()
}

#[derive(Default)]
pub struct VoiceInput {
    phase: Phase,
    /// The setup popover (pick and install a model) is open.
    setup_open: bool,
    /// A short note shown next to the mic, whether it is an error, and since when.
    note: Option<(String, bool, Instant)>,
}

impl Drop for VoiceInput {
    /// A window closed mid-recording: stop the mic and leave no audio behind.
    fn drop(&mut self) {
        if let Phase::Recording { recorder, file, .. } = std::mem::take(&mut self.phase) {
            recorder.stop();
            let _ = std::fs::remove_file(file);
            RECORDING.store(false, Ordering::SeqCst);
        }
    }
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    /// Waiting for the person to answer the system's microphone prompt.
    Asking(u64),
    Recording {
        id: u64,
        recorder: Recorder,
        file: PathBuf,
        pane: WeakEntity<TerminalView>,
        started: Instant,
    },
    Transcribing(u64),
}

/// The folder recordings are written to until they are transcribed (then deleted): private to the
/// user, next to the models.
fn recordings_dir() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("voice")
}

/// Removes recordings a crash left behind (a recording in use is never this old).
fn sweep_stale_recordings(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("recording-") && name.ends_with(".wav")) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| SystemTime::now().duration_since(at).ok())
            .is_some_and(|age| age > STALE_RECORDING);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The transcript as typed into a terminal: one line, no control characters, so it can never press
/// Enter or send an escape sequence on its own.
fn as_typed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().filter(|c| !c.is_control()).collect()
}

/// The transcript as put into a chat's composer after `before` (what is left of the caret): set
/// apart from a word already there, and a space after it for the next words.
fn spliced(before: &str, text: &str) -> String {
    if before.is_empty() || before.ends_with(char::is_whitespace) {
        format!("{text} ")
    } else {
        format!(" {text} ")
    }
}

/// `0:07` for a recording's length.
fn clock(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Whether Enter may send to this terminal: an agent reads a prompt; a shell would run a misheard
/// sentence as a command, so there the words are only typed in.
fn sends_to(view: &TerminalView) -> bool {
    view.agent_kind().and_then(PaneKind::agent).is_some()
}

impl Workbench {
    /// Whether the status bar's model setup is showing (its download bar needs redraws).
    pub(super) fn voice_setup_open(&self) -> bool {
        self.voice_input.setup_open
    }

    /// The mic was clicked (or ⇧⌘M): start, stop and type in, give up waiting, or — with no model
    /// yet — open the setup.
    pub(super) fn toggle_mic(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !available() {
            return;
        }
        match self.voice_input.phase {
            Phase::Recording { .. } => return self.finish_recording(false, window, cx),
            // A click while waiting gives up: a late answer or transcript is then ignored.
            Phase::Asking(_) | Phase::Transcribing(_) => {
                self.voice_input.phase = Phase::Idle;
                return cx.notify();
            }
            Phase::Idle => {}
        }
        // The click that closed the setup from outside doesn't reopen it — but once a model is
        // installed there, the same click records straight away.
        let installed = refresh_installed();
        if self.just_dismissed(MENU_KEY) && !installed {
            return;
        }
        self.voice_input.note = None;
        if !installed {
            self.voice_input.setup_open = !self.voice_input.setup_open;
            return cx.notify();
        }
        self.voice_input.setup_open = false;
        let Some(pane) = self.active_pane() else {
            return self.mic_note(t(cx, "voice.no_terminal").to_string(), true, cx);
        };
        if RECORDING.load(Ordering::SeqCst) {
            return self.mic_note(t(cx, "voice.busy").to_string(), true, cx);
        }
        match mic::permission() {
            Permission::Granted => self.start_recording(pane, window, cx),
            Permission::Denied => {
                mic::open_privacy_settings();
                self.mic_note(t(cx, "voice.denied").to_string(), true, cx);
            }
            Permission::Undetermined => {
                let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
                self.voice_input.phase = Phase::Asking(id);
                cx.notify();
                let (tx, rx) = futures::channel::oneshot::channel();
                mic::request_permission(move |granted| {
                    let _ = tx.send(granted);
                });
                let pane = pane.downgrade();
                cx.spawn_in(window, async move |this, cx| {
                    let granted = rx.await.unwrap_or(false);
                    let _ = this.update_in(cx, |this, window, cx| {
                        if !matches!(this.voice_input.phase, Phase::Asking(asked) if asked == id) {
                            return;
                        }
                        this.voice_input.phase = Phase::Idle;
                        match pane.upgrade() {
                            Some(pane) if granted => this.start_recording(pane, window, cx),
                            _ if !granted => this.mic_note(t(cx, "voice.denied").to_string(), true, cx),
                            _ => this.mic_note(t(cx, "voice.no_terminal").to_string(), true, cx),
                        }
                    });
                })
                .detach();
            }
        }
    }

    fn start_recording(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        if RECORDING.swap(true, Ordering::SeqCst) {
            return self.mic_note(t(cx, "voice.busy").to_string(), true, cx);
        }
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let dir = recordings_dir();
        // AVAudioRecorder creates the WAV itself with the process umask (usually 0644), and may
        // replace a file made for it beforehand, so a pre-created 0600 file can't be relied on.
        // What keeps the audio private from its first byte is the folder: 0700 (an older `voice/`
        // left 0755 is tightened here), so no other user can reach the file inside it, whatever
        // its own mode. No recording when the folder can't be made private.
        if let Err(err) = agentty_bridge::fsutil::create_private_dir(&dir) {
            RECORDING.store(false, Ordering::SeqCst);
            return self.mic_note(tf(cx, "voice.failed", &[("reason", &err.to_string())]), true, cx);
        }
        sweep_stale_recordings(&dir);
        let file = dir.join(format!("recording-{}-{id}.wav", std::process::id()));
        let recorder = match Recorder::start(&file) {
            Ok(recorder) => recorder,
            Err(err) => {
                RECORDING.store(false, Ordering::SeqCst);
                let _ = std::fs::remove_file(&file);
                return self.mic_note(tf(cx, "voice.failed", &[("reason", &err.to_string())]), true, cx);
            }
        };
        // The audio is the user's own words: the file itself is made 0600 as well, in case it is
        // ever moved out of the private folder.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
        }
        // The keyboard goes back to the terminal, so Enter and Esc reach the mic and typing goes on.
        if self.page.is_none() {
            self.focus_pane(&pane, window, cx);
        }
        // The model's file is checked against its hash while the person speaks, not after.
        if let Some(model) = agentty_bridge::voice::installed_model() {
            agentty_bridge::voice::verify_in_background(model);
        }
        self.voice_input.phase = Phase::Recording { id, recorder, file, pane: pane.downgrade(), started: Instant::now() };
        cx.notify();
        // Redraw for the clock and the level meter, and stop at the longest a prompt may run. The
        // ticker belongs to this recording only: it ends with it, whatever starts next.
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            let more = this.update_in(cx, |this, window, cx| {
                let Phase::Recording { id: current, started, .. } = &this.voice_input.phase else { return false };
                if *current != id {
                    return false;
                }
                if started.elapsed() >= MAX_RECORDING {
                    this.finish_recording(false, window, cx);
                    return false;
                }
                cx.notify();
                true
            });
            if !matches!(more, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// Enter and Esc while recording, seen before the terminal: Enter sends what was said, Esc
    /// throws the recording away. Only while a terminal has the keyboard — in a dialog, a field or
    /// a page the keys keep their own meaning. Returns whether the key was taken.
    pub(super) fn voice_key(&mut self, event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let enter = event.keystroke.key == "enter";
        // Enter held down past the first press would type returns into the terminal while the
        // words are still being transcribed.
        if matches!(self.voice_input.phase, Phase::Transcribing(_)) {
            return enter && event.is_held;
        }
        if !matches!(self.voice_input.phase, Phase::Recording { .. }) {
            return false;
        }
        let m = &event.keystroke.modifiers;
        if m.platform || m.control || m.alt || m.shift || self.page.is_some() {
            return false;
        }
        // The terminal, a chat's composer standing for it, or nothing in particular (the workbench
        // itself, right after a click on the mic); a text field, dialog or palette with the
        // keyboard keeps its Enter and Esc.
        let terminal_focused = self.active_pane().is_some_and(|pane| {
            pane.read(cx).focus_handle(cx).contains_focused(window, cx)
                || self.chat_input_for(&pane, cx).is_some_and(|input| input.focus_handle(cx).contains_focused(window, cx))
        });
        if !terminal_focused && window.focused(cx).is_some_and(|focused| focused != self.focus_handle) {
            return false;
        }
        match event.keystroke.key.as_str() {
            "enter" => self.finish_recording(true, window, cx),
            // A menu open over the window closes first; the recording goes only once nothing is.
            "escape" if self.close_menus(cx) => {}
            "escape" => self.cancel_recording(cx),
            _ => return false,
        }
        true
    }

    /// Closes the menus that leave the keyboard where it was (TODO list, tab and pane menus, the
    /// status bar's menus, the plugins' menus, the mic's setup). Returns whether one was open.
    fn close_menus(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.tab_menu.take().is_some()
            | self.todo_menu.take().is_some()
            | self.pane_menu.take().is_some()
            | self.resume_menu.take().is_some()
            | self.status_menu.take().is_some()
            | std::mem::take(&mut self.plugin_mode_menu)
            | std::mem::take(&mut self.plugin_group_menu)
            | self.plugin_pin_menu.take().is_some()
            | self.plugin_tab_menu.take().is_some()
            | std::mem::take(&mut self.voice_input.setup_open);
        if open {
            cx.notify();
        }
        open
    }

    fn cancel_recording(&mut self, cx: &mut Context<Self>) {
        if let Phase::Recording { recorder, file, .. } = std::mem::take(&mut self.voice_input.phase) {
            recorder.stop();
            let _ = std::fs::remove_file(&file);
            RECORDING.store(false, Ordering::SeqCst);
        }
        cx.notify();
    }

    /// Stops the recording and transcribes it in the background; the text goes to the terminal the
    /// recording was made for, if it is still open — typed in to review, or with `send` entered
    /// as a prompt (agents only).
    fn finish_recording(&mut self, send: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Phase::Recording { id, recorder, file, pane, started } = std::mem::take(&mut self.voice_input.phase) else { return };
        recorder.stop();
        RECORDING.store(false, Ordering::SeqCst);
        let discard = |file: &Path| {
            let _ = std::fs::remove_file(file);
        };
        if started.elapsed() < MIN_RECORDING {
            discard(&file);
            return cx.notify();
        }
        if pane.upgrade().is_none() {
            discard(&file);
            return self.mic_note(t(cx, "voice.terminal_closed").to_string(), true, cx);
        }
        let Some(model) = agentty_bridge::voice::installed_model() else {
            discard(&file);
            return self.mic_note(t(cx, "voice.no_model").to_string(), true, cx);
        };
        self.voice_input.phase = Phase::Transcribing(id);
        cx.notify();
        let lang = crate::settings::settings(cx).voice.language.code();
        let task = cx.background_spawn(async move {
            let bytes = std::fs::read(&file);
            let _ = std::fs::remove_file(&file);
            let samples = agentty_bridge::voice::wav_to_samples(&bytes?)?;
            // The language chosen in the settings, else detected from the speech (people often speak
            // another language than the UI's).
            agentty_bridge::voice::transcribe(model, &samples, lang)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                // Given up on (a click while transcribing): the words go nowhere.
                if !matches!(this.voice_input.phase, Phase::Transcribing(current) if current == id) {
                    return;
                }
                this.voice_input.phase = Phase::Idle;
                match result.map(|text| as_typed(&text)) {
                    Ok(text) if text.is_empty() => this.mic_note(t(cx, "voice.nothing_heard").to_string(), false, cx),
                    Ok(text) => match pane.upgrade() {
                        // A lead shown as its chat: the words go to the composer the person sees,
                        // and Enter sends it the way the chat's own Send does.
                        Some(pane) if this.page.is_none() && this.chat_input_for(&pane, cx).is_some() => {
                            let input = this.chat_input_for(&pane, cx).expect("checked above");
                            input.update(cx, |input, cx| {
                                let at = input.cursor();
                                let typed = spliced(input.text().get(..at).unwrap_or(""), &text);
                                input.replace_range(at..at, &typed, cx);
                            });
                            if send {
                                this.send_chat_message(pane.entity_id(), window, cx);
                            }
                            this.focus_pane(&pane, window, cx);
                            cx.notify();
                        }
                        Some(pane) => {
                            if send && sends_to(pane.read(cx)) {
                                pane.update(cx, |view, cx| view.submit_prompt(text, cx));
                            } else {
                                // A space after it, so the next words (typed or spoken) don't run on.
                                pane.update(cx, |view, _| view.insert_text(&format!("{text} ")));
                                if send {
                                    this.mic_note(t(cx, "voice.shell_typed").to_string(), false, cx);
                                }
                            }
                            if this.page.is_none() {
                                this.focus_pane(&pane, window, cx);
                            }
                            cx.notify();
                        }
                        None => this.mic_note(t(cx, "voice.terminal_closed").to_string(), true, cx),
                    },
                    Err(err) => this.mic_note(tf(cx, "voice.failed", &[("reason", &err.to_string())]), true, cx),
                }
            });
        })
        .detach();
    }

    fn mic_note(&mut self, text: String, error: bool, cx: &mut Context<Self>) {
        self.voice_input.note = Some((text, error, Instant::now()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NOTE_FOR).await;
            let _ = this.update(cx, |this, cx| {
                if this.voice_input.note.as_ref().is_some_and(|(_, _, at)| at.elapsed() >= NOTE_FOR) {
                    this.voice_input.note = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The mic in the status bar, with what it is doing: idle, asking, recording (clock and level),
    /// transcribing, or a short note. `None` where the platform cannot record.
    pub(super) fn render_mic_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !available() {
            return None;
        }
        let busy = !matches!(self.voice_input.phase, Phase::Idle);
        if !busy && self.active_pane().is_none() {
            return None;
        }
        let installed = installed();
        let (glyph_color, label, tooltip): (u32, Option<(String, u32)>, String) = match &self.voice_input.phase {
            Phase::Idle => {
                let note = self.voice_input.note.as_ref().filter(|(text, _, _)| !text.is_empty());
                let tip = if installed { t(cx, "voice.idle") } else { t(cx, "voice.setup_hint") };
                (
                    if self.voice_input.setup_open { Chrome::BRIGHT } else { Chrome::MUTED },
                    note.map(|(text, error, _)| (text.clone(), if *error { Chrome::ERROR } else { Chrome::MUTED })),
                    tip.to_string(),
                )
            }
            Phase::Asking(_) => {
                (Chrome::WARNING, Some((t(cx, "voice.asking").to_string(), Chrome::WARNING)), t(cx, "voice.asking").to_string())
            }
            Phase::Recording { started, .. } => (
                Chrome::ERROR,
                Some((
                    format!("{}  ·  {}", tf(cx, "voice.recording", &[("time", &clock(started.elapsed()))]), t(cx, "voice.keys")),
                    Chrome::ERROR,
                )),
                t(cx, "voice.stop").to_string(),
            ),
            Phase::Transcribing(_) => {
                (Chrome::MUTED, Some((t(cx, "voice.transcribing").to_string(), Chrome::MUTED)), t(cx, "voice.transcribing").to_string())
            }
        };
        let (phase_tag, show_tooltip) = match &self.voice_input.phase {
            Phase::Idle => (0u64, true),
            Phase::Asking(_) => (1, true),
            Phase::Recording { .. } => (2, false),
            Phase::Transcribing(_) => (3, false),
        };
        let level = match &self.voice_input.phase {
            Phase::Recording { recorder, .. } => Some(recorder.level()),
            _ => None,
        };
        let glyph: AnyElement = if matches!(self.voice_input.phase, Phase::Transcribing(_)) {
            crate::ui::spinner(13., hex(Chrome::MUTED)).into_any_element()
        } else {
            icon("mic", 13., hex(glyph_color)).into_any_element()
        };
        let button = div()
            // A tooltip built while idle would stay up over the recording label while the pointer
            // rests on the mic: the element id changes with the phase (a new element, no tooltip
            // carried over) and nothing is shown while recording or transcribing — the label says it.
            .id(gpui::ElementId::NamedInteger(MENU_KEY.into(), phase_tag))
            .when(show_tooltip, |d| d.tooltip(crate::ui::Tooltip::text(tooltip, None)))
            .h_full()
            .px_1p5()
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .when(self.voice_input.setup_open, |d| d.bg(hex(Chrome::SELECTED)))
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .when(level.is_some(), |d| d.child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(hex(Chrome::ERROR))))
            .child(glyph)
            .when_some(level, |d, level| {
                // A small meter, so it is plain the mic hears something.
                d.child(
                    div()
                        .flex_shrink_0()
                        .w(px(18.))
                        .h(px(3.))
                        .rounded_full()
                        .bg(hex(Chrome::OVERLAY_BORDER))
                        .child(div().h(px(3.)).rounded_full().bg(hex(Chrome::ERROR)).w(px(18. * level))),
                )
            })
            .when_some(label, |d, (text, color)| d.child(div().t_caption().whitespace_nowrap().text_color(hex(color)).child(text)))
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.toggle_mic(window, cx)));
        let setup = self.voice_input.setup_open.then(|| {
            let panel = popover()
                .w(px(380.))
                .p_2()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if this.voice_input.setup_open {
                        this.voice_input.setup_open = false;
                        this.note_dismissed(MENU_KEY);
                        cx.notify();
                    }
                }))
                .child(self.render_voice_card(cx));
            div().absolute().bottom(px(24.)).right_0().child(gpui::deferred(crate::ui::fade_in("status-mic-fade", panel)).with_priority(3))
        });
        Some(div().relative().h_full().flex().items_center().child(button).children(setup).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_is_typed_as_one_safe_line() {
        assert_eq!(as_typed("  hello\nworld \t again "), "hello world again");
        assert_eq!(as_typed("run\u{1b}[2J this\r"), "run[2J this");
        assert_eq!(as_typed(""), "");
    }

    #[test]
    fn transcript_is_set_apart_in_the_composer() {
        assert_eq!(spliced("", "hello"), "hello ");
        assert_eq!(spliced("fix the ", "tests"), "tests ");
        assert_eq!(spliced("fix the", "tests"), " tests ");
    }

    #[test]
    fn clock_reads_minutes_and_seconds() {
        assert_eq!(clock(Duration::from_secs(7)), "0:07");
        assert_eq!(clock(Duration::from_secs(75)), "1:15");
    }

    #[test]
    fn only_old_recordings_are_swept() {
        let dir = std::env::temp_dir().join(format!("agentty-voice-sweep-{}", std::process::id()));
        agentty_bridge::fsutil::create_private_dir(&dir).unwrap();
        let old = dir.join("recording-1-1.wav");
        let fresh = dir.join("recording-1-2.wav");
        let model = dir.join("ggml-base.bin");
        for path in [&old, &fresh, &model] {
            std::fs::write(path, b"x").unwrap();
        }
        let long_ago = SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options().write(true).open(&old).unwrap().set_modified(long_ago).unwrap();
        std::fs::File::options().write(true).open(&model).unwrap().set_modified(long_ago).unwrap();
        sweep_stale_recordings(&dir);
        assert!(!old.exists());
        assert!(fresh.exists());
        assert!(model.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
