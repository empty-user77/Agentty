//! What a bot is made of, as its settings show it: its prompt, its rules, its context and its
//! tools. The first three are text the user writes; the tools are its permission and its skills,
//! which come from the skills hub (the ones that ship with Agentty, and the user's own).

use super::Workbench;
use crate::i18n::{t, tf};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, Chrome};
use crate::ui::{icon, TypeScale as _};
use agentty_bridge::bots::skills::{self, Category, Skill, BUNDLED, CATEGORIES};
use agentty_bridge::bots::{Bot, BotPermission, BotStore, Part, StoreError, PARTS};
use gpui::{
    div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, Focusable as _, ScrollHandle, SharedString, Subscription, Window,
};
use std::path::PathBuf;

/// Lines of a part shown on its card; the rest is in its editor.
const PREVIEW_LINES: usize = 5;
/// Lines of the bot's memory shown with its context.
const MEMORY_LINES: usize = 4;
/// The rows a part's editor starts with and grows to.
const EDIT_ROWS: (usize, usize) = (8, 22);

/// The names and descriptions of the bundled skills, as the hub shows them (`i18n.rs`).
const SKILL_TEXT: [(&str, &str, &str); 19] = [
    ("computer-control", "bots.skill.computer-control", "bots.skill.computer-control.about"),
    ("messenger-desk", "bots.skill.messenger-desk", "bots.skill.messenger-desk.about"),
    ("phone-mirroring", "bots.skill.phone-mirroring", "bots.skill.phone-mirroring.about"),
    ("daily-brief", "bots.skill.daily-brief", "bots.skill.daily-brief.about"),
    ("reminder-keeper", "bots.skill.reminder-keeper", "bots.skill.reminder-keeper.about"),
    ("file-finder", "bots.skill.file-finder", "bots.skill.file-finder.about"),
    ("folder-tidy", "bots.skill.folder-tidy", "bots.skill.folder-tidy.about"),
    ("inbox-triage", "bots.skill.inbox-triage", "bots.skill.inbox-triage.about"),
    ("meeting-notes", "bots.skill.meeting-notes", "bots.skill.meeting-notes.about"),
    ("writing-editor", "bots.skill.writing-editor", "bots.skill.writing-editor.about"),
    ("translate-natural", "bots.skill.translate-natural", "bots.skill.translate-natural.about"),
    ("web-research", "bots.skill.web-research", "bots.skill.web-research.about"),
    ("news-digest", "bots.skill.news-digest", "bots.skill.news-digest.about"),
    ("price-compare", "bots.skill.price-compare", "bots.skill.price-compare.about"),
    ("study-coach", "bots.skill.study-coach", "bots.skill.study-coach.about"),
    ("idea-partner", "bots.skill.idea-partner", "bots.skill.idea-partner.about"),
    ("pr-review-checklist", "bots.skill.pr-review-checklist", "bots.skill.pr-review-checklist.about"),
    ("bug-reproduction", "bots.skill.bug-reproduction", "bots.skill.bug-reproduction.about"),
    ("ci-failure-triage", "bots.skill.ci-failure-triage", "bots.skill.ci-failure-triage.about"),
];

fn category_key(category: Category) -> &'static str {
    match category {
        Category::Control => "bots.cat.control",
        Category::Daily => "bots.cat.daily",
        Category::Work => "bots.cat.work",
        Category::Research => "bots.cat.research",
        Category::Learning => "bots.cat.learning",
        Category::Coding => "bots.cat.coding",
    }
}

fn part_index(part: Part) -> usize {
    PARTS.iter().position(|known| *known == part).unwrap_or(0)
}

/// A part's title, the line under it, the hint of its empty editor, and its glyph.
fn part_text(part: Part) -> (&'static str, &'static str, &'static str, &'static str) {
    match part {
        Part::Prompt => ("bots.part.prompt", "bots.part.prompt.about", "bots.part.prompt.hint", "pencil"),
        Part::Rules => ("bots.part.rules", "bots.part.rules.about", "bots.part.rules.hint", "file-text"),
        Part::Context => ("bots.part.context", "bots.part.context.about", "bots.part.context.hint", "circle-user"),
    }
}

/// The part being written, in its card.
pub(super) struct PartEdit {
    part: Part,
    input: Entity<TextInput>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HubFilter {
    All,
    Category(Category),
    Mine,
}

/// The skills hub, open for one bot.
pub(super) struct SkillHub {
    handle: String,
    search: Entity<TextInput>,
    filter: HubFilter,
    /// The user's own skills, as found when the hub was opened.
    local: Vec<Skill>,
    scroll: ScrollHandle,
    _subscription: Subscription,
}

/// Where a skill in the hub comes from.
#[derive(Clone)]
enum Source {
    Bundled(&'static str),
    Local(PathBuf),
}

struct HubItem {
    /// The name it has in a bot.
    name: String,
    title: String,
    about: String,
    tag: String,
    source: Source,
}

impl Workbench {
    /// A skill's name as the user reads it: translated for the bundled ones, its own otherwise.
    fn skill_title(&self, name: &str, cx: &gpui::App) -> String {
        SKILL_TEXT.iter().find(|(known, _, _)| *known == name).map_or_else(|| name.to_string(), |(_, title, _)| t(cx, title).to_string())
    }

    /// Reads what the open bot is made of: its parts, its skills, its memory. In the background,
    /// for the settings to draw from.
    pub(super) fn load_bot_profile(&mut self, handle: &str, cx: &mut Context<Self>) {
        let wanted = handle.to_string();
        cx.spawn(async move |this, cx| {
            let read = wanted.clone();
            let (parts, skills, memory) = cx
                .background_spawn(async move {
                    let store = crate::bots::store();
                    (
                        PARTS.map(|part| store.part(&read, part).unwrap_or_default()),
                        store.skills(&read),
                        store.memory(&read).unwrap_or_default(),
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.bots.profile.as_deref() == Some(wanted.as_str()) {
                    this.bots.parts = parts;
                    this.bots.skills = skills;
                    this.bots.profile_memory = memory;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Changes the open bot's files in the background, then reads them again.
    fn change_bot_files(
        &mut self,
        handle: &str,
        change: impl FnOnce(&BotStore, &str) -> Result<(), StoreError> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let wanted = handle.to_string();
        cx.spawn(async move |this, cx| {
            let changed = wanted.clone();
            let done = cx.background_spawn(async move { change(&crate::bots::store(), &changed) }).await;
            let _ = this.update(cx, |this, cx| {
                match done {
                    // The agent reads them when a conversation starts.
                    Ok(()) => this.show_toast(t(cx, "bots.applies_next"), cx),
                    Err(_) => this.show_toast(t(cx, "bots.error_save"), cx),
                }
                this.load_bot_profile(&wanted, cx);
            });
        })
        .detach();
    }

    pub(super) fn edit_bot_part(&mut self, part: Part, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.bots.parts[part_index(part)].clone();
        let hint = t(cx, part_text(part).2);
        let input = cx.new(|cx| TextInput::new(text, hint, window, cx).multiline(EDIT_ROWS.0).growing(EDIT_ROWS.1));
        window.focus(&input.focus_handle(cx), cx);
        self.bots.part_edit = Some(PartEdit { part, input });
        cx.notify();
    }

    fn save_bot_part(&mut self, cx: &mut Context<Self>) {
        let (Some(edit), Some(handle)) = (self.bots.part_edit.take(), self.bots.profile.clone()) else { return };
        let text = edit.input.read(cx).text().trim().to_string();
        let part = edit.part;
        // Shown at once; the file follows.
        self.bots.parts[part_index(part)] = text.clone();
        self.change_bot_files(&handle, move |store, handle| store.write_part(handle, part, &text), cx);
        cx.notify();
    }

    fn add_bot_skill(&mut self, source: Source, cx: &mut Context<Self>) {
        let Some(handle) = self.bots.profile.clone() else { return };
        self.change_bot_files(
            &handle,
            move |store, handle| match source {
                Source::Bundled(name) => store.add_skill(handle, name),
                Source::Local(dir) => store.add_skill_from(handle, &dir).map(|_| ()),
            },
            cx,
        );
    }

    /// The debug driver's click on a bundled skill's button in the hub.
    pub(super) fn toggle_bundled_skill(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(skill) = skills::bundled(name) else { return };
        if self.bots.skills.iter().any(|known| known.name == name) {
            self.remove_bot_skill(name, cx);
        } else {
            self.add_bot_skill(Source::Bundled(skill.name), cx);
        }
    }

    /// The debug driver's typing into the open editor, and its click on "Save".
    pub(super) fn debug_save_bot_part(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(edit) = &self.bots.part_edit {
            let text = text.replace("\\n", "\n");
            edit.input.update(cx, |input, cx| input.set_text(text, cx));
        }
        self.save_bot_part(cx);
    }

    pub(super) fn remove_bot_skill(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(handle) = self.bots.profile.clone() else { return };
        let name = name.to_string();
        self.bots.skills.retain(|skill| skill.name != name);
        self.change_bot_files(&handle, move |store, handle| store.remove_skill(handle, &name), cx);
        cx.notify();
    }

    // -- the four cards ------------------------------------------------------------------------

    fn part_card(&self, glyph: &'static str, title: &'static str, about: &'static str, wide: bool, cx: &gpui::App) -> gpui::Div {
        div()
            .min_w(px(300.))
            .when(wide, |d| d.w_full())
            .when(!wide, |d| d.flex_1().flex_basis(px(340.)))
            .p_4()
            .flex()
            .flex_col()
            .gap_2p5()
            .rounded_xl()
            .bg(hex(Chrome::PANEL))
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_none()
                            .size(px(36.))
                            .rounded_full()
                            .bg(crate::theme::hex_alpha(Chrome::ACCENT, 0.16))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(glyph, 17., hex(Chrome::BRIGHT))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, title)))
                            .child(div().t_caption().text_color(hex(Chrome::MUTED)).truncate().child(t(cx, about))),
                    ),
            )
    }

    /// Prompt, rules, context and tools: what the bot is made of.
    pub(super) fn render_bot_parts(&mut self, bot: &Bot, cx: &mut Context<Self>) -> AnyElement {
        let handle = bot.handle.clone();
        let mut grid = div().flex().flex_wrap().gap_3();
        let editing = self.bots.part_edit.as_ref().map(|edit| (edit.part, edit.input.clone()));
        for part in PARTS {
            let (title, about, _, glyph) = part_text(part);
            let text = self.bots.parts[part_index(part)].clone();
            let input = editing.as_ref().filter(|(edited, _)| *edited == part).map(|(_, input)| input.clone());
            let mut card = self.part_card(glyph, title, about, input.is_some(), cx);
            match input {
                Some(input) => {
                    card = card
                        .child(
                            div()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .bg(hex(Chrome::EDITOR))
                                .border_1()
                                .border_color(hex(Chrome::ACCENT))
                                .t_body()
                                .text_color(hex(Chrome::BRIGHT))
                                .child(input),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(crate::ui::action_button(
                                    ("bot-part-cancel", part_index(part)),
                                    t(cx, "confirm.cancel"),
                                    cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.bots.part_edit = None;
                                        cx.notify();
                                    }),
                                ))
                                .child(crate::ui::action_button(
                                    ("bot-part-save", part_index(part)),
                                    t(cx, "bots.part.save"),
                                    cx.listener(|this, _: &ClickEvent, _, cx| this.save_bot_part(cx)),
                                )),
                        );
                }
                None => {
                    let lines: Vec<String> =
                        text.lines().filter(|line| !line.trim().is_empty()).take(PREVIEW_LINES).map(str::to_string).collect();
                    let more = text.lines().filter(|line| !line.trim().is_empty()).count() > lines.len();
                    let mut preview = div().flex().flex_col().gap_0p5();
                    for line in lines {
                        preview = preview.child(div().t_small().truncate().text_color(hex(Chrome::FOREGROUND)).child(line));
                    }
                    if more {
                        preview = preview.child(div().t_small().text_color(hex(Chrome::MUTED)).child("…"));
                    }
                    card = card.child(preview);
                    if part == Part::Context {
                        // What the bot itself wrote down belongs to what it knows.
                        let remembered: Vec<String> = self
                            .bots
                            .profile_memory
                            .lines()
                            .filter(|line| !line.trim().is_empty())
                            .take(MEMORY_LINES)
                            .map(str::to_string)
                            .collect();
                        let mut memory = div().pt_1().flex().flex_col().gap_0p5().child(
                            div()
                                .t_caption()
                                .font_weight(crate::theme::EMPHASIS)
                                .text_color(hex(Chrome::MUTED))
                                .child(t(cx, "bots.memory")),
                        );
                        if remembered.is_empty() {
                            memory = memory.child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "bots.memory_none")));
                        }
                        for line in remembered {
                            memory = memory.child(div().t_small().truncate().text_color(hex(Chrome::FOREGROUND)).child(line));
                        }
                        card = card.child(memory);
                    }
                    let mut actions = div().flex().items_center().gap_2().child(crate::ui::action_button_with_icon(
                        ("bot-part-edit", part_index(part)),
                        "pencil",
                        t(cx, if text.trim().is_empty() { "bots.part.write" } else { "bots.part.edit" }),
                        cx.listener(move |this, _: &ClickEvent, window, cx| this.edit_bot_part(part, window, cx)),
                    ));
                    if let (Part::Context, Ok(path)) = (part, crate::bots::store().memory_path(&handle)) {
                        let folder = bot.folder.clone();
                        actions = actions.child(crate::ui::action_button(
                            "bot-memory-open",
                            "MEMORY.md",
                            cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.page = None;
                                this.open_in_editor(&path, &folder, window, cx);
                            }),
                        ));
                    }
                    card = card.child(div().flex_1()).child(actions);
                }
            }
            grid = grid.child(card);
        }

        // Tools: what it may do, and the skills it does it with.
        let mut tools = self.part_card("wrench", "bots.part.tools", "bots.part.tools.about", false, cx);
        let mut permissions = div().flex().flex_wrap().gap_1p5();
        for (permission, key) in [
            (BotPermission::Ask, "bots.permission_ask"),
            (BotPermission::ReadOnly, "bots.permission_read"),
            (BotPermission::Files, "bots.permission_files"),
        ] {
            let handle = handle.clone();
            permissions = permissions.child(crate::ui::chip(
                SharedString::from(format!("bot-permission-{key}")),
                t(cx, key),
                bot.permission == permission,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.change_bot(&handle, |bot| bot.permission = permission, cx);
                }),
            ));
        }
        let mut chips = div().flex().flex_wrap().gap_1p5();
        if self.bots.skills.is_empty() {
            chips = chips.child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "bots.skills_none")));
        }
        for (index, skill) in self.bots.skills.iter().enumerate() {
            let name = skill.name.clone();
            chips = chips.child(
                div()
                    .pl_2p5()
                    .pr_1()
                    .py_0p5()
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_full()
                    .bg(hex(Chrome::HOVER))
                    .t_small()
                    .text_color(hex(Chrome::BRIGHT))
                    .child(icon("sparkles", 12., hex(Chrome::FOREGROUND)))
                    .child(self.skill_title(&skill.name, cx))
                    .child(crate::ui::icon_only_sized(
                        ("bot-skill-remove", index),
                        "x",
                        18.,
                        11.,
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.remove_bot_skill(&name, cx)),
                    )),
            );
        }
        let hub_handle = handle.clone();
        tools = tools.child(permissions).child(chips).child(div().flex_1()).child(div().flex().child(crate::ui::action_button_with_icon(
            "bot-skill-hub",
            "blocks",
            t(cx, "bots.hub"),
            cx.listener(move |this, _: &ClickEvent, window, cx| this.open_skill_hub(&hub_handle, window, cx)),
        )));
        grid.child(tools).into_any_element()
    }

    // -- the skills hub ------------------------------------------------------------------------

    pub(super) fn open_skill_hub(&mut self, handle: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.bots.get(handle).is_none() {
            return;
        }
        let search = cx.new(|cx| TextInput::localized("", "bots.hub.search", window, cx));
        let subscription = cx.subscribe_in(&search, window, |this, _, event: &TextInputEvent, _, cx| match event {
            TextInputEvent::Changed => cx.notify(),
            TextInputEvent::Cancelled => this.close_skill_hub(cx),
            _ => {}
        });
        window.focus(&search.focus_handle(cx), cx);
        self.bots.hub = Some(SkillHub {
            handle: handle.to_string(),
            search,
            filter: HubFilter::All,
            local: Vec::new(),
            scroll: ScrollHandle::new(),
            _subscription: subscription,
        });
        // The user's own skills are looked for once, off the main thread.
        cx.spawn(async move |this, cx| {
            let local = cx.background_spawn(async move { skills::local() }).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(hub) = this.bots.hub.as_mut() {
                    hub.local = local;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn close_skill_hub(&mut self, cx: &mut Context<Self>) {
        self.bots.hub = None;
        cx.notify();
    }

    fn hub_items(&self, hub: &SkillHub, filter: HubFilter, cx: &gpui::App) -> Vec<HubItem> {
        let mut items = Vec::new();
        if filter != HubFilter::Mine {
            for skill in BUNDLED.iter().filter(|skill| filter == HubFilter::All || filter == HubFilter::Category(skill.category)) {
                let Some((_, title, about)) = SKILL_TEXT.iter().find(|(name, _, _)| *name == skill.name) else { continue };
                items.push(HubItem {
                    name: skill.name.to_string(),
                    title: t(cx, title).to_string(),
                    about: t(cx, about).to_string(),
                    tag: t(cx, category_key(skill.category)).to_string(),
                    source: Source::Bundled(skill.name),
                });
            }
        }
        if matches!(filter, HubFilter::All | HubFilter::Mine) {
            for skill in &hub.local {
                let name = skills::slug(&skill.name);
                // One the bundle already has under that name is the bundle's.
                if name.is_empty() || skills::bundled(&name).is_some() {
                    continue;
                }
                items.push(HubItem {
                    name,
                    title: skill.name.clone(),
                    about: skill.description.clone(),
                    tag: t(cx, "bots.hub.mine").to_string(),
                    source: Source::Local(skill.dir.clone()),
                });
            }
        }
        let query = hub.search.read(cx).text().trim().to_lowercase();
        if !query.is_empty() {
            items.retain(|item| [&item.name, &item.title, &item.about, &item.tag].iter().any(|text| text.to_lowercase().contains(&query)));
        }
        items
    }

    pub(super) fn render_skill_hub(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let hub = self.bots.hub.as_ref()?;
        let bot = self.bots.get(&hub.handle)?;
        let has = |name: &str| self.bots.skills.iter().any(|skill| skill.name == name);
        let all = self.hub_items(hub, HubFilter::All, cx);
        let shown = self.hub_items(hub, hub.filter, cx);
        let added = self.bots.skills.len();

        let stat = |number: usize, label: String, color: u32| {
            div()
                .flex()
                .flex_col()
                .child(div().text_size(px(22.)).font_weight(crate::theme::EMPHASIS).text_color(hex(color)).child(number.to_string()))
                .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(label))
        };
        let header = div()
            .flex()
            .items_start()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, "bots.hub")))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .t_small()
                            .text_color(hex(Chrome::MUTED))
                            .child(super::bot_face::tile(bot, 18.))
                            .child(tf(cx, "bots.hub.for", &[("name", &bot.name)])),
                    ),
            )
            .child(crate::ui::icon_only("skill-hub-close", "x", cx.listener(|this, _: &ClickEvent, _, cx| this.close_skill_hub(cx))));
        let stats = div()
            .flex()
            .gap_8()
            .child(stat(BUNDLED.len(), t(cx, "bots.hub.bundled").to_string(), Chrome::BRIGHT))
            .child(stat(hub.local.len(), t(cx, "bots.hub.mine").to_string(), Chrome::ATTENTION))
            .child(stat(added, t(cx, "bots.hub.added").to_string(), Chrome::SUCCESS));
        let search = div()
            .px_3()
            .py_1p5()
            .flex()
            .items_center()
            .gap_2()
            .rounded_md()
            .bg(hex(Chrome::EDITOR))
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .child(icon("search", 14., hex(Chrome::MUTED)))
            .child(div().flex_1().min_w_0().t_body().text_color(hex(Chrome::BRIGHT)).child(hub.search.clone()));

        let mut filters = div().flex_none().w(px(164.)).flex().flex_col().gap_px();
        let mut rows = vec![(HubFilter::All, t(cx, "bots.hub.all").to_string(), all.len())];
        for category in CATEGORIES {
            rows.push((
                HubFilter::Category(category),
                t(cx, category_key(category)).to_string(),
                BUNDLED.iter().filter(|skill| skill.category == category).count(),
            ));
        }
        rows.push((HubFilter::Mine, t(cx, "bots.hub.mine").to_string(), hub.local.len()));
        for (index, (filter, label, count)) in rows.into_iter().enumerate() {
            let picked = hub.filter == filter;
            filters = filters.child(
                div()
                    .id(("skill-hub-filter", index))
                    .px_2p5()
                    .py_1p5()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .t_small()
                    .when(picked, |d| d.bg(hex(Chrome::SELECTED)))
                    .text_color(hex(if picked { Chrome::BRIGHT } else { Chrome::FOREGROUND }))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(div().flex_none().t_caption().text_color(hex(Chrome::MUTED)).child(count.to_string()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(hub) = this.bots.hub.as_mut() {
                            hub.filter = filter;
                        }
                        cx.notify();
                    })),
            );
        }

        let mut cards = div().flex().flex_wrap().gap_2();
        if shown.is_empty() {
            cards = cards.child(div().p_4().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "bots.hub.none")));
        }
        for (index, item) in shown.into_iter().enumerate() {
            let installed = has(&item.name);
            let (name, source) = (item.name.clone(), item.source.clone());
            let button = div()
                .id(("skill-hub-add", index))
                .flex_none()
                .px_2p5()
                .py_1()
                .flex()
                .items_center()
                .gap_1()
                .rounded_md()
                .t_small()
                .cursor_pointer()
                .when(installed, |d| d.border_1().border_color(hex(Chrome::BORDER)).text_color(hex(Chrome::SUCCESS)))
                .when(!installed, |d| d.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::ON_ACCENT)))
                .hover(|s| s.opacity(0.85))
                .child(icon(
                    if installed { "check" } else { "plus" },
                    12.,
                    hex(if installed { Chrome::SUCCESS } else { Chrome::ON_ACCENT }),
                ))
                .child(t(cx, if installed { "bots.hub.added" } else { "bots.hub.add" }))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if this.bots.skills.iter().any(|skill| skill.name == name) {
                        this.remove_bot_skill(&name, cx);
                    } else {
                        this.add_bot_skill(source.clone(), cx);
                    }
                }));
            cards = cards.child(
                div()
                    .w(px(318.))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .rounded_lg()
                    .bg(hex(Chrome::PANEL))
                    .border_1()
                    .border_color(hex(if installed { Chrome::OVERLAY_BORDER } else { Chrome::BORDER }))
                    .child(div().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).truncate().child(item.title))
                    .child(div().h(px(34.)).overflow_hidden().t_small().text_color(hex(Chrome::MUTED)).child(item.about))
                    .child(
                        div()
                            .pt_1()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .px_1p5()
                                    .py_px()
                                    .rounded_sm()
                                    .bg(crate::theme::hex_alpha(Chrome::ACCENT, 0.18))
                                    .t_caption()
                                    .text_color(hex(Chrome::FOREGROUND))
                                    .child(item.tag),
                            )
                            .child(div().flex_1())
                            .child(button),
                    ),
            );
        }
        let scroll = hub.scroll.clone();
        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .gap_3()
            .child(filters)
            .child(div().id("skill-hub-cards").flex_1().min_w_0().h_full().overflow_y_scroll().track_scroll(&scroll).child(cards));
        let dialog = div()
            .id("skill-hub")
            .w(px(880.))
            .h(px(620.))
            .max_w_full()
            .max_h_full()
            .p_5()
            .flex()
            .flex_col()
            .gap_4()
            .rounded_xl()
            .bg(hex(Chrome::OVERLAY))
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .shadow_lg()
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(header)
            .child(stats)
            .child(search)
            .child(body);
        Some(
            div()
                .id("skill-hub-overlay")
                .absolute()
                .inset_0()
                .p_4()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::hsla(0., 0., 0., 0.45))
                .occlude()
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_skill_hub(cx)))
                .child(dialog)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_skill_has_a_name_and_a_description_in_each_language() {
        for skill in &BUNDLED {
            let Some((_, title, about)) = SKILL_TEXT.iter().find(|(name, _, _)| *name == skill.name) else {
                panic!("{} has no text", skill.name);
            };
            for language in {
                use crate::settings::Language::*;
                [En, Ko, Ja, Zh, De]
            } {
                for key in [title, about, &category_key(skill.category)] {
                    let text = crate::i18n::tr(language, key);
                    assert!(!text.is_empty() && text != *key, "{key} is not translated");
                }
            }
        }
        assert_eq!(SKILL_TEXT.len(), BUNDLED.len());
        for part in PARTS {
            let (title, about, hint, _) = part_text(part);
            for key in [title, about, hint] {
                assert_ne!(crate::i18n::tr(crate::settings::Language::Ko, key), key);
            }
        }
    }
}
