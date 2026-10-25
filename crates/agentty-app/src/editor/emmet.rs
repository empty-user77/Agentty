//! Emmet abbreviations in HTML and JSX, as VS Code has them built in: `ul>li.item*3` before the
//! cursor is offered first in the completion list and expands to its markup, the empty places
//! (an element's content, `href=""`) left as tab stops. JSX writes `className` and `htmlFor` and
//! closes void elements (`<img />`).
//!
//! The common part of the syntax: a tag (or none: `div`, `li` in a list, `span` in inline
//! markup), `#id`, `.class`, `[attr=value]`, `{text}`, `*3` with `$` for the number, and `>`
//! (child), `+` (sibling), `^` (up) and `( )` (group) between them.

/// Abbreviations longer than this are not looked at.
const LONGEST: usize = 200;
/// Elements one abbreviation may make at most (`*` multiplies).
const MOST: usize = 500;

/// What an abbreviation before the cursor expands to: from byte `start` of the line, the markup
/// (tabs for its nesting, to be indented like the line) and its tab stops as byte ranges of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Expansion {
    pub start: usize,
    pub abbreviation: String,
    pub text: String,
    pub stops: Vec<(usize, usize)>,
}

const VOID: &[&str] = &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"];
const INLINE: &[&str] = &[
    "a", "abbr", "b", "bdi", "bdo", "br", "button", "cite", "code", "data", "dfn", "em", "i", "img", "input", "kbd", "label", "mark", "q",
    "s", "samp", "select", "small", "span", "strong", "sub", "sup", "textarea", "time", "u", "var", "wbr",
];
const TAGS: &[&str] = &[
    "a",
    "abbr",
    "address",
    "area",
    "article",
    "aside",
    "audio",
    "b",
    "base",
    "bdi",
    "bdo",
    "blockquote",
    "body",
    "br",
    "button",
    "canvas",
    "caption",
    "cite",
    "code",
    "col",
    "colgroup",
    "data",
    "datalist",
    "dd",
    "del",
    "details",
    "dfn",
    "dialog",
    "div",
    "dl",
    "dt",
    "em",
    "embed",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hgroup",
    "hr",
    "html",
    "i",
    "iframe",
    "img",
    "input",
    "ins",
    "kbd",
    "label",
    "legend",
    "li",
    "link",
    "main",
    "map",
    "mark",
    "menu",
    "meta",
    "meter",
    "nav",
    "noscript",
    "object",
    "ol",
    "optgroup",
    "option",
    "output",
    "p",
    "picture",
    "pre",
    "progress",
    "q",
    "s",
    "samp",
    "script",
    "search",
    "section",
    "select",
    "slot",
    "small",
    "source",
    "span",
    "strong",
    "style",
    "sub",
    "summary",
    "sup",
    "svg",
    "table",
    "tbody",
    "td",
    "template",
    "textarea",
    "tfoot",
    "th",
    "thead",
    "time",
    "title",
    "tr",
    "track",
    "u",
    "ul",
    "var",
    "video",
    "wbr",
];

/// Attributes an element comes with, empty ones to be filled in.
fn defaults(tag: &str) -> &'static [(&'static str, &'static str)] {
    match tag {
        "a" => &[("href", "")],
        "img" => &[("src", ""), ("alt", "")],
        "input" => &[("type", "text")],
        "label" => &[("for", "")],
        "form" => &[("action", "")],
        "link" => &[("rel", "stylesheet"), ("href", "")],
        "iframe" | "script" | "source" => &[("src", "")],
        _ => &[],
    }
}

#[derive(Clone, Debug, Default)]
struct Node {
    /// Empty: what its parent implies.
    name: String,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<(String, Option<String>)>,
    text: Option<String>,
    repeat: usize,
    /// A `( … )` group: its children only.
    group: bool,
    children: Vec<Node>,
}

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl Parser<'_> {
    /// Siblings and their children, up to `)` (in a group) or the end.
    fn sequence(&mut self, group: bool) -> Option<Vec<Node>> {
        // The path from the top: the node being added to at each depth.
        let mut path: Vec<Vec<Node>> = vec![Vec::new()];
        loop {
            let node = if self.chars.peek() == Some(&'(') {
                self.chars.next();
                let children = self.sequence(true)?;
                Node { group: true, children, repeat: self.repeat()?, ..Default::default() }
            } else {
                self.element()?
            };
            path.last_mut()?.push(node);
            match self.chars.next() {
                Some('>') => path.push(Vec::new()),
                Some('+') => {}
                Some('^') => {
                    let mut ups = 1;
                    while self.chars.next_if_eq(&'^').is_some() {
                        ups += 1;
                    }
                    for _ in 0..ups {
                        if path.len() > 1 {
                            close(&mut path);
                        }
                    }
                }
                Some(')') if group => break,
                None if !group => break,
                _ => return None,
            }
        }
        while path.len() > 1 {
            close(&mut path);
        }
        path.pop()
    }

    fn element(&mut self) -> Option<Node> {
        let mut node = Node { name: self.word(|c| c.is_ascii_alphanumeric() || c == '-' || c == ':'), ..Default::default() };
        loop {
            match self.chars.peek() {
                Some('#') => {
                    self.chars.next();
                    node.id = Some(self.word(name_char)).filter(|id| !id.is_empty());
                    node.id.as_ref()?;
                }
                Some('.') => {
                    self.chars.next();
                    let class = self.word(name_char);
                    if class.is_empty() {
                        return None;
                    }
                    node.classes.push(class);
                }
                Some('[') => {
                    self.chars.next();
                    node.attrs.extend(self.attributes()?);
                }
                Some('{') => {
                    self.chars.next();
                    let text: String = self.chars.by_ref().take_while(|c| *c != '}').collect();
                    node.text = Some(text);
                }
                _ => break,
            }
        }
        let empty = node.name.is_empty() && node.id.is_none() && node.classes.is_empty() && node.attrs.is_empty();
        if empty && node.text.is_none() {
            return None;
        }
        node.repeat = self.repeat()?;
        Some(node)
    }

    fn repeat(&mut self) -> Option<usize> {
        if self.chars.next_if_eq(&'*').is_none() {
            return Some(1);
        }
        self.word(|c| c.is_ascii_digit()).parse().ok().filter(|n| (1..=MOST).contains(n))
    }

    /// `name`, `name=value`, `name="a value"`, separated by spaces, up to `]`.
    fn attributes(&mut self) -> Option<Vec<(String, Option<String>)>> {
        let mut out = Vec::new();
        loop {
            while self.chars.next_if_eq(&' ').is_some() {}
            if self.chars.next_if_eq(&']').is_some() {
                return Some(out);
            }
            let name = self.word(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | ':' | '_' | '@'));
            if name.is_empty() {
                return None;
            }
            let value = if self.chars.next_if_eq(&'=').is_some() {
                Some(match self.chars.peek() {
                    Some(&quote) if quote == '"' || quote == '\'' => {
                        self.chars.next();
                        self.chars.by_ref().take_while(|c| *c != quote).collect()
                    }
                    _ => self.word(|c| c != ' ' && c != ']'),
                })
            } else {
                None
            };
            out.push((name, value));
        }
    }

    fn word(&mut self, keep: impl Fn(char) -> bool) -> String {
        let mut out = String::new();
        while let Some(c) = self.chars.next_if(|c| keep(*c)) {
            out.push(c);
        }
        out
    }
}

fn name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '$')
}

/// The innermost level is done: its nodes become the children of the last node above.
fn close(path: &mut Vec<Vec<Node>>) {
    let children = path.pop().unwrap_or_default();
    if let Some(parent) = path.last_mut().and_then(|level| level.last_mut()) {
        parent.children.extend(children);
    }
}

/// The tree with groups and repeats spelled out, `$` numbered and names implied.
fn spell(nodes: &[Node], parent: &str, out: &mut Vec<Node>, count: &mut usize) -> Option<()> {
    for node in nodes {
        for index in 1..=node.repeat {
            *count += 1;
            if *count > MOST {
                return None;
            }
            if node.group {
                spell(&node.children, parent, out, count)?;
                continue;
            }
            let number = |text: &str| numbered(text, index);
            let name = if node.name.is_empty() { implied(parent).to_string() } else { node.name.clone() };
            let mut children = Vec::new();
            spell(&node.children, &name, &mut children, count)?;
            out.push(Node {
                id: node.id.as_deref().map(number),
                classes: node.classes.iter().map(|c| number(c)).collect(),
                attrs: node.attrs.iter().map(|(n, v)| (n.clone(), v.as_deref().map(number))).collect(),
                text: node.text.as_deref().map(number),
                name,
                repeat: 1,
                group: false,
                children,
            });
        }
    }
    Some(())
}

/// `item$` → `item1`, `$$` → `01`.
fn numbered(text: &str, index: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let mut width = 1;
        while chars.next_if_eq(&'$').is_some() {
            width += 1;
        }
        out.push_str(&format!("{index:0width$}"));
    }
    out
}

fn implied(parent: &str) -> &'static str {
    match parent {
        "ul" | "ol" | "menu" => "li",
        "table" | "tbody" | "thead" | "tfoot" => "tr",
        "tr" => "td",
        "select" | "datalist" | "optgroup" => "option",
        "dl" => "dt",
        parent if INLINE.contains(&parent) => "span",
        _ => "div",
    }
}

struct Writer {
    jsx: bool,
    out: String,
    stops: Vec<(usize, usize)>,
}

impl Writer {
    fn stop(&mut self) {
        self.stops.push((self.out.len(), self.out.len()));
    }

    fn element(&mut self, node: &Node, depth: usize) {
        let jsx = self.jsx;
        let attr_name = |name: &str| match (jsx, name) {
            (true, "class") => "className".to_string(),
            (true, "for") => "htmlFor".to_string(),
            _ => name.to_string(),
        };
        self.out.push('<');
        self.out.push_str(&node.name);
        let mut attrs: Vec<(String, Option<String>)> = Vec::new();
        if let Some(id) = &node.id {
            attrs.push(("id".into(), Some(id.clone())));
        }
        if !node.classes.is_empty() {
            attrs.push(("class".into(), Some(node.classes.join(" "))));
        }
        attrs.extend(node.attrs.iter().cloned());
        for (name, value) in defaults(&node.name) {
            if !attrs.iter().any(|(n, _)| n == name) {
                attrs.push((name.to_string(), Some(value.to_string())));
            }
        }
        for (name, value) in attrs {
            self.out.push(' ');
            self.out.push_str(&attr_name(&name));
            match value.filter(|v| !v.is_empty()) {
                // An expression in JSX: `onClick={save}`.
                Some(value) if self.jsx && value.starts_with('{') && value.ends_with('}') => {
                    self.out.push('=');
                    self.out.push_str(&value);
                }
                Some(value) => {
                    self.out.push_str("=\"");
                    self.out.push_str(&value.replace('"', "&quot;"));
                    self.out.push('"');
                }
                None => {
                    self.out.push_str("=\"");
                    self.stop();
                    self.out.push('"');
                }
            }
        }
        if VOID.contains(&node.name.as_str()) {
            self.out.push_str(if self.jsx { " />" } else { ">" });
            return;
        }
        self.out.push('>');
        if let Some(text) = &node.text {
            self.out.push_str(text);
        }
        let block = node.children.iter().any(|child| !INLINE.contains(&child.name.as_str()));
        if node.children.is_empty() {
            if node.text.is_none() {
                self.stop();
            }
        } else if block {
            for child in &node.children {
                self.out.push('\n');
                self.out.push_str(&"\t".repeat(depth + 1));
                self.element(child, depth + 1);
            }
            self.out.push('\n');
            self.out.push_str(&"\t".repeat(depth));
        } else {
            for child in &node.children {
                self.element(child, depth);
            }
        }
        self.out.push_str("</");
        self.out.push_str(&node.name);
        self.out.push('>');
    }
}

/// The abbreviation ending at byte `col` of `line`, and what it expands to. `None` when the text
/// there is not one: not at the start of the line or right after a tag (a word at the end of a
/// sentence stays a word), or naming an element that is neither HTML nor, in JSX, a component
/// (`Cart>p`). A plain word is one only when it is a tag.
pub fn expand_at(line: &str, col: usize, jsx: bool) -> Option<Expansion> {
    let before = line.get(..col)?;
    let start = abbreviation_start(before)?;
    // At the start of the line, or right after a tag's `>`.
    let prefix = &before[..start];
    if !(prefix.trim().is_empty() || (prefix.ends_with('>') && !prefix.ends_with("=>") && prefix.contains('<'))) {
        return None;
    }
    let abbreviation = &before[start..];
    if abbreviation.is_empty() || abbreviation.len() > LONGEST {
        return None;
    }
    let first = abbreviation.chars().next()?;
    if !(first.is_ascii_alphabetic() || matches!(first, '.' | '#' | '(' | '[' | '{')) {
        return None;
    }
    let tree = Parser { chars: abbreviation.chars().peekable() }.sequence(false)?;
    let mut nodes = Vec::new();
    spell(&tree, "", &mut nodes, &mut 0)?;
    let known = |node: &Node| TAGS.contains(&node.name.as_str()) || (jsx && node.name.starts_with(|c: char| c.is_ascii_uppercase()));
    // A name alone is an abbreviation only when it is HTML's (`Total` typed as text stays text).
    let plain = abbreviation.chars().all(|c| c.is_ascii_alphanumeric());
    if !all(&nodes, &known) || (plain && !TAGS.contains(&abbreviation)) {
        return None;
    }
    let mut writer = Writer { jsx, out: String::new(), stops: Vec::new() };
    for (i, node) in nodes.iter().enumerate() {
        if i > 0 {
            writer.out.push('\n');
        }
        writer.element(node, 0);
    }
    let end = writer.out.len();
    writer.stops.push((end, end));
    Some(Expansion { start, abbreviation: abbreviation.to_string(), text: writer.out, stops: writer.stops })
}

fn all(nodes: &[Node], known: &impl Fn(&Node) -> bool) -> bool {
    nodes.iter().all(|node| known(node) && all(&node.children, known))
}

/// Where the abbreviation before the cursor starts: after the last space outside `[ ]` and
/// `{ }`, or after the `>` of the last tag before it if that is later. Inside a tag, nowhere.
fn abbreviation_start(before: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in before.char_indices().rev() {
        match c {
            ']' | '}' => depth += 1,
            '[' | '{' => depth -= 1,
            c if depth == 0 && c.is_whitespace() => {
                start = i + c.len_utf8();
                break;
            }
            _ => {}
        }
        if depth < 0 {
            return None;
        }
    }
    if let Some(open) = before.rfind('<') {
        let close = open + before[open..].find('>')?;
        start = start.max(close + 1);
    }
    Some(start)
}

/// Whether the cursor is in a `<script>` or `<style>` element of an HTML file, from the text
/// before it, last line first: no markup there.
pub fn in_script_or_style<'a>(lines: impl Iterator<Item = &'a str>) -> bool {
    for line in lines {
        let lower = line.to_ascii_lowercase();
        let open = lower.rfind("<script").max(lower.rfind("<style"));
        let close = lower.rfind("</script").max(lower.rfind("</style"));
        if open.is_some() || close.is_some() {
            return open > close;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand(abbreviation: &str, jsx: bool) -> Option<(String, Vec<(usize, usize)>)> {
        let line = format!("    {abbreviation}");
        expand_at(&line, line.len(), jsx).map(|e| (e.text, e.stops))
    }

    #[test]
    fn expands_tags_classes_children_and_siblings() {
        let (text, stops) = expand("div.card>h2+p", true).unwrap();
        assert_eq!(text, "<div className=\"card\">\n\t<h2></h2>\n\t<p></p>\n</div>");
        assert_eq!(stops.len(), 3, "in h2, in p, at the end");
        assert_eq!(&text[..stops[0].0], "<div className=\"card\">\n\t<h2>");
        assert_eq!(expand("div.card>h2+p", false).unwrap().0, "<div class=\"card\">\n\t<h2></h2>\n\t<p></p>\n</div>");
        // Repeats, numbers, implied names, ids.
        assert_eq!(
            expand("ul#list>li.item$*3", false).unwrap().0,
            "<ul id=\"list\">\n\t<li class=\"item1\"></li>\n\t<li class=\"item2\"></li>\n\t<li class=\"item3\"></li>\n</ul>"
        );
        assert_eq!(expand(".box", true).unwrap().0, "<div className=\"box\"></div>");
        assert_eq!(expand("ul>.x", false).unwrap().0, "<ul>\n\t<li class=\"x\"></li>\n</ul>");
    }

    #[test]
    fn expands_attributes_text_groups_and_climbing() {
        assert_eq!(expand("a", false).unwrap(), ("<a href=\"\"></a>".into(), vec![(9, 9), (11, 11), (15, 15)]));
        assert_eq!(expand("img", true).unwrap().0, "<img src=\"\" alt=\"\" />");
        assert_eq!(expand("label+input[type=checkbox]", true).unwrap().0, "<label htmlFor=\"\"></label>\n<input type=\"checkbox\" />");
        assert_eq!(expand("p{Hello}", false).unwrap().0, "<p>Hello</p>");
        assert_eq!(expand("button[type=\"submit\" disabled]", false).unwrap().0, "<button type=\"submit\" disabled=\"\"></button>");
        assert_eq!(expand("p>span+em", false).unwrap().0, "<p><span></span><em></em></p>", "inline children stay on the line");
        assert_eq!(expand("header>nav^main", false).unwrap().0, "<header>\n\t<nav></nav>\n</header>\n<main></main>");
        assert_eq!(expand("(dt+dd)*2", false).unwrap().0, "<dt></dt>\n<dd></dd>\n<dt></dt>\n<dd></dd>");
        assert_eq!(expand("Card>p", true).unwrap().0, "<Card>\n\t<p></p>\n</Card>", "a component");
    }

    #[test]
    fn plain_words_and_code_are_not_abbreviations() {
        for text in ["count", "useState", "a.b.", "div>", "x+y", "li*0", "Card", "Total", "1div", "div.card>"] {
            assert_eq!(expand(text, true), None, "{text}");
        }
        assert!(expand("Card.x", true).is_some() && expand("Card.x", false).is_none(), "a component in JSX only");
        // At the start of the line or after a tag on it; not after other text or in a tag.
        assert_eq!(expand_at("<div>span.x", 11, true).map(|e| (e.start, e.text)), Some((5, "<span className=\"x\"></span>".into())));
        assert_eq!(expand_at("Click the span", 14, true), None);
        assert_eq!(expand_at("<input type=\"text\" p", 21, true), None);
        assert_eq!(expand_at("const f = () => p", 17, true), None);
        assert_eq!(expand_at("[1, 2]", 6, false), None);
        assert_eq!(expand_at("a > p", 5, false), None, "a `>` in text is no tag");
        assert_eq!(expand_at("<div class=\"x\">ul>li", 20, false).map(|e| e.start), Some(15), "after a tag with attributes");
        // Values: quotes kept apart, an expression in JSX.
        assert_eq!(expand("a[title='say \"hi\"']", false).unwrap().0, "<a title=\"say &quot;hi&quot;\" href=\"\"></a>");
        assert_eq!(expand("button[onClick={save}]", true).unwrap().0, "<button onClick={save}></button>");
        assert!(in_script_or_style(["  body", "<script>"].into_iter()));
        assert!(!in_script_or_style(["  body", "</script>", "<script src=\"x\">"].into_iter()));
    }
}
