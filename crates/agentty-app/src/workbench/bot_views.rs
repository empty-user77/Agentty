//! How bots are met: their section in the sidebar, the templates, "New bot" and the picker for a
//! bot's character.

use super::bot_face::{avatar, face, own_color};
use super::bot_templates::TEMPLATES;
use super::bots::{BotState, Party};
use super::Workbench;
use crate::i18n::{t, tf};
use crate::text_input::TextInput;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, TypeScale as _};
use agentty_bridge::bots::{BotAgent, AVATARS, AVATAR_COLORS};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Div, Entity, SharedString};

/// Bots listed in the sidebar before the list scrolls.
const SIDE_ROWS: f32 = 6.;
const SIDE_ROW: f32 = 46.;

/// What the picker for a bot's character changes.
#[derive(Clone, Copy)]
pub(super) enum Look {
    Character(&'static str),
    Color(&'static str),
    /// A picture of the user's own is to be chosen.
    Picture,
}

/// The grey mark beside the word "Bots" while bots are in beta.
pub(super) fn beta_label(cx: &gpui::App) -> Div {
    div()
        .flex_none()
        .px_1p5()
        .py_px()
        .rounded_full()
        .t_caption()
        .font_weight(gpui::FontWeight::NORMAL)
        .bg(hex(Chrome::HOVER))
        .text_color(hex(Chrome::MUTED))
        .child(t(cx, "bots.beta"))
}

/// The colour a character stands on: the one chosen, else its own.
pub(super) fn tint(character: &str, color: &str) -> u32 {
    AVATAR_COLORS.iter().find(|(name, _)| *name == color).map_or_else(|| own_color(character), |(_, value)| *value)
}

impl Workbench {
    /// The bots, at the foot of the sidebar under the workspaces: each with its character, what it is doing
    /// and when it last did something. A row opens the bot's conversation.
    pub(super) fn render_bots_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let folded = self.prefs(cx).bots_folded;
        // The bar folds the section, as the bars of the groups above it do; the page with every
        // bot has its own button.
        let header = div()
            .h(px(28.))
            .flex_none()
            .pl_3()
            .pr_2()
            .flex()
            .items_center()
            .gap_1()
            .t_small()
            .text_color(hex(Chrome::MUTED))
            .child(
                div()
                    .id("side-bots-fold")
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
                    .child(icon(if folded { "chevron-right" } else { "chevron-down" }, IconSize::INLINE, hex(Chrome::MUTED)))
                    .child(t(cx, "page.bots"))
                    .child(div().pl_1().child(beta_label(cx)))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.change_settings(cx, |s| s.bots_folded = !s.bots_folded);
                        cx.notify();
                    })),
            )
            .child(
                crate::ui::icon_only(
                    "side-bots-all",
                    "maximize-2",
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.close_bot_profile(cx);
                        this.open_bots_page(cx);
                    }),
                )
                .tooltip(crate::ui::Tooltip::text(t(cx, "bots.all"), None)),
            )
            .child(
                crate::ui::icon_only(
                    "side-bots-new",
                    "plus",
                    cx.listener(|this, _: &ClickEvent, window, cx| this.open_new_bot(window, cx)),
                )
                .tooltip(crate::ui::Tooltip::text(t(cx, "bots.new"), None)),
            );
        let mut section = div().flex_none().flex().flex_col().pb_1().border_t_1().border_color(hex(Chrome::BORDER)).child(header);
        if folded {
            return section.into_any_element();
        }
        if self.bots.list.is_empty() {
            let stack = div()
                .flex()
                .flex_none()
                .child(face("cat", own_color("cat"), 28.))
                .child(div().ml(px(-9.)).child(face("owl", own_color("owl"), 28.)))
                .child(div().ml(px(-9.)).child(face("fox", own_color("fox"), 28.)));
            return section
                .child(
                    div()
                        .id("side-bots-start")
                        .mx_2()
                        .mb_1()
                        .p_2p5()
                        .flex()
                        .items_center()
                        .gap_2p5()
                        .rounded_lg()
                        .bg(hex(Chrome::PANEL))
                        .border_1()
                        .border_color(hex(Chrome::BORDER))
                        .cursor_pointer()
                        .hover(|s| s.border_color(hex(Chrome::OVERLAY_BORDER)))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.open_new_bot(window, cx)))
                        .child(stack)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .t_body()
                                        .font_weight(crate::theme::EMPHASIS)
                                        .text_color(hex(Chrome::BRIGHT))
                                        .truncate()
                                        .child(t(cx, "bots.side_title")),
                                )
                                .child(div().t_small().text_color(hex(Chrome::MUTED)).truncate().child(t(cx, "bots.side_about"))),
                        ),
                )
                .into_any_element();
        }
        let now = crate::ui::now_ms();
        let front = match self.page {
            None => self.workspace_bot(self.active_workspace, cx),
            Some(super::Page::Bots) => self.bots.profile.clone(),
            Some(_) => None,
        };
        let mut rows = div().id("side-bots-rows").max_h(px(SIDE_ROWS * SIDE_ROW)).overflow_y_scroll().px_2().flex().flex_col().gap_px();
        for bot in &self.bots.list {
            let state = self.bot_state(bot, cx);
            let line = self.bot_line(bot, state, cx).unwrap_or_else(|| bot.purpose.clone());
            let last = self.bots.activity.iter().find(|entry| entry.bot == bot.handle).map(|entry| entry.at_ms);
            let handle = bot.handle.clone();
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("side-bot-{handle}")))
                    .h(px(SIDE_ROW))
                    .flex_none()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .rounded_md()
                    .cursor_pointer()
                    .when(front.as_deref() == Some(bot.handle.as_str()), |d| d.bg(hex(Chrome::SELECTED)))
                    .hover(|s| s.bg(hex(Chrome::HOVER)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open_bot_profile(&handle, window, cx)))
                    .child(avatar(bot, state, 30., Chrome::SIDE_BAR))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .t_body()
                                            .font_weight(crate::theme::EMPHASIS)
                                            .text_color(hex(if state == BotState::Off { Chrome::MUTED } else { Chrome::BRIGHT }))
                                            .child(bot.name.clone()),
                                    )
                                    .children(last.map(|at| {
                                        div()
                                            .flex_none()
                                            .t_caption()
                                            .text_color(hex(Chrome::MUTED))
                                            .child(crate::ui::relative_time(now, at))
                                    })),
                            )
                            .child(
                                div()
                                    .t_small()
                                    .truncate()
                                    .text_color(hex(if state == BotState::NeedsYou { Chrome::ATTENTION } else { Chrome::MUTED }))
                                    .child(line),
                            ),
                    ),
            );
        }
        section = section.child(rows);
        section.into_any_element()
    }

    /// Who an entry of the activity is from or to, as a small tile: a bot's character, or a glyph
    /// for the user, a schedule, a channel, a room.
    pub(super) fn party_face(&self, party: &Party, size: f32) -> AnyElement {
        let glyph = |name: &'static str| {
            div()
                .flex_none()
                .size(px(size))
                .rounded(px(size * 0.2375))
                .bg(hex(Chrome::SELECTED))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(name, size * 0.62, hex(Chrome::FOREGROUND)))
                .into_any_element()
        };
        match party {
            Party::Bot(handle) => match self.bots.get(handle) {
                Some(bot) => super::bot_face::tile(bot, size).into_any_element(),
                None => glyph("bot"),
            },
            Party::You => glyph("circle-user"),
            Party::Session => glyph("terminal"),
            Party::Schedule => glyph("clock"),
            Party::Watch => glyph("eye"),
            Party::Channel(_) => glyph("send"),
            Party::Room(_) => glyph("users"),
        }
    }

    /// The ready-made bots as cards `width` wide: a character, the job, and when it starts by
    /// itself. A card opens "New bot" filled in from it; `blank` adds the card for an empty one.
    pub(super) fn render_bot_templates(&self, place: &'static str, width: f32, blank: bool, cx: &mut Context<Self>) -> AnyElement {
        let card = |id: String, tile: AnyElement, title: &str, when: Option<(&'static str, String)>, about: &str| {
            div()
                .id(SharedString::from(id))
                .w(px(width))
                .p_3()
                .flex()
                .items_start()
                .gap_3()
                .rounded_lg()
                .bg(hex(Chrome::PANEL))
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .cursor_pointer()
                .hover(|s| s.border_color(hex(Chrome::OVERLAY_BORDER)).bg(hex(Chrome::HOVER)))
                .child(tile)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .t_body()
                                .font_weight(crate::theme::EMPHASIS)
                                .text_color(hex(Chrome::BRIGHT))
                                .truncate()
                                .child(title.to_string()),
                        )
                        .child(div().t_small().text_color(hex(Chrome::MUTED)).child(about.to_string()))
                        .children(when.map(|(glyph, when)| {
                            div()
                                .pt_0p5()
                                .flex()
                                .items_center()
                                .gap_1()
                                .t_caption()
                                .text_color(hex(Chrome::MUTED))
                                .child(icon(glyph, 11., hex(Chrome::MUTED)))
                                .child(when)
                        })),
                )
        };
        let mut grid = div().flex().flex_wrap().gap_2();
        for (index, template) in TEMPLATES.iter().enumerate() {
            // What starts it without the user: something it watches for, or a time.
            let when = match (template.watch, &template.schedule) {
                (Some(kind), _) => Some(("eye", t(cx, super::bot_watch::label(kind)).to_string())),
                (None, Some(schedule)) => Some(("clock", format!("{} {}", t(cx, schedule.days), schedule.time))),
                (None, None) => None,
            };
            grid = grid.child(
                card(
                    format!("{place}-template-{}", template.handle),
                    face(template.avatar, tint(template.avatar, template.color), 44.).into_any_element(),
                    t(cx, template.title),
                    when,
                    t(cx, template.about),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.pick_bot_template(Some(index), window, cx))),
            );
        }
        if blank {
            let tile = div()
                .flex_none()
                .size(px(44.))
                .rounded(px(44. * 0.2375))
                .border_1()
                .border_dashed()
                .border_color(hex(Chrome::OVERLAY_BORDER))
                .flex()
                .items_center()
                .justify_center()
                .child(icon("plus", 18., hex(Chrome::FOREGROUND)))
                .into_any_element();
            grid = grid.child(
                card(format!("{place}-template-blank"), tile, t(cx, "bots.blank"), None, t(cx, "bots.blank.about"))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.pick_bot_template(None, window, cx))),
            );
        }
        grid.into_any_element()
    }

    /// The characters and the colours to choose from, with the chosen ones ringed.
    pub(super) fn render_look_picker(
        &self,
        place: &str,
        character: &str,
        color: &str,
        picture: Option<std::path::PathBuf>,
        on_pick: impl Fn(&mut Self, Look, &mut Context<Self>) + Clone + 'static,
        cx: &mut Context<Self>,
    ) -> (Div, Div) {
        let ring = |chosen: bool| if chosen { hex(Chrome::BRIGHT) } else { hex_alpha(0, 0.) };
        let mut characters = div().flex().flex_wrap().gap_1();
        for name in AVATARS {
            let pick = on_pick.clone();
            characters = characters.child(
                div()
                    .id(SharedString::from(format!("{place}-face-{name}")))
                    .p(px(2.))
                    .rounded(px(12.))
                    .border_2()
                    .border_color(ring(name == character && picture.is_none()))
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.85))
                    .child(face(name, tint(name, color), 34.))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| pick(this, Look::Character(name), cx))),
            );
        }
        // The user's own picture: the one chosen, ringed, and the tile that chooses one.
        if let Some(path) = &picture {
            characters = characters.child(
                div().p(px(2.)).rounded(px(12.)).border_2().border_color(ring(true)).child(super::bot_face::picture(path.clone(), 34.)),
            );
        }
        let pick = on_pick.clone();
        characters = characters.child(
            div()
                .id(SharedString::from(format!("{place}-face-picture")))
                .p(px(2.))
                .rounded(px(12.))
                .border_2()
                .border_color(ring(false))
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .tooltip(crate::ui::Tooltip::text(t(cx, "bots.picture"), None))
                .child(
                    div()
                        .size(px(34.))
                        .rounded(px(34. * 0.2375))
                        .border_1()
                        .border_dashed()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("image", 16., hex(Chrome::FOREGROUND))),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| pick(this, Look::Picture, cx))),
        );
        let shown = tint(character, color);
        let mut colors = div().flex().flex_wrap().gap_1();
        for (name, value) in AVATAR_COLORS {
            let pick = on_pick.clone();
            colors = colors.child(
                div()
                    .id(SharedString::from(format!("{place}-color-{name}")))
                    .p(px(2.))
                    .rounded_full()
                    .border_2()
                    .border_color(ring(value == shown && picture.is_none()))
                    .cursor_pointer()
                    .child(div().size(px(20.)).rounded_full().bg(hex(value)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| pick(this, Look::Color(name), cx))),
            );
        }
        (characters, colors)
    }

    pub(super) fn render_new_bot_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.bots.new_bot.as_ref()?;
        let button = |id: &'static str, primary: bool| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded_md()
                .t_body()
                .cursor_pointer()
                .bg(hex(if primary { Chrome::ACCENT } else { 0x2d2d30 }))
                .when(!primary && crate::theme::is_light(), crate::ui::outlined)
                .text_color(hex(if primary { Chrome::ON_ACCENT } else { Chrome::BRIGHT }))
                .hover(|s| s.opacity(0.85))
        };
        let cancel = button("new-bot-cancel", false)
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.close_new_bot(window, cx)))
            .child(t(cx, "confirm.cancel"));
        let title = |text: &'static str| div().t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(text);
        let (pick_title, new_title) = (t(cx, "bots.pick"), t(cx, "bots.new"));
        let shell = |width: f32| {
            div()
                .id("new-bot-dialog")
                .w(px(width))
                .max_w_full()
                .max_h_full()
                .overflow_y_scroll()
                .p_5()
                .flex()
                .flex_col()
                .gap_3()
                .rounded_xl()
                .bg(hex(Chrome::OVERLAY))
                .border_1()
                .border_color(hex(Chrome::OVERLAY_BORDER))
                .shadow_lg()
                .on_click(|_, _, cx| cx.stop_propagation())
        };
        let body = if dialog.picking {
            shell(700.)
                .child(title(pick_title))
                .child(self.render_bot_templates("new-bot", 323., true, cx))
                .child(div().flex().justify_end().child(cancel))
        } else {
            let field = |input: &Entity<TextInput>| {
                div()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .bg(hex(Chrome::PANEL))
                    .border_1()
                    .border_color(hex(Chrome::BORDER))
                    .t_body()
                    .text_color(hex(Chrome::BRIGHT))
                    .child(input.clone())
            };
            let mut agents = div().flex().gap_1();
            for (agent, label) in [(BotAgent::Claude, "Claude Code"), (BotAgent::Codex, "Codex")] {
                agents = agents.child(crate::ui::chip(
                    label,
                    label,
                    dialog.agent == agent,
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(dialog) = this.bots.new_bot.as_mut() {
                            dialog.agent = agent;
                        }
                        cx.notify();
                    }),
                ));
            }
            let (characters, colors) = self.render_look_picker(
                "new-bot",
                &dialog.avatar,
                &dialog.color,
                dialog.image.clone(),
                |this, look, cx| {
                    if matches!(look, Look::Picture) {
                        return this.browse_bot_image(None, cx);
                    }
                    if let Some(dialog) = this.bots.new_bot.as_mut() {
                        dialog.image = None;
                        match look {
                            Look::Character(name) => dialog.avatar = name.to_string(),
                            Look::Color(name) => dialog.color = name.to_string(),
                            Look::Picture => {}
                        }
                    }
                    cx.notify();
                },
                cx,
            );
            let schedule = dialog.template.and_then(|index| TEMPLATES[index].schedule.as_ref()).map(|schedule| {
                let when = format!("{} {}", t(cx, schedule.days), schedule.time);
                div().flex().child(crate::ui::chip(
                    "new-bot-schedule",
                    tf(cx, "bots.auto_run", &[("when", &when)]),
                    dialog.scheduled,
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        if let Some(dialog) = this.bots.new_bot.as_mut() {
                            dialog.scheduled = !dialog.scheduled;
                        }
                        cx.notify();
                    }),
                ))
            });
            shell(460.)
                .child(title(new_title))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(match &dialog.image {
                            Some(path) => super::bot_face::picture(path.clone(), 76.),
                            None => face(&dialog.avatar, tint(&dialog.avatar, &dialog.color), 76.),
                        })
                        .child(div().flex_1().min_w_0().flex().flex_col().gap_2().child(field(&dialog.name)).child(field(&dialog.handle))),
                )
                .child(characters)
                .child(colors)
                .child(field(&dialog.purpose))
                .child(agents)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .t_small()
                                .text_color(hex(Chrome::MUTED))
                                .child(crate::ui::tilde(&dialog.folder)),
                        )
                        .child(crate::ui::action_button(
                            "new-bot-folder",
                            t(cx, "bots.folder"),
                            cx.listener(|this, _: &ClickEvent, _, cx| this.browse_bot_folder(cx)),
                        )),
                )
                .children(schedule)
                .children(dialog.template.and_then(|index| TEMPLATES[index].watch).map(|kind| {
                    div().flex().child(crate::ui::chip(
                        "new-bot-watch",
                        tf(cx, "bots.auto_watch", &[("what", t(cx, super::bot_watch::label(kind)))]),
                        dialog.watching,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Some(dialog) = this.bots.new_bot.as_mut() {
                                dialog.watching = !dialog.watching;
                            }
                            cx.notify();
                        }),
                    ))
                }))
                .children(dialog.error.map(|key| div().t_small().text_color(hex(Chrome::ERROR)).child(t(cx, key))))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            button("new-bot-back", false)
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    if let Some(dialog) = this.bots.new_bot.as_mut() {
                                        dialog.picking = true;
                                    }
                                    cx.notify();
                                }))
                                .child(t(cx, "bots.back")),
                        )
                        .child(div().flex_1())
                        .child(cancel)
                        .child(
                            button("new-bot-create", true)
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.create_bot(window, cx)))
                                .child(t(cx, "bots.create")),
                        ),
                )
        };
        Some(
            div()
                .id("new-bot-overlay")
                .absolute()
                .inset_0()
                .p_4()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::hsla(0., 0., 0., 0.45))
                .occlude()
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(body)
                .into_any_element(),
        )
    }
}
