//! "Build my idea" page: a simple chat where the user describes what they want (messages, pasted
//! documents, attached files). Starting creates a project folder (`agentty_bridge::idea`) and hands
//! it to an agent that builds a demo, shows it live in the in-app browser, and points at Launch.

use crate::i18n::{t, tf};
use crate::launch::PaneKind;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{chip, icon, tilde, IconSize, TypeScale};
use agentty_bridge::idea::{IdeaInput, MAX_ATTACHMENTS};
use gpui::{
    div, prelude::*, px, ClickEvent, Context, Entity, EventEmitter, Focusable, FontWeight, PathPromptOptions, SharedString, Subscription,
    Window,
};
use std::path::PathBuf;

pub enum IdeaEvent {
    Start { input: IdeaInput, root: PathBuf, agent: PaneKind },
}

struct Message {
    text: String,
    /// Pasted as a block (a spec or document) rather than typed.
    pasted: bool,
}

const EXAMPLES: [&str; 4] = ["idea.example.landing", "idea.example.booking", "idea.example.todo", "idea.example.portfolio"];

pub struct IdeaView {
    input: Entity<TextInput>,
    name: Entity<TextInput>,
    messages: Vec<Message>,
    attachments: Vec<PathBuf>,
    agent: PaneKind,
    /// Which agents can be picked (Claude Code, Codex).
    available: (bool, bool),
    root: PathBuf,
    error: Option<String>,
    /// Waiting for the project folder: a second click must not create another one.
    starting: bool,
    scroll: gpui::ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<IdeaEvent> for IdeaView {}

impl IdeaView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::localized("", "idea.input_placeholder", window, cx).keep_pasted_lines());
        let name = cx.new(|cx| TextInput::localized("", "idea.name_placeholder", window, cx));
        let subscriptions = vec![
            cx.subscribe(&input, |this, input, event: &TextInputEvent, cx| match event {
                TextInputEvent::Confirmed => {
                    let text = input.read(cx).text().trim().to_string();
                    if !text.is_empty() {
                        this.push_message(text, false, cx);
                        input.update(cx, |input, cx| input.set_text("", cx));
                    }
                }
                TextInputEvent::PastedLines(text) => this.push_message(text.replace("\r\n", "\n").trim().to_string(), true, cx),
                TextInputEvent::Changed => {
                    this.error = None;
                    cx.notify();
                }
                _ => {}
            }),
            cx.subscribe(&name, |_, _, event: &TextInputEvent, cx| {
                if matches!(event, TextInputEvent::Changed) {
                    cx.notify();
                }
            }),
        ];
        window.focus(&input.read(cx).focus_handle(cx));
        Self {
            input,
            name,
            messages: Vec::new(),
            attachments: Vec::new(),
            agent: PaneKind::Claude,
            available: (true, true),
            root: agentty_bridge::idea::default_root(),
            error: None,
            starting: false,
            scroll: gpui::ScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    /// Installed agents; picks one that is there.
    pub fn set_agents(&mut self, claude: bool, codex: bool, cx: &mut Context<Self>) {
        if self.available == (claude, codex) {
            return;
        }
        self.available = (claude, codex);
        if self.agent == PaneKind::Claude && !claude && codex {
            self.agent = PaneKind::Codex;
        } else if self.agent == PaneKind::Codex && !codex && claude {
            self.agent = PaneKind::Claude;
        }
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.input.read(cx).focus_handle(cx));
    }

    fn push_message(&mut self, text: String, pasted: bool, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        self.messages.push(Message { text, pasted });
        self.error = None;
        // Newest message in view.
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Files dropped on the page or picked with the attach button.
    pub fn add_files(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        for path in paths {
            if path.is_file() && !self.attachments.contains(path) && self.attachments.len() < MAX_ATTACHMENTS {
                self.attachments.push(path.clone());
            }
        }
        self.error = None;
        cx.notify();
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                let _ = this.update(cx, |this, cx| this.add_files(&paths, cx));
            }
        })
        .detach();
    }

    fn pick_root(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = paths.await {
                if let Some(folder) = paths.pop() {
                    let _ = this.update(cx, |this, cx| {
                        this.root = folder;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn can_start(&self, cx: &gpui::App) -> bool {
        !self.messages.is_empty() || !self.attachments.is_empty() || !self.input.read(cx).text().trim().is_empty()
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if self.starting {
            return;
        }
        // Text still in the box counts as the last message.
        let pending = self.input.read(cx).text().trim().to_string();
        if !pending.is_empty() {
            self.messages.push(Message { text: pending, pasted: false });
            self.input.update(cx, |input, cx| input.set_text("", cx));
        }
        if self.messages.is_empty() && self.attachments.is_empty() {
            self.error = Some(t(cx, "idea.empty").to_string());
            cx.notify();
            return;
        }
        let name = self.name.read(cx).text().trim().to_string();
        let input = IdeaInput {
            messages: self.messages.iter().map(|m| m.text.clone()).collect(),
            attachments: self.attachments.clone(),
            name: (!name.is_empty()).then_some(name),
        };
        self.starting = true;
        cx.notify();
        cx.emit(IdeaEvent::Start { input, root: self.root.clone(), agent: self.agent });
    }

    /// `AGENTTY_DEBUG` driver: `msg:<text>`, `paste:<text>`, `file:<path>`, `start`.
    pub fn debug_action(&mut self, argument: &str, cx: &mut Context<Self>) {
        match argument.split_once(':').unwrap_or((argument, "")) {
            ("msg", text) => self.push_message(text.to_string(), false, cx),
            ("paste", text) => self.push_message(crate::debug::unescape(text).replace('\r', "\n"), true, cx),
            ("file", path) => self.add_files(&[PathBuf::from(path)], cx),
            ("start", _) => self.start(cx),
            _ => {}
        }
    }

    /// Called once the project exists: the page is ready for the next idea.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.messages.clear();
        self.attachments.clear();
        self.error = None;
        self.starting = false;
        self.name.update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    pub fn show_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error);
        self.starting = false;
        cx.notify();
    }

    fn render_steps(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let step =
            |number: &'static str, glyph: &'static str, title: &'static str, body: &'static str, color: u32, cx: &mut Context<Self>| {
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .gap_2()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(hex_alpha(color, 0.35))
                    .bg(hex_alpha(color, 0.08))
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(px(28.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(hex_alpha(color, 0.2))
                            .child(icon(glyph, IconSize::INLINE, hex(color))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .t_small()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(hex(Chrome::BRIGHT))
                                    .child(format!("{number}  {}", t(cx, title))),
                            )
                            .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, body))),
                    )
            };
        div()
            .w_full()
            .flex()
            .gap_2()
            .child(step("1", "lightbulb", "idea.step1", "idea.step1_body", Chrome::ORANGE, cx))
            .child(step("2", "hammer", "idea.step2", "idea.step2_body", Chrome::BLUE, cx))
            .child(step("3", "rocket", "idea.step3", "idea.step3_body", Chrome::GREEN, cx))
    }

    fn render_conversation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().w_full().flex().flex_col().gap_2();
        if self.messages.is_empty() && self.attachments.is_empty() {
            let mut examples = div().flex().flex_wrap().justify_center().gap_2();
            for (index, key) in EXAMPLES.into_iter().enumerate() {
                examples = examples.child(
                    div()
                        .id(("idea-example", index))
                        .px_3()
                        .py_1p5()
                        .rounded_full()
                        .border_1()
                        .border_color(hex(Chrome::BORDER))
                        .t_small()
                        .text_color(hex(Chrome::FOREGROUND))
                        .cursor_pointer()
                        .hover(|s| s.border_color(hex(Chrome::ACCENT)).text_color(hex(Chrome::BRIGHT)))
                        .child(t(cx, key))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            let text = t(cx, key).to_string();
                            this.input.update(cx, |input, cx| input.set_text(text, cx));
                            this.focus(window, cx);
                        })),
                );
            }
            return list
                .items_center()
                .py_6()
                .child(div().t_body().text_color(hex(Chrome::MUTED)).child(t(cx, "idea.empty_hint")))
                .child(examples);
        }
        for (index, message) in self.messages.iter().enumerate() {
            let remove = cx.listener(move |this, _: &ClickEvent, _, cx| {
                if index < this.messages.len() {
                    this.messages.remove(index);
                    cx.notify();
                }
            });
            let body = if message.pasted {
                let lines = message.text.lines().count();
                let preview: String = message.text.lines().take(6).collect::<Vec<_>>().join("\n");
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .t_caption()
                            .text_color(hex(Chrome::MUTED))
                            .child(icon("file-text", IconSize::INLINE, hex(Chrome::MUTED)))
                            .child(tf(cx, "idea.pasted", &[("n", &lines.to_string())])),
                    )
                    .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(preview))
            } else {
                div().t_body().text_color(hex(Chrome::BRIGHT)).child(message.text.clone())
            };
            list = list.child(
                div().w_full().flex().justify_end().child(
                    div()
                        .id(("idea-message", index))
                        .group("idea-message")
                        .relative()
                        .max_w(px(560.))
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(hex_alpha(Chrome::ACCENT, if message.pasted { 0.12 } else { 0.28 }))
                        .border_1()
                        .border_color(hex_alpha(Chrome::ACCENT, 0.4))
                        .child(body)
                        .child(
                            div()
                                .id(("idea-message-remove", index))
                                .absolute()
                                .top(px(-6.))
                                .right(px(-6.))
                                .size(px(18.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(hex(Chrome::OVERLAY))
                                .border_1()
                                .border_color(hex(Chrome::OVERLAY_BORDER))
                                .cursor_pointer()
                                .invisible()
                                .group_hover("idea-message", |s| s.visible())
                                .child(icon("x", 10., hex(Chrome::FOREGROUND)))
                                .on_click(remove),
                        ),
                ),
            );
        }
        if !self.attachments.is_empty() {
            let mut files = div().flex().flex_wrap().justify_end().gap_1();
            for (index, path) in self.attachments.iter().enumerate() {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                files = files.child(
                    div()
                        .id(("idea-file", index))
                        .flex()
                        .items_center()
                        .gap_1()
                        .pl_2()
                        .pr_1()
                        .py_0p5()
                        .rounded_md()
                        .border_1()
                        .border_color(hex(Chrome::BORDER))
                        .bg(hex(Chrome::PANEL))
                        .t_small()
                        .text_color(hex(Chrome::FOREGROUND))
                        .child(icon("file-text", IconSize::INLINE, hex(Chrome::MUTED)))
                        .child(name)
                        .child(
                            div()
                                .id(("idea-file-remove", index))
                                .p_0p5()
                                .rounded_sm()
                                .cursor_pointer()
                                .hover(|s| s.bg(hex(Chrome::HOVER)))
                                .child(icon("x", 10., hex(Chrome::MUTED)))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    if index < this.attachments.len() {
                                        this.attachments.remove(index);
                                        cx.notify();
                                    }
                                })),
                        ),
                );
            }
            list = list.child(files);
        }
        list
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_text = !self.input.read(cx).text().trim().is_empty();
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pl_2()
                    .pr_1()
                    .py_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(hex(Chrome::ACCENT))
                    .bg(hex(0x1a1a1a))
                    .child(
                        div()
                            .id("idea-attach")
                            .flex_shrink_0()
                            .p_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(hex(Chrome::HOVER)))
                            .child(icon("file-plus", IconSize::BUTTON, hex(Chrome::FOREGROUND)))
                            .tooltip(crate::ui::Tooltip::text(t(cx, "idea.attach"), None))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.pick_files(cx))),
                    )
                    .child(div().flex_1().min_w_0().py_1().t_body().text_color(hex(Chrome::BRIGHT)).child(self.input.clone()))
                    .child(
                        div()
                            .id("idea-send")
                            .flex_shrink_0()
                            .size(px(28.))
                            .rounded_md()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .bg(if has_text { hex(Chrome::ACCENT) } else { hex(Chrome::HOVER) })
                            .child(icon("arrow-up", IconSize::INLINE, hex(Chrome::BRIGHT)))
                            .tooltip(crate::ui::Tooltip::text(t(cx, "idea.send"), Some("↩")))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                let text = this.input.read(cx).text().trim().to_string();
                                if !text.is_empty() {
                                    this.push_message(text, false, cx);
                                    this.input.update(cx, |input, cx| input.set_text("", cx));
                                }
                            })),
                    ),
            )
            .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "idea.composer_hint")))
    }

    fn render_options(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = |key: &'static str, cx: &mut Context<Self>| {
            div().w(px(96.)).flex_shrink_0().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, key))
        };
        let (claude, codex) = self.available;
        let mut agents = div().flex().gap_1();
        for (kind, name, installed) in [(PaneKind::Claude, "Claude Code", claude), (PaneKind::Codex, "Codex", codex)] {
            if installed {
                agents = agents.child(chip(
                    SharedString::from(format!("idea-agent-{name}")),
                    name,
                    self.agent == kind,
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.agent = kind;
                        cx.notify();
                    }),
                ));
            }
        }
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(hex(Chrome::BORDER))
            .child(
                div().flex().items_center().gap_2().child(label("idea.name", cx)).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .border_1()
                        .border_color(hex(Chrome::BORDER))
                        .bg(hex(0x1a1a1a))
                        .t_small()
                        .text_color(hex(Chrome::BRIGHT))
                        .child(self.name.clone()),
                ),
            )
            .child(div().flex().items_center().gap_2().child(label("idea.agent", cx)).child(agents))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(label("idea.location", cx))
                    .child(div().flex_1().min_w_0().truncate().t_small().text_color(hex(Chrome::FOREGROUND)).child(tilde(&self.root)))
                    .child(
                        div()
                            .id("idea-location")
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .t_small()
                            .cursor_pointer()
                            .text_color(hex(Chrome::FOREGROUND))
                            .hover(|s| s.bg(hex(Chrome::HOVER)))
                            .child(t(cx, "idea.change"))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.pick_root(cx))),
                    ),
            )
    }
}

impl Render for IdeaView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.can_start(cx) && !self.starting;
        let no_agent = self.available == (false, false);
        let page = div()
            .w_full()
            .max_w(px(760.))
            .mx_auto()
            .px_6()
            .py_8()
            .flex()
            .flex_col()
            .items_center()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .child(icon("lightbulb", 36., hex(Chrome::ORANGE)))
                    .child(div().t_display().font_weight(FontWeight::LIGHT).text_color(hex(Chrome::BRIGHT)).child(t(cx, "idea.title")))
                    .child(div().t_body().text_color(hex(Chrome::MUTED)).child(t(cx, "idea.subtitle"))),
            )
            .child(self.render_steps(cx))
            .child(self.render_conversation(cx))
            .child(self.render_composer(cx))
            .child(self.render_options(cx))
            .children(self.error.clone().map(|error| div().w_full().t_small().text_color(hex(Chrome::ERROR)).child(error)))
            .when(no_agent, |d| d.child(div().w_full().t_small().text_color(hex(Chrome::WARNING)).child(t(cx, "idea.no_agent"))))
            .child(
                div()
                    .id("idea-start")
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_6()
                    .py_2p5()
                    .rounded_lg()
                    .t_title()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(hex(Chrome::BRIGHT))
                    .bg(if ready && !no_agent { hex(Chrome::ACCENT) } else { hex(Chrome::HOVER) })
                    .when(ready && !no_agent, |d| d.cursor_pointer().hover(|s| s.bg(hex(Chrome::BLUE))))
                    .child(icon("sparkles", IconSize::BUTTON, hex(Chrome::BRIGHT)))
                    .child(t(cx, "idea.start"))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if !no_agent {
                            this.start(cx);
                        }
                    })),
            )
            .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "idea.start_hint")));
        div()
            .size_full()
            .relative()
            .bg(hex(Chrome::EDITOR))
            .child(div().id("idea-scroll").size_full().overflow_y_scroll().track_scroll(&self.scroll).child(page))
            .child(crate::ui::scrollbar(self.scroll.clone()))
    }
}
