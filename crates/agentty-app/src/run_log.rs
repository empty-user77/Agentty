//! The text side of the run log: a program's output (bytes, as they come from its pipes) turned
//! into lines with their colours, a level for each line, and the filter the log panel shows them
//! through. No UI here.
//!
//! Output written for a terminal is cleaned up on the way: colours (SGR) are kept, every other
//! escape sequence is dropped, a carriage return that redraws a line (a progress bar) replaces
//! that line instead of piling up copies, and a backspace takes back a character.

use std::collections::VecDeque;
use std::ops::Range;

/// Lines kept per run; the oldest go first.
pub const MAX_LINES: usize = 20_000;
/// A line longer than this is cut (a minified bundle printed by mistake).
const MAX_LINE_CHARS: usize = 4_000;
/// An escape sequence still open after this many bytes is not one: it is dropped rather than
/// waited for, so a program that never ends one can't grow the buffer without bound.
const MAX_ESCAPE: usize = 4_096;
/// Coloured stretches kept per line: output that changes colour at every character keeps its
/// text, not thousands of spans.
const MAX_STYLES: usize = 64;
/// Chunks of output waiting for the UI: a program writing faster than the log can show waits.
const QUEUE: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Out,
    Err,
    /// A line of the panel's own: the command that started, how it ended.
    Note,
}

/// The colour of a stretch of text: one of the 16 terminal colours, or an exact one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Indexed(u8),
    Rgb(u32),
}

impl Color {
    /// As `0xRRGGBB`, for the dark chrome (the palette of a common dark terminal theme).
    pub fn rgb(self) -> u32 {
        const BASE: [u32; 16] = [
            0x5c6370, 0xf14c4c, 0x23d18b, 0xe5e510, 0x3b8eea, 0xd670d6, 0x29b8db, 0xcccccc, 0x767676, 0xf14c4c, 0x23d18b, 0xf5f543,
            0x3b8eea, 0xd670d6, 0x29b8db, 0xe5e5e5,
        ];
        match self {
            Color::Rgb(rgb) => rgb,
            Color::Indexed(n) if n < 16 => BASE[n as usize],
            Color::Indexed(n) if n < 232 => {
                let n = n - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v as u32 * 40 };
                (level(n / 36) << 16) | (level((n / 6) % 6) << 8) | level(n % 6)
            }
            Color::Indexed(n) => {
                let v = 8 + (n as u32 - 232) * 10;
                (v << 16) | (v << 8) | v
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Option<Color>,
    pub bold: bool,
}

/// A line of output: its text, the coloured stretches in it (byte ranges), and its level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub text: String,
    pub styles: Vec<(Range<usize>, Style)>,
    pub level: Level,
    pub stream: Stream,
}

/// Where the parser of one stream is.
#[derive(Default)]
struct Partial {
    text: String,
    styles: Vec<(Range<usize>, Style)>,
    style: Style,
    /// Bytes of a UTF-8 character or an escape sequence that continue in the next chunk.
    pending: Vec<u8>,
    /// A `\r` was the last thing seen: a `\n` next ends the line, anything else redraws it.
    carriage_return: bool,
}

impl Partial {
    fn push_char(&mut self, c: char) {
        if self.text.len() >= MAX_LINE_CHARS {
            return;
        }
        let start = self.text.len();
        self.text.push(c);
        if self.style == Style::default() {
            return;
        }
        let full = self.styles.len() >= MAX_STYLES;
        match self.styles.last_mut() {
            Some((range, style)) if *style == self.style && range.end == start => range.end = self.text.len(),
            _ if full => {}
            _ => self.styles.push((start..self.text.len(), self.style)),
        }
    }

    fn clear_line(&mut self) {
        self.text.clear();
        self.styles.clear();
    }

    fn backspace(&mut self) {
        if let Some(c) = self.text.pop() {
            let end = self.text.len();
            let _ = c;
            self.styles.retain_mut(|(range, _)| {
                range.end = range.end.min(end);
                range.start < range.end
            });
        }
    }

    /// Applies an SGR sequence's parameters (`1;31`, `38;5;208`, `0`).
    fn sgr(&mut self, params: &str) {
        let numbers: Vec<u32> =
            if params.is_empty() { vec![0] } else { params.split([';', ':']).map(|p| p.parse().unwrap_or(0)).collect() };
        let mut i = 0;
        while i < numbers.len() {
            match numbers[i] {
                0 => self.style = Style::default(),
                1 => self.style.bold = true,
                22 => self.style.bold = false,
                n @ 30..=37 => self.style.fg = Some(Color::Indexed((n - 30) as u8)),
                n @ 90..=97 => self.style.fg = Some(Color::Indexed((n - 90 + 8) as u8)),
                39 => self.style.fg = None,
                38 => match numbers.get(i + 1) {
                    Some(5) => {
                        self.style.fg = numbers.get(i + 2).map(|n| Color::Indexed(*n as u8));
                        i += 2;
                    }
                    Some(2) => {
                        let channel = |k: usize| numbers.get(i + k).copied().unwrap_or(0).min(255);
                        self.style.fg = Some(Color::Rgb((channel(2) << 16) | (channel(3) << 8) | channel(4)));
                        i += 4;
                    }
                    _ => {}
                },
                // A background colour: skip its arguments too.
                48 => match numbers.get(i + 1) {
                    Some(5) => i += 2,
                    Some(2) => i += 4,
                    _ => {}
                },
                _ => {}
            }
            i += 1;
        }
    }
}

/// The lines of one run, kept up to [`MAX_LINES`].
#[derive(Default)]
pub struct LogBuffer {
    lines: VecDeque<LogLine>,
    /// Lines dropped from the front so far: a line's number stays the same while it is kept.
    dropped: usize,
    out: Partial,
    err: Partial,
    /// Error and warning lines kept (the panel's own notes not counted).
    errors: usize,
    warnings: usize,
}

impl LogBuffer {
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn get(&self, index: usize) -> Option<&LogLine> {
        self.lines.get(index)
    }

    pub fn lines(&self) -> impl Iterator<Item = &LogLine> {
        self.lines.iter()
    }

    /// Lines dropped from the front since the start.
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    pub fn clear(&mut self) {
        self.dropped += self.lines.len();
        self.lines.clear();
        self.errors = 0;
        self.warnings = 0;
    }

    /// Error lines kept.
    pub fn errors(&self) -> usize {
        self.errors
    }

    /// Warning lines kept.
    pub fn warnings(&self) -> usize {
        self.warnings
    }

    fn count(&mut self, line: &LogLine, by: isize) {
        let counter = match (line.level, line.stream) {
            (_, Stream::Note) => return,
            (Level::Error, _) => &mut self.errors,
            (Level::Warn, _) => &mut self.warnings,
            _ => return,
        };
        *counter = counter.saturating_add_signed(by);
    }

    /// A line of the panel's own ("▶ npm run dev", "Process ended with code 1").
    pub fn note(&mut self, text: &str, level: Level) {
        self.add(LogLine { text: text.to_string(), styles: Vec::new(), level, stream: Stream::Note });
    }

    /// Takes a chunk of `stream`'s output.
    pub fn push(&mut self, stream: Stream, bytes: &[u8]) {
        let mut done = Vec::new();
        let partial = match stream {
            Stream::Out | Stream::Note => &mut self.out,
            Stream::Err => &mut self.err,
        };
        let mut data = std::mem::take(&mut partial.pending);
        data.extend_from_slice(bytes);
        let mut i = 0;
        while i < data.len() {
            let byte = data[i];
            if partial.carriage_return && byte != b'\n' {
                partial.carriage_return = false;
                // A redrawn line (a progress bar) replaces the old one, unless the `\r` ended the chunk.
                partial.clear_line();
            }
            match byte {
                b'\n' => {
                    partial.carriage_return = false;
                    done.push(finish(partial, stream));
                    i += 1;
                }
                b'\r' => {
                    partial.carriage_return = true;
                    i += 1;
                }
                0x08 => {
                    partial.backspace();
                    i += 1;
                }
                b'\t' => {
                    for _ in 0..4 {
                        partial.push_char(' ');
                    }
                    i += 1;
                }
                0x1b => match escape_length(&data[i..]) {
                    Some((length, sgr)) => {
                        if let Some(params) = sgr {
                            partial.sgr(params);
                        }
                        i += length;
                    }
                    // Too long to be a sequence still coming: the ESC is dropped, the rest read as text.
                    None if data.len() - i > MAX_ESCAPE => i += 1,
                    None => {
                        // The sequence continues in the next chunk.
                        partial.pending = data[i..].to_vec();
                        break;
                    }
                },
                b if b < 0x20 || b == 0x7f => i += 1,
                b if b < 0x80 => {
                    partial.push_char(b as char);
                    i += 1;
                }
                _ => {
                    let width = utf8_width(byte);
                    if i + width > data.len() {
                        partial.pending = data[i..].to_vec();
                        break;
                    }
                    match std::str::from_utf8(&data[i..i + width]) {
                        Ok(text) => {
                            text.chars().for_each(|c| partial.push_char(c));
                            i += width;
                        }
                        // Not UTF-8 (Latin-1, CP949): one replacement for the byte, and what
                        // follows read on its own — a newline after it still ends the line.
                        Err(_) => {
                            partial.push_char('\u{fffd}');
                            i += 1;
                        }
                    }
                }
            }
        }
        for line in done {
            self.add(line);
        }
    }

    /// The process ended: what it wrote without a final newline becomes a line too.
    pub fn flush(&mut self) {
        for stream in [Stream::Out, Stream::Err] {
            let partial = match stream {
                Stream::Out | Stream::Note => &mut self.out,
                Stream::Err => &mut self.err,
            };
            partial.pending.clear();
            partial.carriage_return = false;
            if !partial.text.is_empty() {
                let line = finish(partial, stream);
                self.add(line);
            }
        }
    }

    fn add(&mut self, line: LogLine) {
        self.count(&line, 1);
        self.lines.push_back(line);
        while self.lines.len() > MAX_LINES {
            if let Some(old) = self.lines.pop_front() {
                self.count(&old, -1);
            }
            self.dropped += 1;
        }
    }
}

fn finish(partial: &mut Partial, stream: Stream) -> LogLine {
    let text = std::mem::take(&mut partial.text);
    let styles = std::mem::take(&mut partial.styles);
    let level = level_of(&text);
    LogLine { text, styles, level, stream }
}

fn utf8_width(first: u8) -> usize {
    match first {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    }
}

/// How long the escape sequence at the start of `data` is, and an SGR's parameters if it is one.
/// `None` while it is not complete yet.
fn escape_length(data: &[u8]) -> Option<(usize, Option<&str>)> {
    match data.get(1)? {
        b'[' => {
            // CSI: parameters, then a final byte in 0x40..=0x7e.
            let end = data[2..].iter().position(|b| (0x40..=0x7e).contains(b))? + 2;
            let params = std::str::from_utf8(&data[2..end]).unwrap_or("");
            Some((end + 1, (data[end] == b'm').then_some(params)))
        }
        b']' | b'P' | b'_' | b'^' => {
            // OSC / DCS / …: up to BEL or ST (ESC \).
            for (i, pair) in data.windows(2).enumerate().skip(1) {
                if pair[0] == 0x07 {
                    return Some((i + 1, None));
                }
                if pair == [0x1b, b'\\'] {
                    return Some((i + 2, None));
                }
            }
            if data.last() == Some(&0x07) {
                return Some((data.len(), None));
            }
            None
        }
        // `ESC (B` and the like: one more byte.
        b'(' | b')' | b'#' => data.get(2).map(|_| (3, None)),
        // A two-byte sequence (`ESC =`, `ESC M`, …).
        0x30..=0x7e => Some((2, None)),
        // A lone ESC: only it goes, the byte after it (a newline) stays.
        _ => Some((1, None)),
    }
}

/// A line's level, from the words logging libraries print: `ERROR` / `WARN` (Spring Boot,
/// Logback, Log4j), `error` / `warn` tags of Node tools, and the lines of a Java stack trace.
pub fn level_of(text: &str) -> Level {
    let trimmed = text.trim_start();
    if trimmed.starts_with("at ") && text.starts_with(char::is_whitespace) || trimmed.starts_with("Caused by:") {
        return Level::Error;
    }
    // The most severe word wins; DEBUG only when nothing else is said.
    let (mut warn, mut debug) = (false, false);
    for word in text.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).take(24) {
        let found = match word {
            "ERROR" | "FATAL" | "SEVERE" | "ERR" | "CRITICAL" | "Error" | "error" | "Exception" | "Fatal" | "fatal" | "failed"
            | "FAILED" => Level::Error,
            "WARN" | "WARNING" | "Warning" | "warn" | "warning" | "deprecated" => Level::Warn,
            "DEBUG" | "TRACE" | "debug" | "trace" => Level::Debug,
            w if w.ends_with("Exception") || w.ends_with("Error") && w.len() > 5 => Level::Error,
            _ => continue,
        };
        match found {
            Level::Error => return Level::Error,
            Level::Warn => warn = true,
            _ => debug = true,
        }
    }
    if warn {
        Level::Warn
    } else if debug {
        Level::Debug
    } else {
        Level::Info
    }
}

/// Web addresses in `text` (`http://localhost:5173/`, `https://…`), as byte ranges.
pub fn links(text: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(offset) = text[from..].find("http") {
        let start = from + offset;
        let rest = &text[start..];
        if !(rest.starts_with("http://") || rest.starts_with("https://")) {
            from = start + 4;
            continue;
        }
        let mut end = start + rest.find(|c: char| c.is_whitespace() || "\"'<>`".contains(c)).unwrap_or(rest.len());
        // Punctuation that ends a sentence or closes a bracket around the address is not part of it.
        while end > start && text[..end].ends_with(['.', ',', ';', ':', ')', ']', '}']) {
            let closing = text[..end].chars().last();
            let opens = |c: char| text[start..end].matches(c).count();
            if closing == Some(']') && opens('[') >= opens(']') {
                break;
            }
            end -= 1;
        }
        if end > start + "http://".len() {
            found.push(start..end);
        }
        from = end.max(start + 4);
    }
    found
}

/// An address a server printed as where it listens, as a browser can open it: `0.0.0.0` and
/// `[::]` (every interface) become `localhost`.
pub fn openable(url: &str) -> String {
    url.replacen("://0.0.0.0", "://localhost", 1).replacen("://[::]", "://localhost", 1)
}

/// Whether the end of a run's output says a port it wanted was taken, and which ports it names.
pub fn port_conflict<'a>(lines: impl Iterator<Item = &'a LogLine>) -> Option<Vec<u16>> {
    let mut found = false;
    let mut ports = Vec::new();
    for line in lines {
        let lower = line.text.to_lowercase();
        let hit = lower.contains("address already in use")
            || lower.contains("eaddrinuse")
            || lower.contains("port") && (lower.contains("already in use") || lower.contains("is in use"));
        if !hit {
            continue;
        }
        found = true;
        for port in ports_said(&lower) {
            if !ports.contains(&port) {
                ports.push(port);
            }
        }
    }
    found.then_some(ports)
}

/// Numbers `text` (lower case) gives as ports: right after `:` (`:::5173`, `127.0.0.1:8080`) or
/// after the word `port` (`Port 8080 was already in use`). A date, a pid or an address part is
/// not one; nor is anything under 1024 (a time's `:34`, a privileged port no dev server uses).
fn ports_said(text: &str) -> Vec<u16> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let end = (i..bytes.len()).find(|&j| !bytes[j].is_ascii_digit()).unwrap_or(bytes.len());
        let before = text[..i].trim_end_matches(' ');
        let said = text[..i].ends_with(':') || before.ends_with("port");
        if let Some(port) = text[i..end].parse::<u16>().ok().filter(|p| said && *p >= 1024) {
            found.push(port);
        }
        i = end;
    }
    found
}

/// What the log panel shows: lines matching the words or the pattern, at or above a level.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
    /// Only lines at this level or more severe; `None` for all.
    pub level: Option<Level>,
}

/// A [`Filter`] ready to match lines.
pub enum Matcher {
    All,
    Words(String, bool),
    Pattern(regex::Regex),
}

impl Filter {
    /// The matcher for the query, or the pattern's error.
    pub fn matcher(&self) -> Result<Matcher, String> {
        if self.query.is_empty() {
            return Ok(Matcher::All);
        }
        if self.regex {
            return regex::RegexBuilder::new(&self.query)
                .case_insensitive(!self.case_sensitive)
                .size_limit(1 << 20)
                .build()
                .map(Matcher::Pattern)
                .map_err(|err| err.to_string().lines().last().unwrap_or_default().trim().to_string());
        }
        let words = if self.case_sensitive { self.query.clone() } else { self.query.to_lowercase() };
        Ok(Matcher::Words(words, self.case_sensitive))
    }

    pub fn shows_level(&self, level: Level) -> bool {
        self.level.is_none_or(|most| level <= most)
    }
}

impl Matcher {
    /// Where the line matches (empty for [`Matcher::All`]); `None` when it doesn't.
    pub fn find(&self, text: &str) -> Option<Vec<Range<usize>>> {
        match self {
            Matcher::All => Some(Vec::new()),
            Matcher::Words(words, case_sensitive) => {
                let found: Vec<Range<usize>> = if *case_sensitive {
                    text.match_indices(words.as_str()).map(|(i, m)| i..i + m.len()).collect()
                } else if text.is_ascii() {
                    text.to_ascii_lowercase().match_indices(words.as_str()).map(|(i, m)| i..i + m.len()).collect()
                } else {
                    // Lower-casing can change byte lengths outside ASCII: compare char by char.
                    lowercase_matches(text, words)
                };
                (!found.is_empty()).then_some(found)
            }
            Matcher::Pattern(pattern) => {
                let found: Vec<Range<usize>> = pattern.find_iter(text).map(|m| m.range()).filter(|r| !r.is_empty()).collect();
                (!found.is_empty() || pattern.is_match(text)).then_some(found)
            }
        }
    }
}

fn lowercase_matches(text: &str, words: &str) -> Vec<Range<usize>> {
    let needle: Vec<char> = words.chars().collect();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i + needle.len() <= chars.len() && !needle.is_empty() {
        let hit = needle.iter().enumerate().all(|(k, n)| chars[i + k].1.to_lowercase().eq(n.to_lowercase()));
        if hit {
            let end = chars.get(i + needle.len()).map_or(text.len(), |(b, _)| *b);
            found.push(chars[i].0..end);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    found
}

// -- the process --------------------------------------------------------------------------------

/// What a running process reports.
pub enum Event {
    Output(Stream, Vec<u8>),
    /// It ended: its exit code (`None` when a signal ended it).
    Exited(Option<i32>),
}

/// Process groups started for the run log, stopped when Agentty quits: they run in groups of
/// their own (so a stop reaches the whole tree), which also means no hangup reaches them.
static RUNNING: std::sync::Mutex<Vec<Record>> = std::sync::Mutex::new(Vec::new());

/// A run as kept in `run-log.json` in the data folder while it lasts: if Agentty goes down without
/// ending it (a crash, a kill, a build without the watchdog), the next start finds it there.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Record {
    /// Its process group (the watchdog's pid).
    group: u32,
    /// When the group's leader started, as `ps` says: tells it from a process given the same pid
    /// later.
    started: String,
}

fn registry_path() -> std::path::PathBuf {
    // Tests start real runs: their records stay out of the user's data folder.
    if cfg!(test) {
        return std::env::temp_dir().join(format!("agentty-run-log-test-{}.json", std::process::id()));
    }
    agentty_bridge::fsutil::data_dir().join("run-log.json")
}

fn save(records: &[Record]) {
    let path = registry_path();
    if records.is_empty() {
        let _ = std::fs::remove_file(path);
    } else if let Ok(bytes) = serde_json::to_vec_pretty(records) {
        let _ = agentty_bridge::fsutil::write_private(&path, &bytes);
    }
}

fn remember(pid: u32) {
    let started = start_time(pid).unwrap_or_default();
    if let Ok(mut running) = RUNNING.lock() {
        running.push(Record { group: pid, started });
        save(&running);
    }
}

/// Whether `pid` leads a run of this Agentty that has not ended: a signal for a run that ended
/// would reach whatever got its pid since.
fn is_ours(pid: u32) -> bool {
    RUNNING.lock().is_ok_and(|running| running.iter().any(|r| r.group == pid))
}

fn forget(pid: u32) {
    if let Ok(mut running) = RUNNING.lock() {
        running.retain(|r| r.group != pid);
        save(&running);
    }
}

fn run_ps(args: &[&str]) -> String {
    std::process::Command::new("/bin/ps")
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

/// When `pid` started, as `ps` says; `None` once it is gone.
fn start_time(pid: u32) -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    Some(run_ps(&["-o", "lstart=", "-p", &pid.to_string()]).trim().to_string()).filter(|s| !s.is_empty())
}

/// Every process of group `group` and everything below them (a process that put itself in a
/// group of its own is still a child of the run). Unix only: Windows ends a tree with `taskkill /T`.
#[cfg(unix)]
fn tree_of(group: u32) -> Vec<u32> {
    let rows: Vec<(u32, u32, u32)> = run_ps(&["-A", "-o", "pid=,ppid=,pgid=,stat="])
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let mut number = || parts.next()?.parse::<u32>().ok();
            let row = (number()?, number()?, number()?);
            // A zombie has ended already; it only waits for its parent to collect it.
            (!parts.next().is_some_and(|stat| stat.starts_with('Z'))).then_some(row)
        })
        .collect();
    let mut tree: Vec<u32> = rows.iter().filter(|(_, _, pgid)| *pgid == group).map(|(pid, _, _)| *pid).collect();
    let mut i = 0;
    while i < tree.len() && tree.len() < 4096 {
        let parent = tree[i];
        for (pid, ppid, _) in &rows {
            if *ppid == parent && !tree.contains(pid) {
                tree.push(*pid);
            }
        }
        i += 1;
    }
    let own = std::process::id();
    tree.retain(|pid| *pid > 1 && *pid != own);
    tree
}

/// Sends `signal` to group `group` and to every process of its tree.
#[cfg(unix)]
fn signal_tree(group: u32, signal: libc::c_int) {
    let own = unsafe { libc::getpgrp() } as u32;
    if group <= 1 || group == own {
        return;
    }
    unsafe {
        libc::killpg(group as libc::pid_t, signal);
    }
    for pid in tree_of(group) {
        unsafe {
            libc::kill(pid as libc::pid_t, signal);
        }
    }
}

/// Ends whatever is left of group `group`: `SIGTERM`, then `SIGKILL` for what ignores it.
/// Blocks for a few seconds at most; returns whether anything was left.
fn end_tree(group: u32) -> bool {
    #[cfg(unix)]
    {
        if tree_of(group).is_empty() {
            return false;
        }
        signal_tree(group, libc::SIGTERM);
        for _ in 0..12 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            if tree_of(group).is_empty() {
                return true;
            }
        }
        signal_tree(group, libc::SIGKILL);
        true
    }
    #[cfg(not(unix))]
    {
        let _ = group;
        false
    }
}

/// At start: ends the runs an earlier Agentty left behind (it crashed or was killed while they
/// ran), as `run-log.json` lists them. A group whose leader is alive but started at another time
/// is a different process that got the same pid, and is left alone. Returns how many were ended.
pub fn reap_orphans() -> usize {
    let path = registry_path();
    let Ok(text) = std::fs::read_to_string(&path) else { return 0 };
    let records: Vec<Record> = serde_json::from_str(&text).unwrap_or_default();
    // Runs of this session (started while this ran) are not left over: never these.
    let current: Vec<u32> = RUNNING.lock().map(|r| r.iter().map(|r| r.group).collect()).unwrap_or_default();
    let ended = reap(records.into_iter().filter(|r| !current.contains(&r.group)));
    if let Ok(running) = RUNNING.lock() {
        save(&running);
    }
    ended
}

/// Ends the runs of `records` that are still there and still the same processes.
fn reap(records: impl IntoIterator<Item = Record>) -> usize {
    let mut ended = 0;
    for record in records {
        let leader = start_time(record.group);
        if leader.is_some() && leader.as_deref() != Some(record.started.as_str()) {
            continue;
        }
        if end_tree(record.group) {
            eprintln!("agentty: ended a run left from before (process group {})", record.group);
            ended += 1;
        }
    }
    ended
}

/// Runs `$AGENTTY_RUN_COMMAND` in the user's shell and waits for it, while a watcher reads the
/// lifeline on stdin: when Agentty's end of it closes (Agentty quit, crashed or was killed — no
/// quit hook runs then, and a server in a group of its own gets no hangup), the whole group is
/// sent `SIGTERM`. Ctrl-C and `SIGTERM` reach the command; this shell only waits for it to end and
/// passes its exit code on. (`:` handlers, not ignored signals: an ignored one would be ignored by
/// the command too.)
const WATCHDOG: &str = r#"exec 3<&0
trap : INT TERM
"$AGENTTY_RUN_SHELL" -l -i -c "$AGENTTY_RUN_COMMAND" </dev/null 3<&- &
child=$!
(read -r _ <&3; kill -TERM 0) </dev/null >/dev/null 2>&1 &
watcher=$!
exec 3<&-
code=0
while kill -0 "$child" 2>/dev/null; do wait "$child"; code=$?; done
kill "$watcher" 2>/dev/null
exit "$code"
"#;

/// Starts `command` in `dir` the way a new terminal would (the user's login shell, so `PATH`
/// has nvm, sdkman, Homebrew, …), its output piped here. Returns its pid and its events.
pub fn start(command: &str, dir: &std::path::Path) -> std::io::Result<(u32, futures::channel::mpsc::Receiver<Event>)> {
    use std::io::Read;
    use std::process::Stdio;
    let shell = crate::launch::LaunchSpec::shell_program();
    let mut process = if cfg!(windows) {
        let mut process = agentty_bridge::process::command(&shell);
        process.args(["-NoLogo", "-Command", command]);
        process
    } else {
        // Through the watchdog, which ends the run when Agentty goes away however it does.
        let mut process = agentty_bridge::process::command("/bin/sh");
        process.args(["-c", WATCHDOG]).env("AGENTTY_RUN_SHELL", &shell).env("AGENTTY_RUN_COMMAND", command);
        process
    };
    process
        .current_dir(dir)
        // The watchdog's lifeline (Windows: nothing to read).
        .stdin(if cfg!(windows) { Stdio::null() } else { Stdio::piped() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Written to a pipe, most tools leave their colours out; these bring them back.
        .env("FORCE_COLOR", "1")
        .env("CLICOLOR_FORCE", "1")
        .env("SPRING_OUTPUT_ANSI_ENABLED", "ALWAYS")
        .env("PYTHONUNBUFFERED", "1")
        .env("TERM", "xterm-256color")
        .env_remove("CLAUDECODE");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        process.process_group(0);
    }
    let mut child = process.spawn()?;
    let pid = child.id();
    remember(pid);
    let (tx, rx) = futures::channel::mpsc::channel(QUEUE);
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let readers: Vec<(Stream, Box<dyn Read + Send>)> = [
        child.stdout.take().map(|s| (Stream::Out, Box::new(s) as Box<dyn Read + Send>)),
        child.stderr.take().map(|s| (Stream::Err, Box::new(s) as Box<dyn Read + Send>)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let count = readers.len();
    for (stream, mut reader) in readers {
        let (mut tx, done_tx) = (tx.clone(), done_tx.clone());
        std::thread::spawn(move || {
            use futures::SinkExt;
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    // Waits while the queue is full; ends once the log stopped listening (the run
                    // ended, but something it left behind still holds the pipe).
                    Ok(n) => {
                        if futures::executor::block_on(tx.send(Event::Output(stream, buffer[..n].to_vec()))).is_err() {
                            break;
                        }
                    }
                }
            }
            let _ = done_tx.send(());
        });
    }
    // Held open while the run lasts (`wait` would close it first): only Agentty's end closing
    // it, as Agentty exits or crashes, tells the watchdog to stop the run.
    let lifeline = child.stdin.take();
    std::thread::spawn(move || {
        // Ended by Ctrl-C, `SIGTERM` or `SIGKILL` (the watchdog reports those as 128 + signal):
        // stopped, not failed.
        let code = child.wait().ok().and_then(|status| status.code()).filter(|code| ![130, 137, 143].contains(code));
        drop(lifeline);
        // Children that outlived it (a server whose launcher exited) belong to nobody now.
        end_tree(pid);
        forget(pid);
        // What it wrote last is read before the end is told; a daemon it left behind holding the
        // pipes open (Gradle's) is not waited for.
        for _ in 0..count {
            if done_rx.recv_timeout(std::time::Duration::from_secs(1)).is_err() {
                break;
            }
        }
        let mut tx = tx;
        let _ = futures::executor::block_on(futures::SinkExt::send(&mut tx, Event::Exited(code)));
    });
    Ok((pid, rx))
}

/// Asks the run's process tree to stop: Ctrl-C first (`SIGINT`, what a terminal sends), then
/// `SIGTERM`, then `SIGKILL` (`force` 0, 1, 2). Windows ends the tree at once.
pub fn signal(pid: u32, force: u8) {
    if !is_ours(pid) {
        return;
    }
    #[cfg(unix)]
    {
        let signal = match force {
            0 => libc::SIGINT,
            1 => libc::SIGTERM,
            _ => libc::SIGKILL,
        };
        // The whole tree, not only the group: `ps` runs, so not on the UI thread.
        std::thread::spawn(move || signal_tree(pid, signal));
    }
    #[cfg(windows)]
    {
        let _ = force;
        // Off the UI thread: `taskkill` takes a moment.
        std::thread::spawn(move || {
            let _ = agentty_bridge::process::command("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output();
        });
    }
}

/// Agentty is quitting: every run of the log ends with it.
pub fn stop_all() {
    let groups: Vec<u32> = RUNNING.lock().map(|r| r.iter().map(|r| r.group).collect()).unwrap_or_default();
    #[cfg(unix)]
    for group in groups {
        signal_tree(group, libc::SIGTERM);
    }
    #[cfg(windows)]
    for group in groups {
        signal(group, 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(buffer: &LogBuffer) -> Vec<&str> {
        buffer.lines().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn colours_are_kept_and_other_escapes_dropped() {
        let mut buffer = LogBuffer::default();
        buffer.push(Stream::Out, b"\x1b[32mINFO\x1b[0m started \x1b]0;title\x07on \x1b[1;38;5;208m:8080\x1b[m\x1b[K\n");
        let line = buffer.get(0).unwrap();
        assert_eq!(line.text, "INFO started on :8080");
        assert_eq!(line.styles.len(), 2);
        assert_eq!(&line.text[line.styles[0].0.clone()], "INFO");
        assert_eq!(line.styles[0].1.fg, Some(Color::Indexed(2)));
        assert_eq!(&line.text[line.styles[1].0.clone()], ":8080");
        assert!(line.styles[1].1.bold);
        assert_eq!(line.styles[1].1.fg.map(Color::rgb), Some(Color::Indexed(208).rgb()));
    }

    #[test]
    fn chunks_split_anywhere_give_the_same_lines() {
        let output = "한글 로그 \x1b[31merror\x1b[0m here\r\nnext ✓\n".as_bytes();
        for cut in 0..output.len() {
            let mut buffer = LogBuffer::default();
            buffer.push(Stream::Out, &output[..cut]);
            buffer.push(Stream::Out, &output[cut..]);
            assert_eq!(texts(&buffer), ["한글 로그 error here", "next ✓"], "cut at {cut}");
        }
    }

    #[test]
    fn an_escape_that_never_ends_does_not_grow_without_bound() {
        let mut buffer = LogBuffer::default();
        buffer.push(Stream::Out, b"\x1b]0;");
        for _ in 0..100 {
            buffer.push(Stream::Out, &[b'x'; 1024]);
        }
        assert!(buffer.out.pending.len() <= MAX_ESCAPE + 1024);
        buffer.push(Stream::Out, b"\nafter\n");
        assert_eq!(buffer.lines().last().map(|l| l.text.as_str()), Some("after"));
    }

    #[test]
    fn bytes_that_are_not_utf8_do_not_swallow_the_newline() {
        let mut buffer = LogBuffer::default();
        buffer.push(Stream::Out, b"caf\xe9\nnext\x1b\nlast\n");
        assert_eq!(texts(&buffer), ["caf\u{fffd}", "next", "last"]);
    }

    #[test]
    fn a_redrawn_line_replaces_itself() {
        let mut buffer = LogBuffer::default();
        buffer.push(Stream::Out, b"building 10%\rbuilding 50%\rbuilding 100%\ndone\n");
        assert_eq!(texts(&buffer), ["building 100%", "done"]);
        buffer.push(Stream::Out, b"abc\x08d\n");
        assert_eq!(buffer.get(2).unwrap().text, "abd");
    }

    #[test]
    fn streams_keep_their_own_partial_lines_and_flush() {
        let mut buffer = LogBuffer::default();
        buffer.push(Stream::Out, b"out ");
        buffer.push(Stream::Err, b"err line\n");
        buffer.push(Stream::Out, b"line\nno newline");
        buffer.flush();
        assert_eq!(texts(&buffer), ["err line", "out line", "no newline"]);
        assert_eq!(buffer.get(0).unwrap().stream, Stream::Err);
    }

    #[test]
    fn level_counts_follow_the_lines_kept() {
        let mut buffer = LogBuffer::default();
        buffer.note("▶ npm run dev", Level::Error);
        buffer.push(Stream::Out, b"ERROR one\nWARN two\nfine\n");
        assert_eq!((buffer.errors(), buffer.warnings()), (1, 1));
        for n in 0..MAX_LINES {
            buffer.note(&n.to_string(), Level::Info);
        }
        assert_eq!((buffer.errors(), buffer.warnings()), (0, 0), "the counted lines were dropped");
        buffer.push(Stream::Out, b"ERROR again\n");
        buffer.clear();
        assert_eq!(buffer.errors(), 0);
    }

    #[test]
    fn colour_changes_at_every_character_keep_few_spans() {
        let mut buffer = LogBuffer::default();
        let mut line = Vec::new();
        for i in 0..1000u32 {
            line.extend_from_slice(format!("\x1b[3{}mx", i % 8).as_bytes());
        }
        line.push(b'\n');
        buffer.push(Stream::Out, &line);
        let kept = buffer.get(0).unwrap();
        assert_eq!(kept.text.len(), 1000);
        assert!(kept.styles.len() <= MAX_STYLES);
    }

    #[test]
    fn old_lines_go_first() {
        let mut buffer = LogBuffer::default();
        for n in 0..MAX_LINES + 5 {
            buffer.note(&n.to_string(), Level::Info);
        }
        assert_eq!(buffer.len(), MAX_LINES);
        assert_eq!(buffer.dropped(), 5);
        assert_eq!(buffer.get(0).unwrap().text, "5");
    }

    /// Runs `command` to its end and returns its lines and exit code.
    #[cfg(unix)]
    fn run(command: &str) -> (Vec<String>, Option<i32>) {
        use futures::StreamExt;
        let (_, mut events) = start(command, &std::env::temp_dir()).unwrap();
        let mut buffer = LogBuffer::default();
        let code = futures::executor::block_on(async {
            while let Some(event) = events.next().await {
                match event {
                    Event::Output(stream, bytes) => buffer.push(stream, &bytes),
                    Event::Exited(code) => return code,
                }
            }
            None
        });
        buffer.flush();
        (buffer.lines().map(|l| l.text.clone()).collect(), code)
    }

    #[cfg(unix)]
    #[test]
    fn a_run_reports_its_output_and_exit_code() {
        let (lines, code) = run("echo out; echo err >&2; printf 'last'; exit 3");
        assert!(lines.contains(&"out".to_string()), "{lines:?}");
        assert!(lines.contains(&"err".to_string()), "{lines:?}");
        assert!(lines.contains(&"last".to_string()), "{lines:?}");
        assert_eq!(code, Some(3));
    }

    #[cfg(unix)]
    #[test]
    fn a_stopped_run_ends_with_its_whole_group() {
        use futures::StreamExt;
        let (pid, mut events) = start("sleep 30 & sleep 30; wait", &std::env::temp_dir()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(800));
        signal(pid, 1);
        let started = std::time::Instant::now();
        let code = futures::executor::block_on(async {
            while let Some(event) = events.next().await {
                if let Event::Exited(code) = event {
                    return code;
                }
            }
            Some(-1)
        });
        assert_ne!(code, Some(-1), "it ended");
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        // Nothing of the group is left.
        let alive = unsafe { libc::killpg(pid as libc::pid_t, 0) } == 0;
        assert!(!alive, "group {pid} still has processes");
    }

    #[cfg(unix)]
    #[test]
    fn runs_left_behind_are_ended_but_not_a_process_that_took_their_pid() {
        use std::os::unix::process::CommandExt;
        let spawn = || std::process::Command::new("/bin/sh").args(["-c", "sleep 60 & sleep 60"]).process_group(0).spawn().unwrap();
        let (mut left, mut other) = (spawn(), spawn());
        std::thread::sleep(std::time::Duration::from_millis(300));
        let record = |child: &std::process::Child, started: String| Record { group: child.id(), started };
        let records = vec![
            record(&left, start_time(left.id()).unwrap()),
            // Recorded with another start time: the pid now belongs to someone else.
            record(&other, "Thu Jan  1 00:00:00 1970".into()),
        ];
        assert_eq!(reap(records), 1);
        assert!(tree_of(left.id()).is_empty(), "the run left behind is gone");
        assert!(!tree_of(other.id()).is_empty(), "the other process is untouched");
        let _ = left.wait();
        end_tree(other.id());
        let _ = other.wait();
    }

    #[test]
    fn links_are_found_without_what_surrounds_them() {
        let text = "Serving HTTP on :: port 5199 (http://[::]:5199/) ...";
        let found = links(text);
        assert_eq!(found.len(), 1);
        assert_eq!(&text[found[0].clone()], "http://[::]:5199/");
        assert_eq!(openable(&text[found[0].clone()]), "http://localhost:5199/");
        let text = "  ➜  Local:   http://localhost:5173/, docs at https://vite.dev.";
        let found: Vec<&str> = links(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(found, ["http://localhost:5173/", "https://vite.dev"]);
        assert!(links("httpd started, no address").is_empty());
        assert_eq!(openable("http://0.0.0.0:8080/actuator"), "http://localhost:8080/actuator");
    }

    #[test]
    fn a_taken_port_is_recognized() {
        let line = |text: &str| LogLine { text: text.into(), styles: Vec::new(), level: Level::Error, stream: Stream::Err };
        let lines = [line("Web server failed to start. Port 8080 was already in use.")];
        assert_eq!(port_conflict(lines.iter()), Some(vec![8080]));
        let lines = [line("Error: listen EADDRINUSE: address already in use :::5173")];
        assert_eq!(port_conflict(lines.iter()), Some(vec![5173]));
        let lines = [line("OSError: [Errno 48] Address already in use")];
        assert_eq!(port_conflict(lines.iter()), Some(vec![]));
        assert_eq!(port_conflict([line("all good")].iter()), None);
        // Dates, times and addresses are not ports.
        let lines = [line("2026/10/15 10:00:00 listen tcp :8080: bind: address already in use")];
        assert_eq!(port_conflict(lines.iter()), Some(vec![8080]));
        let lines = [line("bind 127.0.0.1:3000 failed: address already in use (pid 4242)")];
        assert_eq!(port_conflict(lines.iter()), Some(vec![3000]));
    }

    #[test]
    fn levels_from_common_loggers() {
        assert_eq!(level_of("2026-10-15T10:00:00.000+09:00 ERROR 1234 --- [main] o.s.boot.SpringApplication : failed"), Level::Error);
        assert_eq!(level_of("2026-10-15T10:00:00.000+09:00  WARN 1234 --- [main] o.h.engine : HHH000"), Level::Warn);
        assert_eq!(level_of("2026-10-15T10:00:00.000+09:00  INFO 1234 --- [main] Started App in 2.1 seconds"), Level::Info);
        assert_eq!(level_of("\tat com.example.App.main(App.java:10)"), Level::Error);
        assert_eq!(level_of("java.lang.IllegalStateException: boom"), Level::Error);
        assert_eq!(level_of("[vite] warning: something"), Level::Warn);
        assert_eq!(level_of("  ➜  Local:   http://localhost:5173/"), Level::Info);
        assert_eq!(level_of("DEBUG o.s.web : mapped"), Level::Debug);
        assert_eq!(level_of("DEBUG ERROR mixed"), Level::Error);
    }

    #[test]
    fn filters_by_words_pattern_and_level() {
        let words = Filter { query: "Started".into(), ..Default::default() }.matcher().unwrap();
        assert_eq!(words.find("app started on 8080"), Some(std::iter::once(4..11).collect()));
        assert_eq!(words.find("nothing"), None);
        let exact = Filter { query: "Started".into(), case_sensitive: true, ..Default::default() }.matcher().unwrap();
        assert_eq!(exact.find("app started"), None);
        let pattern = Filter { query: r":\d{4}".into(), regex: true, ..Default::default() }.matcher().unwrap();
        assert_eq!(pattern.find("on :8080 and :5173"), Some(vec![3..8, 13..18]));
        assert!(Filter { query: "(".into(), regex: true, ..Default::default() }.matcher().is_err());
        assert_eq!(Filter::default().matcher().unwrap().find("x"), Some(vec![]));
        let warn = Filter { level: Some(Level::Warn), ..Default::default() };
        assert!(warn.shows_level(Level::Error) && warn.shows_level(Level::Warn) && !warn.shows_level(Level::Info));
        let korean = Filter { query: "서버".into(), ..Default::default() }.matcher().unwrap();
        assert_eq!(korean.find("API 서버 시작"), Some(std::iter::once(4..10).collect()));
    }
}
