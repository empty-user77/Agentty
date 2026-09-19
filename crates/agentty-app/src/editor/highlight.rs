//! Syntax colors: Sublime Text grammars (bat's curated set, through `two-face`) run by `syntect`
//! on its pure-Rust regex engine, with a Dark+-like palette that matches the app.
//!
//! Lines are highlighted top-down and cached with the parser state after each one, so an edit
//! only re-reads the lines from the change down to what is on screen.

use super::language::Language;
use std::path::Path;
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Instant;
use syntect::highlighting::{
    Color, FontStyle, HighlightIterator, HighlightState, Highlighter, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

/// Lines longer than this are shown uncolored (minified files would take seconds each).
const MAX_LINE: usize = 4_000;

pub const FOREGROUND: u32 = 0xd4d4d4;

static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
static THEME: OnceLock<Theme> = OnceLock::new();

/// The grammars, once loaded. Loading takes a moment: [`load`] does it off the UI thread first.
pub fn syntaxes() -> Option<&'static SyntaxSet> {
    SYNTAXES.get()
}

/// Loads the grammars (call from a background thread).
pub fn load() -> &'static SyntaxSet {
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

fn theme() -> &'static Theme {
    THEME.get_or_init(dark_theme)
}

/// Grammar for a file: the language's own, else whatever knows the extension or first line.
pub fn grammar_for<'a>(set: &'a SyntaxSet, language: Language, path: &Path, first_line: &str) -> Option<&'a SyntaxReference> {
    language
        .grammars()
        .iter()
        .find_map(|name| set.find_syntax_by_name(name))
        .or_else(|| path.extension().and_then(|e| set.find_syntax_by_extension(&e.to_string_lossy())))
        .or_else(|| path.file_name().and_then(|n| set.find_syntax_by_extension(&n.to_string_lossy())))
        .or_else(|| set.find_syntax_by_first_line(first_line))
        .filter(|syntax| syntax.name != "Plain Text")
}

/// A colored stretch of a line: `len` bytes in `color`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub len: usize,
    pub color: u32,
    pub bold: bool,
    pub italic: bool,
}

/// Colors of a document's lines, highlighted on demand from the top.
pub struct Highlights {
    syntax: Option<&'static SyntaxReference>,
    /// Spans of each highlighted line, and the parser state after it.
    lines: Vec<(Vec<Span>, ParseState, HighlightState)>,
}

impl Highlights {
    pub fn new(syntax: Option<&'static SyntaxReference>) -> Self {
        Self { syntax, lines: Vec::new() }
    }

    pub fn grammar_name(&self) -> Option<&'static str> {
        self.syntax.map(|s| s.name.as_str())
    }

    /// Forgets lines from `line` on (they changed).
    pub fn invalidate_from(&mut self, line: usize) {
        self.lines.truncate(line);
    }

    /// Highlights on until line `upto` (exclusive), stopping early at `deadline` so a frame is
    /// never held up; `line(i)` gives the text. Returns false while lines before `upto` are
    /// still missing (the next frame goes on).
    pub fn advance<'t>(&mut self, upto: usize, deadline: Instant, line: impl Fn(usize) -> &'t str) -> bool {
        let (Some(syntax), Some(set)) = (self.syntax, syntaxes()) else { return true };
        let highlighter = Highlighter::new(theme());
        let mut done = 0usize;
        while self.lines.len() < upto {
            // The clock is read every few lines only.
            if done % 16 == 15 && Instant::now() >= deadline {
                return false;
            }
            let index = self.lines.len();
            let (mut parse, mut state) = match self.lines.last() {
                Some((_, parse, state)) => (parse.clone(), state.clone()),
                None => (ParseState::new(syntax), HighlightState::new(&highlighter, ScopeStack::new())),
            };
            let text = line(index);
            let spans = if text.len() > MAX_LINE {
                vec![Span { len: text.len(), color: FOREGROUND, bold: false, italic: false }]
            } else {
                let with_newline = format!("{text}\n");
                match parse.parse_line(&with_newline, set) {
                    Ok(ops) => {
                        let mut spans: Vec<Span> = Vec::new();
                        let mut used = 0;
                        for (style, piece) in HighlightIterator::new(&mut state, &ops, &with_newline, &highlighter) {
                            // The line break the grammar was given is not part of the line.
                            let len = piece.len().min(text.len() - used);
                            if len == 0 {
                                continue;
                            }
                            used += len;
                            let color = (style.foreground.r as u32) << 16 | (style.foreground.g as u32) << 8 | style.foreground.b as u32;
                            spans.push(Span {
                                len,
                                color,
                                bold: style.font_style.contains(FontStyle::BOLD),
                                italic: style.font_style.contains(FontStyle::ITALIC),
                            });
                        }
                        spans
                    }
                    // A grammar that fails on a line leaves the rest of the file uncolored.
                    Err(_) => {
                        self.syntax = None;
                        return true;
                    }
                }
            };
            self.lines.push((spans, parse, state));
            done += 1;
        }
        true
    }

    /// Spans of a highlighted line.
    pub fn spans(&self, line: usize) -> Option<&[Span]> {
        self.lines.get(line).map(|(spans, _, _)| spans.as_slice())
    }
}

fn rgb(value: u32) -> Color {
    Color { r: (value >> 16) as u8, g: (value >> 8) as u8, b: value as u8, a: 0xff }
}

/// Visual Studio Code's Dark+ colors, by TextMate scope.
fn dark_theme() -> Theme {
    const RULES: &[(&str, u32, FontStyle)] = &[
        ("comment, punctuation.definition.comment", 0x6a9955, FontStyle::empty()),
        ("string, punctuation.definition.string, string.quoted", 0xce9178, FontStyle::empty()),
        ("constant.character.escape, constant.character", 0xd7ba7d, FontStyle::empty()),
        ("string.regexp", 0xd16969, FontStyle::empty()),
        ("constant.numeric", 0xb5cea8, FontStyle::empty()),
        ("constant.language, constant.other.boolean, variable.language", 0x569cd6, FontStyle::empty()),
        ("constant.other, variable.other.constant, support.constant", 0x4fc1ff, FontStyle::empty()),
        ("keyword, storage, storage.type, storage.modifier", 0x569cd6, FontStyle::empty()),
        ("keyword.control, keyword.other.import, keyword.other.package, keyword.control.import", 0xc586c0, FontStyle::empty()),
        ("keyword.operator", FOREGROUND, FontStyle::empty()),
        ("keyword.operator.new, keyword.operator.expression, keyword.operator.word", 0x569cd6, FontStyle::empty()),
        ("entity.name.function, support.function, meta.function-call, variable.function", 0xdcdcaa, FontStyle::empty()),
        (
            "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.name.interface, entity.name.trait, \
             entity.other.inherited-class, support.type, support.class, storage.type.java, storage.type.kotlin",
            0x4ec9b0,
            FontStyle::empty(),
        ),
        ("storage.type.primitive, storage.type.numeric, storage.type.boolean", 0x569cd6, FontStyle::empty()),
        ("variable, variable.other, variable.parameter, meta.definition.variable", 0x9cdcfe, FontStyle::empty()),
        (
            "support.type.property-name, meta.object-literal.key, meta.mapping.key string, entity.name.tag.yaml",
            0x9cdcfe,
            FontStyle::empty(),
        ),
        ("entity.name.tag, punctuation.definition.tag", 0x569cd6, FontStyle::empty()),
        ("entity.other.attribute-name", 0x9cdcfe, FontStyle::empty()),
        ("entity.other.attribute-name.class.css, entity.other.attribute-name.id.css", 0xd7ba7d, FontStyle::empty()),
        ("support.constant.property-value, meta.property-value", 0xce9178, FontStyle::empty()),
        ("meta.annotation, storage.type.annotation, punctuation.definition.annotation, meta.attribute", 0xdcdcaa, FontStyle::empty()),
        ("entity.name.namespace, entity.name.module, entity.name.section.toml, entity.name.table.toml", 0x4ec9b0, FontStyle::empty()),
        ("entity.name.lifetime, storage.modifier.lifetime", 0x569cd6, FontStyle::ITALIC),
        ("entity.name.macro, support.macro", 0x569cd6, FontStyle::empty()),
        ("markup.heading, entity.name.section.markdown", 0x569cd6, FontStyle::BOLD),
        ("markup.bold", 0x569cd6, FontStyle::BOLD),
        ("markup.italic", FOREGROUND, FontStyle::ITALIC),
        ("markup.raw, markup.inline.raw, markup.raw.block", 0xce9178, FontStyle::empty()),
        ("markup.underline.link, string.other.link", 0x3794ff, FontStyle::empty()),
        ("markup.quote", 0x6a9955, FontStyle::empty()),
        ("markup.list punctuation.definition.list_item", 0x6796e6, FontStyle::empty()),
        ("markup.inserted", 0xb5cea8, FontStyle::empty()),
        ("markup.deleted", 0xce9178, FontStyle::empty()),
        ("invalid", 0xf44747, FontStyle::empty()),
    ];
    let scopes = RULES
        .iter()
        .filter_map(|(selector, color, font_style)| {
            Some(ThemeItem {
                scope: ScopeSelectors::from_str(selector).ok()?,
                style: StyleModifier {
                    foreground: Some(rgb(*color)),
                    background: None,
                    font_style: (!font_style.is_empty()).then_some(*font_style),
                },
            })
        })
        .collect();
    Theme {
        name: Some("Agentty Dark".into()),
        author: None,
        settings: ThemeSettings { foreground: Some(rgb(FOREGROUND)), background: Some(rgb(0x1f1f1f)), ..ThemeSettings::default() },
        scopes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LANGUAGES: [Language; 21] = [
        Language::Java,
        Language::Kotlin,
        Language::JavaScript,
        Language::Jsx,
        Language::TypeScript,
        Language::Tsx,
        Language::Json,
        Language::Yaml,
        Language::Toml,
        Language::Markdown,
        Language::Rust,
        Language::Python,
        Language::Go,
        Language::Shell,
        Language::Html,
        Language::Css,
        Language::Scss,
        Language::Less,
        Language::Sql,
        Language::Xml,
        Language::Dockerfile,
    ];

    #[test]
    fn every_language_has_a_bundled_grammar() {
        let set = load();
        for language in LANGUAGES {
            let grammar = language.grammars().iter().find_map(|name| set.find_syntax_by_name(name));
            assert!(grammar.is_some(), "{language:?}: none of {:?} is bundled", language.grammars());
        }
        // Others by extension.
        assert_eq!(grammar_for(set, Language::Other, Path::new("main.c"), "").map(|s| s.name.as_str()), Some("C"));
        assert!(grammar_for(set, Language::Other, Path::new("notes.txt"), "").is_none());
    }

    #[test]
    fn keywords_strings_and_comments_get_their_colors() {
        let set = load();
        let syntax = grammar_for(set, Language::Kotlin, Path::new("a.kt"), "");
        let mut highlights = Highlights::new(syntax);
        let lines = ["fun main() {", "    val s = \"hi\" // note", "}"];
        assert!(highlights.advance(3, far(), |i| lines[i]));
        let spans = highlights.spans(1).unwrap();
        assert_eq!(spans.iter().map(|s| s.len).sum::<usize>(), lines[1].len());
        let color_at = |offset: usize| {
            let mut start = 0;
            spans.iter().find(|s| {
                start += s.len;
                start > offset
            })
        };
        assert_eq!(color_at(4).unwrap().color, 0x569cd6, "val");
        assert_eq!(color_at(13).unwrap().color, 0xce9178, "string");
        assert_eq!(color_at(20).unwrap().color, 0x6a9955, "comment");
    }

    fn far() -> Instant {
        Instant::now() + std::time::Duration::from_secs(60)
    }

    #[test]
    fn highlighting_resumes_after_an_edit_and_respects_its_budget() {
        let set = load();
        let syntax = grammar_for(set, Language::Rust, Path::new("a.rs"), "");
        let mut highlights = Highlights::new(syntax);
        let lines = ["/* start", "still comment", "end */", "fn x() {}"];
        // Out of time: some lines wait for the next frame (at least a first batch is done).
        let many: Vec<String> = (0..200).map(|i| format!("let x{i} = {i};")).collect();
        let mut long = Highlights::new(syntax);
        assert!(!long.advance(200, Instant::now(), |i| many[i].as_str()));
        assert!(long.spans(14).is_some() && long.spans(199).is_none());
        assert!(long.advance(200, far(), |i| many[i].as_str()));
        assert!(highlights.advance(4, far(), |i| lines[i]));
        let comment = highlights.spans(1).unwrap()[0].color;
        assert_eq!(comment, 0x6a9955);
        highlights.invalidate_from(1);
        assert!(highlights.spans(0).is_some() && highlights.spans(1).is_none());
        let edited = ["/* start */", "not comment", "end */", "fn x() {}"];
        highlights.invalidate_from(0);
        assert!(highlights.advance(4, far(), |i| edited[i]));
        assert_ne!(highlights.spans(1).unwrap()[0].color, 0x6a9955);
    }
}

#[cfg(test)]
mod licenses {
    /// The license notices of the bundled grammars, shipped with the app (`third-party-licenses.py`).
    /// `AGENTTY_BLESS=1 cargo test` rewrites the file after a `two-face` update.
    #[test]
    fn bundled_grammar_licenses_are_listed() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/licenses/syntax-definitions.md");
        let listing = two_face::acknowledgement::listing().to_md();
        if std::env::var("AGENTTY_BLESS").as_deref() == Ok("1") {
            std::fs::write(&path, &listing).unwrap();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), listing, "run with AGENTTY_BLESS=1 to update {}", path.display());
    }
}
