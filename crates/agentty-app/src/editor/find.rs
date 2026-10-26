//! Find in file (⌘F): a small bar over the editor that finds a query in the file in front (any
//! case, or as typed with **Aa**; anywhere, or whole names only with **ab**; as a regular
//! expression with **.\***, `$1` in the replacement for its groups), highlights every
//! match (`element.rs` paints them) and selects one at a time; Enter / ↓ / ⌘G next, ⇧Enter / ↑ /
//! ⇧⌘G previous, Esc closes. ⌥⌘F (or the bar's arrow) adds a replace row: Enter replaces the
//! selected match and goes to the next, ⌘Enter replaces them all as one undo step. The bar sits
//! outside the editor's key context, so typing in it never reaches the file (⌘Z undoes the query,
//! not an edit).

use super::buffer::{Buffer, Pos};
use super::language::Language;
use super::{CodeEditor, Content};
use crate::i18n::t;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, Chrome};
use crate::ui::{icon_only, TypeScale};
use gpui::{actions, div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, Subscription, Window};

actions!(code_editor, [FindNext, FindPrevious, FindReplace, ToggleCase, ToggleWord, ToggleRegex, ReplaceAll]);

/// The bar's own key context: steps work while typing in it, and the editor's keys don't.
const CONTEXT: &str = "EditorFind";

/// Matches counted and highlighted at most: past this a query is too common to step through, and
/// a huge file stays responsive while typing.
pub const MAX_MATCHES: usize = 10_000;

/// Where one match starts and ends, on one line.
pub type Match = (Pos, Pos);

pub fn bind_keys(cx: &mut gpui::App) {
    use crate::key;
    // ⇧⌘G opens Git everywhere else: only the bar takes it over.
    cx.bind_keys([
        key("cmd-g", FindNext, Some(CONTEXT)),
        key("cmd-shift-g", FindPrevious, Some(CONTEXT)),
        key("cmd-g", FindNext, Some(super::CONTEXT)),
        // VS Code's keys for replace and the two options.
        key("cmd-alt-f", FindReplace, Some(super::CONTEXT)),
        key("cmd-alt-f", FindReplace, Some(CONTEXT)),
        key("cmd-alt-c", ToggleCase, Some(CONTEXT)),
        key("cmd-alt-w", ToggleWord, Some(CONTEXT)),
        key("cmd-alt-r", ToggleRegex, Some(CONTEXT)),
        key("cmd-enter", ReplaceAll, Some(CONTEXT)),
    ]);
}

pub struct FindBar {
    pub(super) input: Entity<TextInput>,
    query: String,
    pub matches: Vec<(Pos, Pos)>,
    /// What the matches were found in: the file in front, its text's revision and the options.
    searched: Option<(usize, u64, Options)>,
    options: Options,
    /// The replace row, while it shows.
    replace: Option<(Entity<TextInput>, Subscription)>,
    _subscription: Subscription,
}

/// How the query is matched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// As typed (Aa), not in any case.
    pub case: bool,
    /// Whole names only (ab): not inside a longer one.
    pub word: bool,
    /// A regular expression (.*), matched within a line.
    pub regex: bool,
}

/// One of the bar's options.
#[derive(Clone, Copy)]
enum Toggle {
    Case,
    Word,
    Regex,
}

/// Regular expressions longer than this, or compiling to more, are not searched for.
const REGEX_LEN: usize = 1000;
const REGEX_SIZE: usize = 1 << 20;

fn regex_for(query: &str, options: Options) -> Option<regex::Regex> {
    if query.len() > REGEX_LEN {
        return None;
    }
    regex::RegexBuilder::new(query).case_insensitive(!options.case).size_limit(REGEX_SIZE).build().ok()
}

impl CodeEditor {
    /// Opens the bar (or goes back to it), starting from the selected text when one line of it is
    /// selected.
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(self.active) else { return };
        // The grid's filter is its find: rows with the text in them.
        if doc.showing_grid() {
            self.focus_grid_filter(window, cx);
            return;
        }
        if !matches!(doc.content, Content::Text) || doc.previewing() {
            return;
        }
        // The selected text, or else the name at the cursor (as VS Code does).
        let selected = doc.buffer.selected_text();
        let cursor = doc.buffer.cursor();
        let line = doc.buffer.line(cursor.line);
        let name = super::symbols::name_at(line, cursor.col, doc.language).filter(|(s, e)| s < e);
        let word = name.map(|(s, e)| line[s..e].to_string());
        let seed = if selected.is_empty() { word } else { (!selected.contains('\n')).then_some(selected) };
        // Found from the name's start, so the name the cursor is on is the one selected.
        if let (true, Some((start, _))) = (doc.buffer.selection.is_empty(), name) {
            if let Some(doc) = self.docs.get_mut(self.active) {
                doc.buffer.move_to(Pos::new(cursor.line, start), false);
            }
        }
        if let Some(bar) = self.find.as_ref() {
            let input = bar.input.clone();
            input.update(cx, |input, cx| {
                if let Some(seed) = &seed {
                    input.set_text(seed.clone(), cx);
                }
                input.focus_and_select_all(window, cx);
            });
            self.find_query_changed(cx);
            return;
        }
        let input = cx.new(|cx| TextInput::localized(seed.unwrap_or_default(), "find.placeholder", window, cx));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Changed => this.find_query_changed(cx),
            TextInputEvent::Confirmed | TextInputEvent::Down => this.find_step(true, cx),
            TextInputEvent::Up => this.find_step(false, cx),
            TextInputEvent::Cancelled => this.close_find(window, cx),
            _ => {}
        });
        input.update(cx, |input, cx| input.focus_and_select_all(window, cx));
        self.find = Some(FindBar {
            input,
            query: String::new(),
            matches: Vec::new(),
            searched: None,
            options: Options::default(),
            replace: None,
            _subscription: subscription,
        });
        self.find_query_changed(cx);
    }

    /// Closes the bar; the match that was selected stays selected, in the editor that has focus again.
    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.take().is_some() {
            window.focus(&self.focus_handle);
            cx.notify();
        }
    }

    /// Finds again when the query, the file in front or its text changed since the last search.
    pub(super) fn refresh_find(&mut self) {
        let Some(bar) = self.find.as_mut() else { return };
        let Some(doc) = self.docs.get(self.active) else {
            bar.matches.clear();
            bar.searched = None;
            return;
        };
        let key = (self.active, doc.buffer.revision(), bar.options);
        if bar.searched != Some(key) {
            bar.matches = find_all(&doc.buffer, &bar.query, bar.options, doc.language);
            bar.searched = Some(key);
        }
    }

    /// The matches to paint, and which one is selected: the one the selection covers exactly.
    pub(super) fn find_matches(&self) -> Option<(&[Match], Option<usize>)> {
        let bar = self.find.as_ref()?;
        let doc = self.docs.get(self.active)?;
        let selected = doc.buffer.selection.range();
        let current = bar.matches.binary_search_by(|m| m.0.cmp(&selected.0)).ok().filter(|&i| bar.matches[i] == selected);
        Some((&bar.matches, current))
    }

    /// Typing in the bar: the match at or after where the selection starts, so the one being
    /// typed stays selected as the query grows.
    pub(super) fn find_query_changed(&mut self, cx: &mut Context<Self>) {
        let Some(bar) = self.find.as_mut() else { return };
        bar.query = bar.input.read(cx).text().to_string();
        bar.searched = None;
        self.refresh_find();
        let Some(doc) = self.docs.get(self.active) else { return };
        let at = doc.buffer.selection.range().0;
        let target = self.find.as_ref().and_then(|bar| next_from(&bar.matches, at, true));
        self.select_match(target, cx);
    }

    pub(super) fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.refresh_find();
        let Some((matches, current)) = self.find_matches() else { return };
        let Some(doc) = self.docs.get(self.active) else { return };
        let at = if forward { doc.buffer.selection.range().1 } else { doc.buffer.selection.range().0 };
        let target = step(matches, current, at, forward);
        self.select_match(target, cx);
    }

    /// ⌥⌘F: the bar with its replace row (or the row closed again from the bar).
    pub fn open_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            self.open_find(window, cx);
        }
        let Some(bar) = self.find.as_mut() else { return };
        if let Some((input, _)) = bar.replace.as_ref() {
            let input = input.clone();
            return input.update(cx, |input, cx| input.focus_and_select_all(window, cx));
        }
        let input = cx.new(|cx| TextInput::localized("", "find.replace_placeholder", window, cx));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Confirmed => this.replace_one(cx),
            TextInputEvent::Cancelled => this.close_find(window, cx),
            _ => {}
        });
        input.update(cx, |input, cx| input.focus_and_select_all(window, cx));
        bar.replace = Some((input, subscription));
        cx.notify();
    }

    fn toggle_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.find.as_mut() {
            Some(bar) if bar.replace.is_some() => {
                bar.replace = None;
                let input = bar.input.clone();
                input.update(cx, |input, cx| input.focus_and_select_all(window, cx));
                cx.notify();
            }
            _ => self.open_replace(window, cx),
        }
    }

    fn toggle_option(&mut self, which: Toggle, cx: &mut Context<Self>) {
        let Some(bar) = self.find.as_mut() else { return };
        let option = match which {
            Toggle::Case => &mut bar.options.case,
            Toggle::Word => &mut bar.options.word,
            Toggle::Regex => &mut bar.options.regex,
        };
        *option = !*option;
        self.find_query_changed(cx);
    }

    /// What replaces the match `start..end`: `with`, or for a regular expression `with` with
    /// `$1` / `${name}` taken from its groups.
    fn replacement_for(&self, start: Pos, end: Pos, with: &str) -> String {
        let Some(bar) = self.find.as_ref().filter(|bar| bar.options.regex) else { return with.to_string() };
        let Some(doc) = self.docs.get(self.active) else { return with.to_string() };
        let line = doc.buffer.line(start.line);
        match regex_for(&bar.query, bar.options).and_then(|re| {
            re.captures_at(line, start.col).filter(|c| c.get(0).is_some_and(|m| m.start() == start.col && m.end() == end.col))
        }) {
            Some(captures) => {
                let mut out = String::new();
                captures.expand(with, &mut out);
                out
            }
            None => with.to_string(),
        }
    }

    fn replacement(&self, cx: &gpui::App) -> Option<String> {
        Some(self.find.as_ref()?.replace.as_ref()?.0.read(cx).text().to_string())
    }

    /// Replaces the selected match and selects the next one (only selects one when none is).
    pub(super) fn replace_one(&mut self, cx: &mut Context<Self>) {
        let Some(with) = self.replacement(cx) else { return };
        self.refresh_find();
        let current = self.find_matches().and_then(|(matches, current)| current.map(|i| matches[i]));
        if let Some((start, end)) = current {
            let with = self.replacement_for(start, end, &with);
            self.edit(cx, |b| {
                b.move_to(start, false);
                b.move_to(end, true);
                b.paste(&with);
            });
        }
        self.find_step(true, cx);
    }

    /// Replaces every match, as one undo step.
    pub(super) fn replace_all(&mut self, cx: &mut Context<Self>) {
        let Some(with) = self.replacement(cx) else { return };
        self.refresh_find();
        let Some(bar) = self.find.as_ref() else { return };
        if bar.matches.is_empty() {
            return;
        }
        let Some(doc) = self.docs.get(self.active) else { return };
        let regex = bar.options.regex.then(|| regex_for(&bar.query, bar.options)).flatten();
        let (new, count) = replaced(&doc.buffer, &bar.matches, &with, regex.as_ref());
        if self.replace_text(self.active, &new, cx) {
            self.flash(crate::i18n::tf(cx, "find.replaced", &[("n", &count.to_string())]), cx);
        }
        cx.notify();
    }

    /// Debug driver: the replace row with `with` in it.
    pub(super) fn debug_replace(&mut self, with: &str, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.open_replace(window, cx);
        if let Some((input, _)) = self.find.as_ref().and_then(|bar| bar.replace.as_ref()) {
            let input = input.clone();
            input.update(cx, |input, cx| input.set_text(with.to_string(), cx));
        }
        if all {
            self.replace_all(cx);
        } else {
            self.replace_one(cx);
        }
    }

    /// Debug driver: the options, by name (`case`, `word`, `regex`).
    pub(super) fn debug_find_option(&mut self, name: &str, cx: &mut Context<Self>) {
        let which = match name {
            "case" => Toggle::Case,
            "regex" => Toggle::Regex,
            _ => Toggle::Word,
        };
        self.toggle_option(which, cx);
    }

    fn select_match(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        let Some((start, end)) = index.and_then(|i| self.find.as_ref()?.matches.get(i).copied()) else {
            cx.notify();
            return;
        };
        self.motion(cx, |buffer| {
            buffer.move_to(start, false);
            buffer.move_to(end, true);
        });
    }

    pub(super) fn render_find_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.refresh_find();
        let (matches, current) = self.find_matches()?;
        let query_empty = self.find.as_ref()?.query.is_empty();
        let count = match (matches.len(), current) {
            _ if query_empty => String::new(),
            (0, _) => t(cx, "find.none").to_string(),
            (total, Some(index)) => format!("{}/{total}{}", index + 1, if total >= MAX_MATCHES { "+" } else { "" }),
            (total, None) => format!("{total}{}", if total >= MAX_MATCHES { "+" } else { "" }),
        };
        let bar = self.find.as_ref()?;
        let input = bar.input.clone();
        let options = bar.options;
        let replace = bar.replace.as_ref().map(|(input, _)| input.clone());
        // A small text toggle (Aa, ab), lit while on.
        let toggle = |id: &'static str, label: &'static str, on: bool, tip: String, keys: &'static str| {
            div()
                .id(id)
                .flex_shrink_0()
                .px_1()
                .rounded_sm()
                .cursor_pointer()
                .t_caption()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(hex(if on { Chrome::BRIGHT } else { Chrome::MUTED }))
                .when(on, |d| d.bg(crate::theme::hex_alpha(Chrome::ACCENT, 0.45)))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .child(label)
                .tooltip(crate::ui::Tooltip::text(tip, Some(keys)))
        };
        let find_row = div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                icon_only(
                    "editor-find-replace",
                    if replace.is_some() { "chevron-down" } else { "chevron-right" },
                    cx.listener(|this, _: &ClickEvent, window, cx| this.toggle_replace(window, cx)),
                )
                .tooltip(crate::ui::Tooltip::text(t(cx, "find.replace"), Some("⌥⌘F"))),
            )
            .child(div().flex_1().min_w_0().t_small().text_color(hex(Chrome::BRIGHT)).child(input))
            .child(
                toggle("editor-find-case", "Aa", options.case, t(cx, "find.match_case").to_string(), "⌥⌘C")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_option(Toggle::Case, cx))),
            )
            .child(
                toggle("editor-find-word", "ab", options.word, t(cx, "find.whole_word").to_string(), "⌥⌘W")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_option(Toggle::Word, cx))),
            )
            .child(
                toggle("editor-find-regex", ".*", options.regex, t(cx, "find.regex").to_string(), "⌥⌘R")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_option(Toggle::Regex, cx))),
            )
            .child(div().flex_shrink_0().t_caption().text_color(hex(Chrome::MUTED)).child(count))
            .child(
                icon_only("editor-find-prev", "chevron-up", cx.listener(|this, _: &ClickEvent, _, cx| this.find_step(false, cx)))
                    .tooltip(crate::ui::Tooltip::text(t(cx, "tooltip.find_prev"), Some("⇧↩"))),
            )
            .child(
                icon_only("editor-find-next", "chevron-down", cx.listener(|this, _: &ClickEvent, _, cx| this.find_step(true, cx)))
                    .tooltip(crate::ui::Tooltip::text(t(cx, "tooltip.find_next"), Some("↩"))),
            )
            .child(
                icon_only("editor-find-close", "x", cx.listener(|this, _: &ClickEvent, window, cx| this.close_find(window, cx)))
                    .tooltip(crate::ui::Tooltip::text(t(cx, "confirm.close"), Some("esc"))),
            );
        let text_button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .flex_shrink_0()
                .px_1p5()
                .py_0p5()
                .rounded_sm()
                .cursor_pointer()
                .t_caption()
                .text_color(hex(Chrome::FOREGROUND))
                .hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT)))
                .child(label)
        };
        let replace_row = replace.map(|input| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .pl(px(26.))
                .child(div().flex_1().min_w_0().t_small().text_color(hex(Chrome::BRIGHT)).child(input))
                .child(
                    text_button("editor-replace-one", t(cx, "find.replace"))
                        .tooltip(crate::ui::Tooltip::text(t(cx, "find.replace"), Some("↩")))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.replace_one(cx))),
                )
                .child(
                    text_button("editor-replace-all", t(cx, "find.replace_all"))
                        .tooltip(crate::ui::Tooltip::text(t(cx, "find.replace_all"), Some("⌘↩")))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.replace_all(cx))),
                )
        });
        Some(
            div()
                .id("editor-find")
                .key_context(CONTEXT)
                .absolute()
                .top(px(41.))
                .right(px(18. + self.beside_width()))
                .w(px(420.))
                .max_w(gpui::relative(0.9))
                .px_1()
                .py_1()
                .flex()
                .flex_col()
                .gap_1()
                .rounded_md()
                .bg(hex(Chrome::OVERLAY))
                .border_1()
                .border_color(hex(Chrome::OVERLAY_BORDER))
                .shadow_lg()
                .occlude()
                .on_action(cx.listener(|this, _: &FindNext, _, cx| this.find_step(true, cx)))
                .on_action(cx.listener(|this, _: &FindPrevious, _, cx| this.find_step(false, cx)))
                .on_action(cx.listener(|this, _: &FindReplace, window, cx| this.toggle_replace(window, cx)))
                .on_action(cx.listener(|this, _: &ToggleCase, _, cx| this.toggle_option(Toggle::Case, cx)))
                .on_action(cx.listener(|this, _: &ToggleWord, _, cx| this.toggle_option(Toggle::Word, cx)))
                .on_action(cx.listener(|this, _: &ToggleRegex, _, cx| this.toggle_option(Toggle::Regex, cx)))
                .on_action(cx.listener(|this, _: &ReplaceAll, _, cx| this.replace_all(cx)))
                // ⇧Enter is the field's own (a new line in a multi-line field): taken before it.
                .capture_action(cx.listener(|this, _: &crate::text_input::NewLine, _, cx| {
                    this.find_step(false, cx);
                    cx.stop_propagation();
                }))
                .child(find_row)
                .children(replace_row)
                .into_any_element(),
        )
    }
}

/// Where `query` appears in the buffer, in document order, as (start, end) on one line each.
/// Matches don't overlap. An empty query, or one spanning lines, finds nothing. Whole names are
/// told apart by `language`'s name characters.
pub fn find_all(buffer: &Buffer, query: &str, options: Options, language: Language) -> Vec<(Pos, Pos)> {
    if query.is_empty() || query.contains('\n') {
        return Vec::new();
    }
    if options.regex {
        return find_regex(buffer, query, options, language);
    }
    let needle: Vec<char> = if options.case { query.chars().collect() } else { query.chars().flat_map(char::to_lowercase).collect() };
    let name = |c: char| super::symbols::is_name_char(c, language);
    let mut found = Vec::new();
    for index in 0..buffer.line_count() {
        let line = buffer.line(index);
        for (start, end) in find_in_line(line, &needle, options.case) {
            let whole = !line[..start].chars().next_back().is_some_and(name) && !line[end..].chars().next().is_some_and(name);
            if options.word && !whole {
                continue;
            }
            found.push((Pos::new(index, start), Pos::new(index, end)));
            if found.len() >= MAX_MATCHES {
                return found;
            }
        }
    }
    found
}

/// [`find_all`] for a regular expression: its matches within each line (an empty one is no
/// match), none at all while it does not compile.
fn find_regex(buffer: &Buffer, query: &str, options: Options, language: Language) -> Vec<(Pos, Pos)> {
    let Some(re) = regex_for(query, options) else { return Vec::new() };
    let name = |c: char| super::symbols::is_name_char(c, language);
    let mut found = Vec::new();
    for index in 0..buffer.line_count() {
        let line = buffer.line(index);
        for m in re.find_iter(line).filter(|m| !m.is_empty()) {
            let whole = !line[..m.start()].chars().next_back().is_some_and(name) && !line[m.end()..].chars().next().is_some_and(name);
            if options.word && !whole {
                continue;
            }
            found.push((Pos::new(index, m.start()), Pos::new(index, m.end())));
            if found.len() >= MAX_MATCHES {
                return found;
            }
        }
    }
    found
}

/// Byte ranges of `needle` in `line` (already lowercased unless `case`), compared a (lowercased)
/// character at a time so a match's ends are always character boundaries of the line itself.
fn find_in_line(line: &str, needle: &[char], case: bool) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let starts: Vec<usize> = line.char_indices().map(|(i, _)| i).collect();
    let mut from = 0;
    while from < starts.len() {
        let start = starts[from];
        let mut wanted = needle.iter();
        let mut end = None;
        for (offset, c) in line[start..].char_indices() {
            let same = if case { wanted.next() == Some(&c) } else { c.to_lowercase().all(|l| wanted.next() == Some(&l)) };
            if !same {
                break;
            }
            if wanted.as_slice().is_empty() {
                end = Some(start + offset + c.len_utf8());
                break;
            }
        }
        match end {
            Some(end) => {
                found.push((start, end));
                from = starts.partition_point(|&s| s < end);
            }
            None => from += 1,
        }
    }
    found
}

/// The buffer's text with every match (in document order, one line each) made `with`, and how
/// many there were.
/// With `regex`, `$1` / `${name}` in `with` are its groups at each match.
fn replaced(buffer: &Buffer, matches: &[(Pos, Pos)], with: &str, regex: Option<&regex::Regex>) -> (String, usize) {
    let mut lines: Vec<String> = (0..buffer.line_count()).map(|i| buffer.line(i).to_string()).collect();
    for (start, end) in matches.iter().rev() {
        let text = match regex.and_then(|re| re.captures_at(buffer.line(start.line), start.col)) {
            Some(captures) => {
                let mut out = String::new();
                captures.expand(with, &mut out);
                out
            }
            None => with.to_string(),
        };
        lines[start.line].replace_range(start.col..end.col, &text);
    }
    (lines.join("\n"), matches.len())
}

/// The match to go to from `at`: the first one starting at or after it going forward, the last
/// one ending at or before it going back, wrapping around the file.
pub fn next_from(matches: &[(Pos, Pos)], at: Pos, forward: bool) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    Some(if forward {
        let index = matches.partition_point(|(start, _)| *start < at);
        if index == matches.len() {
            0
        } else {
            index
        }
    } else {
        let index = matches.partition_point(|(_, end)| *end <= at);
        if index == 0 {
            matches.len() - 1
        } else {
            index - 1
        }
    })
}

/// One step from match `current` (wrapping), or the nearest to `at` when none is current.
pub fn step(matches: &[(Pos, Pos)], current: Option<usize>, at: Pos, forward: bool) -> Option<usize> {
    let count = matches.len();
    match current {
        _ if count == 0 => None,
        Some(index) if index < count => Some(if forward { (index + 1) % count } else { (index + count - 1) % count }),
        _ => next_from(matches, at, forward),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(text: &str, query: &str) -> Vec<((usize, usize), (usize, usize))> {
        find_all(&Buffer::new(text), query, Options::default(), Language::Other)
            .into_iter()
            .map(|(s, e)| ((s.line, s.col), (e.line, e.col)))
            .collect()
    }

    #[test]
    fn options_and_replacing() {
        let buffer = Buffer::new("total Total totals\nsub_total total");
        let find = |case, word| {
            find_all(&buffer, "total", Options { case, word, regex: false }, Language::JavaScript)
                .into_iter()
                .map(|(s, _)| (s.line, s.col))
                .collect::<Vec<_>>()
        };
        assert_eq!(find(false, false), [(0, 0), (0, 6), (0, 12), (1, 4), (1, 10)]);
        assert_eq!(find(true, false), [(0, 0), (0, 12), (1, 4), (1, 10)]);
        assert_eq!(find(false, true), [(0, 0), (0, 6), (1, 10)]);
        assert_eq!(find(true, true), [(0, 0), (1, 10)]);
        let matches = find_all(&buffer, "total", Options { case: true, word: true, regex: false }, Language::JavaScript);
        assert_eq!(replaced(&buffer, &matches, "sum", None), ("sum Total totals\nsub_total sum".to_string(), 2));
    }

    #[test]
    fn regular_expressions_with_groups() {
        let buffer = Buffer::new("const a = useState(1);\nconst bb = useState(22);\nlet c = 3;");
        let regex = Options { regex: true, ..Options::default() };
        let matches = find_all(&buffer, r"const (\w+) = useState\((\d+)\)", regex, Language::JavaScript);
        assert_eq!(matches.len(), 2);
        let re = regex_for(r"const (\w+) = useState\((\d+)\)", regex).unwrap();
        assert_eq!(
            replaced(&buffer, &matches, "const [$1, set] = useState($2)", Some(&re)).0,
            "const [a, set] = useState(1);\nconst [bb, set] = useState(22);\nlet c = 3;"
        );
        // Case, an empty match, a broken expression.
        assert_eq!(find_all(&buffer, "CONST", regex, Language::JavaScript).len(), 2);
        assert!(find_all(&buffer, "CONST", Options { case: true, ..regex }, Language::JavaScript).is_empty());
        assert!(find_all(&buffer, "x*", regex, Language::JavaScript).is_empty());
        assert!(find_all(&buffer, "(unclosed", regex, Language::JavaScript).is_empty());
    }

    #[test]
    fn finds_every_match_ignoring_case() {
        assert_eq!(ranges("Foo foo\nbar FOO", "foo"), vec![((0, 0), (0, 3)), ((0, 4), (0, 7)), ((1, 4), (1, 7))]);
        assert!(ranges("foo", "").is_empty());
        assert!(ranges("foo\nbar", "o\nb").is_empty(), "a query over two lines finds nothing");
    }

    #[test]
    fn matches_do_not_overlap() {
        assert_eq!(ranges("aaaa", "aa"), vec![((0, 0), (0, 2)), ((0, 2), (0, 4))]);
    }

    #[test]
    fn multibyte_text_gives_byte_ranges_on_character_boundaries() {
        // Each Hangul syllable is three bytes.
        assert_eq!(ranges("안녕 세상, 세상아", "세상"), vec![((0, 7), (0, 13)), ((0, 15), (0, 21))]);
        assert_eq!(ranges("ÉCOLE école", "école"), vec![((0, 0), (0, 6)), ((0, 7), (0, 13))]);
    }

    #[test]
    fn stepping_wraps_around() {
        let matches = find_all(&Buffer::new("x\nfoo\nx foo\nfoo"), "foo", Options::default(), Language::Other);
        assert_eq!(matches.len(), 3);
        assert_eq!(step(&matches, Some(2), Pos::default(), true), Some(0));
        assert_eq!(step(&matches, Some(0), Pos::default(), false), Some(2));
        assert_eq!(step(&matches, Some(1), Pos::default(), true), Some(2));
        assert_eq!(step(&[], Some(1), Pos::default(), true), None);
    }

    #[test]
    fn starts_from_the_cursor() {
        let matches = find_all(&Buffer::new("foo\nfoo\nfoo"), "foo", Options::default(), Language::Other);
        assert_eq!(next_from(&matches, Pos::new(1, 1), true), Some(2));
        assert_eq!(next_from(&matches, Pos::new(1, 0), true), Some(1));
        assert_eq!(next_from(&matches, Pos::new(2, 2), true), Some(0), "past the last one: back to the top");
        assert_eq!(next_from(&matches, Pos::new(1, 1), false), Some(0));
        assert_eq!(next_from(&matches, Pos::new(0, 0), false), Some(2), "before the first one: from the bottom");
    }

    #[test]
    fn matches_follow_an_edit() {
        let mut buffer = Buffer::new("one two\ntwo");
        assert_eq!(find_all(&buffer, "two", Options::default(), Language::Other).len(), 2);
        buffer.select_all();
        buffer.insert("two two two");
        assert_eq!(find_all(&buffer, "two", Options::default(), Language::Other).len(), 3);
    }
}
