//! Settings → About → Open source: one page listing everything Agentty is built with — the fonts,
//! icons and scripts it bundles and every crate linked into it — each with its version, license and
//! home page. The list is `assets/licenses/open-source.tsv`, written by
//! `scripts/third-party-licenses.py --list` (CI checks it is current); the full license texts ship
//! with the app as `THIRD_PARTY_LICENSES.txt` (macOS) and are in the repository otherwise.

use super::settings_page::SettingsSection;
use super::Workbench;
use crate::i18n::{t, tf};
use crate::theme::{hex, Chrome};
use crate::ui::TypeScale;
use gpui::{div, prelude::*, px, ClickEvent, Context, Div, SharedString};
use std::sync::LazyLock;

const LIST: &str = include_str!("../../assets/licenses/open-source.tsv");
const SOURCE_URL: &str = "https://github.com/empty-user77/Agentty";
const NOTICES_URL: &str = "https://github.com/empty-user77/Agentty/blob/main/THIRD_PARTY_NOTICES.md";

#[derive(Debug, Clone, PartialEq)]
struct Component {
    bundled: bool,
    name: &'static str,
    version: &'static str,
    license: &'static str,
    home: &'static str,
}

fn parse(list: &'static str) -> Vec<Component> {
    list.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let (kind, name, version, license, home) = (fields.next()?, fields.next()?, fields.next()?, fields.next()?, fields.next()?);
            Some(Component { bundled: kind == "asset", name, version, license, home })
        })
        .collect()
}

static COMPONENTS: LazyLock<Vec<Component>> = LazyLock::new(|| parse(LIST));

/// The license texts in full: the file the release bundle carries, else the repository's notices.
fn open_full_texts(cx: &mut gpui::App) {
    let bundled = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("../Resources/THIRD_PARTY_LICENSES.txt")))
        .filter(|path| path.is_file());
    match bundled {
        Some(path) => cx.open_with_system(&path),
        None => cx.open_url(NOTICES_URL),
    }
}

/// A library on the list: one line, whatever number of its versions the app links.
struct Library {
    name: &'static str,
    /// Its versions, oldest first: `0.1 / 0.2`.
    versions: String,
    /// The licenses of those versions, each named once.
    license: String,
    home: &'static str,
}

/// The components grouped by name, in the list's order.
fn libraries(components: impl Iterator<Item = &'static Component>) -> Vec<Library> {
    let mut libraries: Vec<Library> = Vec::new();
    for component in components {
        match libraries.iter_mut().find(|l| l.name == component.name) {
            Some(library) => {
                if !component.version.is_empty() {
                    library.versions.push_str(" / ");
                    library.versions.push_str(component.version);
                }
                if !library.license.split(" / ").any(|l| l == component.license) {
                    library.license.push_str(" / ");
                    library.license.push_str(component.license);
                }
            }
            None => libraries.push(Library {
                name: component.name,
                versions: component.version.to_string(),
                license: component.license.to_string(),
                home: component.home,
            }),
        }
    }
    libraries
}

/// One line of the list: a group's title (with how many libraries it has), or a library.
enum Line {
    Heading { bundled: bool, count: usize },
    Item(Library),
}

/// The list's lines: what the app bundles, then the crates, each under its title.
static LINES: LazyLock<Vec<Line>> = LazyLock::new(|| {
    let mut lines = Vec::new();
    for bundled in [true, false] {
        let group = libraries(COMPONENTS.iter().filter(|c| c.bundled == bundled));
        lines.push(Line::Heading { bundled, count: group.len() });
        lines.extend(group.into_iter().map(Line::Item));
    }
    lines
});

/// Every line has this height, so the list draws only the lines in view (`uniform_list`).
const LINE_HEIGHT: f32 = 30.;

fn line(index: usize, cx: &gpui::App) -> gpui::AnyElement {
    let base = div().w_full().h(px(LINE_HEIGHT)).flex().items_center().gap_3().px_2();
    match &LINES[index] {
        Line::Heading { bundled, count } => {
            let title = if *bundled {
                t(cx, "open_source.bundled").to_string()
            } else {
                tf(cx, "open_source.crates", &[("n", &count.to_string())])
            };
            base.items_end()
                .pb_1()
                .border_b_1()
                .border_color(hex(Chrome::BORDER))
                .child(div().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(title))
                .into_any_element()
        }
        Line::Item(library) => {
            let home = library.home;
            base.id(SharedString::from(format!("open-source-{index}")))
                .rounded_sm()
                .cursor_pointer()
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .on_click(move |_: &ClickEvent, _, cx| cx.open_url(home))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap_2()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().t_small().text_color(hex(Chrome::FOREGROUND)).child(library.name))
                        .child(div().min_w_0().truncate().t_caption().text_color(hex(Chrome::MUTED)).child(library.versions.clone())),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .max_w(px(280.))
                        .truncate()
                        .t_caption()
                        .text_color(hex(Chrome::MUTED))
                        .child(library.license.clone()),
                )
                .into_any_element()
        }
    }
}

impl Workbench {
    /// The top of the page, above the list: back to About, a word of thanks, the full texts.
    pub(super) fn render_open_source(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .child(div().pb_5().flex().child(crate::ui::action_button_with_icon(
                "open-source-back",
                "arrow-left",
                t(cx, "settings.about"),
                cx.listener(|this, _: &ClickEvent, _, cx| this.open_settings_section(SettingsSection::About, cx)),
            )))
            .child(div().pb_4().t_body().text_color(hex(Chrome::FOREGROUND)).child(t(cx, "open_source.intro")))
            .child(
                div()
                    .pb_4()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(crate::ui::action_button_with_icon(
                        "open-source-texts",
                        "file-text",
                        t(cx, "open_source.full_texts"),
                        |_, _, cx| open_full_texts(cx),
                    ))
                    .child(crate::ui::action_button_with_icon("open-source-repo", "code", t(cx, "open_source.source"), |_, _, cx| {
                        cx.open_url(SOURCE_URL)
                    })),
            )
    }
}

/// Every component, drawn a screenful at a time: the list has some 760 lines.
pub(super) fn open_source_list() -> gpui::UniformList {
    gpui::uniform_list("open-source-list", LINES.len(), |range, _, cx| range.map(|index| line(index, cx)).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_line_is_a_component() {
        let lines = LIST.lines().filter(|l| !l.is_empty() && !l.starts_with('#')).count();
        assert_eq!(COMPONENTS.len(), lines, "a line of open-source.tsv does not have five fields");
        assert!(COMPONENTS.iter().all(|c| !c.name.is_empty() && !c.license.is_empty() && c.home.starts_with("http")));
        assert!(COMPONENTS.iter().any(|c| c.bundled && c.name == "JetBrains Mono"));
        assert!(COMPONENTS.iter().any(|c| !c.bundled && c.name == "agentty-gpui"));
    }

    /// Every crate on the list is the version Cargo.lock has: the list follows dependency updates
    /// (CI also runs `scripts/third-party-licenses.py --list --check`, which catches added crates).
    #[test]
    fn crates_match_the_lockfile() {
        // A Windows checkout may have CRLF line ends.
        let lock = include_str!("../../../../Cargo.lock").replace('\r', "");
        for component in COMPONENTS.iter().filter(|c| !c.bundled) {
            let entry = format!("name = \"{}\"\nversion = \"{}\"", component.name, component.version);
            assert!(
                lock.contains(&entry),
                "{} {} is not in Cargo.lock: run scripts/third-party-licenses.py --list",
                component.name,
                component.version
            );
        }
    }

    #[test]
    fn versions_of_a_library_share_a_line() {
        let list: &'static str = "crate\tabc\t0.1.0\tMIT\thttps://a\ncrate\tabc\t0.2.0\tMIT OR Apache-2.0\thttps://a\ncrate\tabc\t0.3.0\tMIT\thttps://a\ncrate\txyz\t1.0.0\tMIT\thttps://x\n";
        let components: &'static [Component] = Box::leak(parse(list).into_boxed_slice());
        let grouped = libraries(components.iter());
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].versions, "0.1.0 / 0.2.0 / 0.3.0");
        assert_eq!(grouped[0].license, "MIT / MIT OR Apache-2.0");
        assert_eq!(grouped[1].versions, "1.0.0");
    }

    #[test]
    fn parses_fields() {
        let parsed =
            parse("# header\nasset\tFont\t1.0\tOFL-1.1\thttps://example.com\ncrate\tserde\t1.0.0\tMIT OR Apache-2.0\thttps://serde.rs\n");
        assert_eq!(parsed.len(), 2);
        assert!(parsed[0].bundled);
        assert_eq!(
            parsed[1],
            Component { bundled: false, name: "serde", version: "1.0.0", license: "MIT OR Apache-2.0", home: "https://serde.rs" }
        );
    }
}
