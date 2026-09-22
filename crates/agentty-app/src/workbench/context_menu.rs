//! Status bar "context" panel: what the active agent session holds in its context window — token
//! breakdown, loaded memory files, skills, files in play and compactions.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{compact_number, icon, popover, IconSize, TypeScale};
use agentty_bridge::context::{ContextSnapshot, FileUse, Part};
use agentty_bridge::model::Agent;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, SharedString};
use std::path::PathBuf;

const FILE_ROWS: usize = 8;
const SUMMARY_CHARS: usize = 280;

fn part_color(part: Part) -> u32 {
    match part {
        Part::System => 0x6e7681,
        Part::Memory => Chrome::PURPLE,
        Part::Skills => Chrome::ORANGE,
        Part::Summary => Chrome::GREEN,
        Part::Conversation => Chrome::BLUE,
    }
}

fn part_label(part: Part) -> &'static str {
    match part {
        Part::System => "context.part.system",
        Part::Memory => "context.part.memory",
        Part::Skills => "context.part.skills",
        Part::Summary => "context.part.summary",
        Part::Conversation => "context.part.conversation",
    }
}

fn scope_label(scope: &str) -> Option<&'static str> {
    Some(match scope {
        "User" => "context.scope.user",
        "Project" => "context.scope.project",
        "Local" => "context.scope.local",
        "AutoMem" => "context.scope.auto",
        "Managed" => "context.scope.managed",
        _ => return None,
    })
}

/// The compaction summary without Claude Code's "continued from a previous conversation" preamble.
fn summary_excerpt(text: &str) -> String {
    let body = text.split_once("Summary:").map_or(text, |(_, rest)| rest);
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut excerpt: String = flat.chars().take(SUMMARY_CHARS).collect();
    if flat.chars().count() > SUMMARY_CHARS {
        excerpt.push('…');
    }
    excerpt
}

impl Workbench {
    /// Reads the active pane's session context in the background.
    pub(super) fn load_context(&mut self, cx: &mut Context<Self>) {
        let Some(pane) = self.active_pane() else { return };
        let view = pane.read(cx);
        let agent = view.agent_kind().and_then(crate::launch::PaneKind::agent);
        let id = view.session_id_live.clone().or_else(|| view.spec.session_id.clone());
        let (Some(agent), Some(id)) = (agent, id) else {
            self.inventory.context = None;
            return;
        };
        let key = (agent, id.clone());
        let previous = self.inventory.context.take().filter(|(k, _, _)| *k == key).and_then(|(_, _, s)| s);
        self.inventory.context = Some((key.clone(), true, previous));
        let task = cx.background_spawn(async move { agentty_bridge::context::snapshot(agent, &id) });
        cx.spawn(async move |this, cx| {
            let snapshot = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some((_, loading, slot)) = this.inventory.context.as_mut().filter(|(k, _, _)| *k == key) {
                    *loading = false;
                    *slot = snapshot;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The context-memory panel itself, with no placement of its own: the caller decides where it
    /// goes and defers it. (Deferring here as well as at the call site is what GPUI refuses with
    /// "cannot call defer_draw during deferred drawing".)
    pub(super) fn render_context_menu(&self, agent: Agent, cx: &mut Context<Self>) -> AnyElement {
        let (loading, snapshot) = match &self.inventory.context {
            Some((_, loading, snapshot)) => (*loading, snapshot.as_ref()),
            None => (false, None),
        };
        let refresh = div()
            .id("context-refresh")
            .tooltip(crate::ui::Tooltip::text(t(cx, "context.refresh"), None))
            .p_1()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .child(if loading {
                crate::ui::spinner(IconSize::INLINE, hex(Chrome::MUTED)).into_any_element()
            } else {
                icon("refresh-cw", IconSize::INLINE, hex(Chrome::MUTED)).into_any_element()
            })
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.load_context(cx);
                cx.notify();
            }));
        let header = div()
            .px_2()
            .pt_1()
            .pb_1p5()
            .flex()
            .items_center()
            .gap_2()
            .t_caption()
            .text_color(hex(Chrome::MUTED))
            .child(div().flex_1().truncate().child(format!("{} · {}", agent.display_name(), t(cx, "context.title"))))
            .children(snapshot.and_then(|s| s.model.clone()))
            .child(refresh);

        let body = match snapshot {
            Some(snapshot) => self.render_context_body(snapshot, cx).into_any_element(),
            None if loading => crate::ui::loading_row(t(cx, "context.loading")).into_any_element(),
            None => crate::ui::hint(t(cx, "context.unavailable")).into_any_element(),
        };
        crate::ui::fade_in(
            "status-menu-fade",
            popover()
                .w(px(440.))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if this.status_menu.take().is_some() {
                        this.note_dismissed("status-context");
                    }
                    cx.notify();
                }))
                .child(header)
                .child(body),
        )
        .into_any_element()
    }

    fn render_context_body(&self, s: &ContextSnapshot, cx: &mut Context<Self>) -> impl IntoElement {
        let section = |title: String, count: Option<usize>| {
            div()
                .px_2()
                .pt_2p5()
                .pb_1()
                .flex()
                .items_center()
                .gap_1p5()
                .t_caption()
                .font_weight(crate::theme::EMPHASIS)
                .text_color(hex(Chrome::MUTED))
                .child(title)
                .children(count.map(|n| div().font_weight(FontWeight::NORMAL).child(n.to_string())))
        };
        let open_row = |id: SharedString, path: PathBuf, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_md()
                .flex()
                .items_center()
                .gap_2()
                .t_small()
                .cursor_pointer()
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| cx.open_with_system(&path)))
        };

        // Usage: total and a stacked bar of the parts.
        let percent = if s.window > 0 { s.used as f64 / s.window as f64 * 100.0 } else { 0.0 };
        let mut bar = div().h(px(8.)).w_full().flex().rounded_full().overflow_hidden().bg(hex(0x2d2d30));
        let scale = s.window.max(s.used).max(1) as f32;
        for (part, tokens) in &s.parts {
            bar = bar.child(div().h_full().w(gpui::relative(*tokens as f32 / scale)).bg(hex(part_color(*part))));
        }
        let mut legend = div().flex().flex_wrap().gap_x_3().gap_y_1().t_caption();
        for (part, tokens) in &s.parts {
            legend = legend.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().size(px(7.)).rounded_full().bg(hex(part_color(*part))))
                    .child(div().text_color(hex(Chrome::FOREGROUND)).child(t(cx, part_label(*part))))
                    .child(div().text_color(hex(Chrome::MUTED)).child(compact_number(*tokens))),
            );
        }
        if s.window > s.used {
            legend = legend.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().size(px(7.)).rounded_full().bg(hex(0x2d2d30)).border_1().border_color(hex(Chrome::OVERLAY_BORDER)))
                    .child(div().text_color(hex(Chrome::FOREGROUND)).child(t(cx, "context.part.free")))
                    .child(div().text_color(hex(Chrome::MUTED)).child(compact_number(s.window - s.used))),
            );
        }
        let usage = div()
            .px_2()
            .pb_1()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_1p5()
                    .child(
                        div().t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(compact_number(s.used)),
                    )
                    .when(s.window > 0, |d| {
                        d.child(
                            div().t_small().text_color(hex(Chrome::MUTED)).child(format!("/ {} · {percent:.0}%", compact_number(s.window))),
                        )
                    }),
            )
            .child(bar)
            .child(legend);

        let mut list = div().id("context-list").flex().flex_col().max_h(px(420.)).overflow_y_scroll().child(usage);

        // Memory files.
        list = list.child(section(t(cx, "context.memory").into(), Some(s.memory.len())));
        if s.memory.is_empty() {
            list = list.child(crate::ui::hint(t(cx, "context.memory_none")));
        }
        for (index, file) in s.memory.iter().enumerate() {
            let name = file.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let folder = file.path.parent().map(crate::ui::tilde).unwrap_or_default();
            let scope = scope_label(&file.scope).map(|k| t(cx, k).to_string()).unwrap_or_else(|| file.scope.clone());
            list = list.child(
                open_row(SharedString::from(format!("context-memory-{index}")), file.path.clone(), cx)
                    .child(icon("file-text", IconSize::INLINE, hex(Chrome::PURPLE)))
                    .child(div().flex_shrink_0().text_color(hex(Chrome::BRIGHT)).child(name))
                    .child(
                        div()
                            .flex_shrink_0()
                            .px_1()
                            .rounded_sm()
                            .t_caption()
                            .bg(hex_alpha(Chrome::PURPLE, 0.16))
                            .text_color(hex(Chrome::PURPLE))
                            .child(scope),
                    )
                    .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::MUTED)).child(folder))
                    .child(
                        div().flex_shrink_0().t_caption().text_color(hex(Chrome::MUTED)).child(format!("~{}", compact_number(file.tokens))),
                    ),
            );
        }

        // Skills.
        if s.skills_available > 0 || !s.skills_loaded.is_empty() {
            list = list.child(section(t(cx, "context.skills").into(), None));
            let mut chips = div().px_2().pb_1().flex().flex_wrap().items_center().gap_1().t_caption();
            chips = chips.child(div().text_color(hex(Chrome::MUTED)).child(tf(
                cx,
                "context.skills_available",
                &[("n", &s.skills_available.to_string())],
            )));
            for name in &s.skills_loaded {
                chips = chips.child(
                    div().px_1p5().rounded_sm().bg(hex_alpha(Chrome::ORANGE, 0.16)).text_color(hex(Chrome::ORANGE)).child(name.clone()),
                );
            }
            list = list.child(chips);
        }

        // Files the agent read, edited or was given since the last compaction.
        list = list.child(section(t(cx, "context.files").into(), Some(s.files.len())));
        if s.files.is_empty() {
            list = list.child(crate::ui::hint(t(cx, "context.files_none")));
        }
        for (index, file) in s.files.iter().take(FILE_ROWS).enumerate() {
            let (label, color) = match file.usage {
                FileUse::Read => (t(cx, "context.file.read"), Chrome::MUTED),
                FileUse::Edited => (t(cx, "context.file.edited"), Chrome::SUCCESS),
                FileUse::Attached => (t(cx, "context.file.attached"), Chrome::BLUE),
            };
            let name = file.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let folder = file.path.parent().map(crate::ui::tilde).unwrap_or_default();
            list = list.child(
                open_row(SharedString::from(format!("context-file-{index}")), file.path.clone(), cx)
                    .child(div().flex_shrink_0().w(px(34.)).t_caption().text_color(hex(color)).child(label))
                    .child(div().flex_shrink_0().max_w(px(170.)).truncate().text_color(hex(Chrome::BRIGHT)).child(name))
                    .child(div().flex_1().min_w_0().truncate().text_color(hex(Chrome::MUTED)).child(folder)),
            );
        }
        if s.files.len() > FILE_ROWS {
            list = list.child(div().px_2().pb_1().t_caption().text_color(hex(Chrome::MUTED)).child(tf(
                cx,
                "context.files_more",
                &[("n", &(s.files.len() - FILE_ROWS).to_string())],
            )));
        }

        // Compactions.
        list = list.child(section(t(cx, "context.compaction").into(), None));
        match &s.last_compaction {
            None => list = list.child(crate::ui::hint(t(cx, "context.compaction_none"))),
            Some(compaction) => {
                let mut line = tf(cx, "context.compaction_count", &[("n", &s.compactions.to_string())]);
                if compaction.before > 0 {
                    line.push_str(&format!(" · {} → {}", compact_number(compaction.before), compact_number(compaction.after)));
                }
                list = list.child(div().px_2().t_small().text_color(hex(Chrome::FOREGROUND)).child(line));
                if let Some(summary) = compaction.summary.as_deref().map(summary_excerpt).filter(|s| !s.is_empty()) {
                    list = list.child(
                        div()
                            .mx_2()
                            .mt_1()
                            .mb_1()
                            .px_2()
                            .py_1p5()
                            .rounded_md()
                            .bg(hex(0x1e1e1e))
                            .border_l_2()
                            .border_color(hex(Chrome::GREEN))
                            .t_caption()
                            .text_color(hex(Chrome::MUTED))
                            .child(summary),
                    );
                }
            }
        }

        let footer = div()
            .mt_1()
            .px_2()
            .pt_1p5()
            .pb_0p5()
            .border_t_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .t_caption()
            .text_color(hex(Chrome::MUTED))
            .child(tf(cx, "context.footer", &[("prompts", &s.prompts.to_string()), ("tools", &s.tool_calls.to_string())]));
        div().flex().flex_col().child(list).child(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::summary_excerpt;

    #[test]
    fn summary_excerpt_drops_the_preamble() {
        let text = "This session is being continued from a previous conversation.\n\nSummary:\n1. Primary   request\n   and intent";
        assert_eq!(summary_excerpt(text), "1. Primary request and intent");
        assert_eq!(summary_excerpt("short"), "short");
    }
}
