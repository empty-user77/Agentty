//! "Send to…": where a prompt from a plugin or a link goes — a new workspace, a new tab here, or
//! an open workspace — and with which agent. Nothing is sent until the user confirms.

use super::{status_label, Workbench};
use crate::i18n::{t, tf};
use crate::launch::PaneKind;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{chip, icon, tilde, TypeScale};
use agentty_bridge::plugins::{PromptRequest, PromptTarget};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, PathPromptOptions, SharedString, Window};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    NewWorkspace,
    NewTab,
    Workspace(u64),
}

pub struct PromptDialog {
    pub request: PromptRequest,
    pub kind: PaneKind,
    pub cwd: PathBuf,
    pub destination: Destination,
    pub submit: bool,
    pub error: Option<String>,
    scroll: gpui::ScrollHandle,
}

/// Characters of the prompt shown in the preview.
const PREVIEW_CHARS: usize = 4_000;

impl Workbench {
    pub(super) fn open_prompt_dialog(&mut self, request: PromptRequest, _window: &mut Window, cx: &mut Context<Self>) {
        let kind = match request.agent.as_deref() {
            Some("codex") => PaneKind::Codex,
            Some("shell" | "terminal") => PaneKind::Shell,
            _ => PaneKind::Claude,
        };
        let cwd = request.cwd.clone().filter(|p| p.is_dir()).unwrap_or_else(|| self.default_cwd(cx));
        let destination = match (request.target, request.workspace_id) {
            (PromptTarget::Workspace, Some(id)) if self.workspaces.iter().any(|w| w.id == id) => Destination::Workspace(id),
            (PromptTarget::NewTab, _) if !self.workspaces.is_empty() => Destination::NewTab,
            _ => Destination::NewWorkspace,
        };
        self.prompt_dialog =
            Some(PromptDialog { request, kind, cwd, destination, submit: true, error: None, scroll: gpui::ScrollHandle::new() });
        self.launcher_open = false;
        self.notices_open = false;
        cx.notify();
    }

    pub(super) fn confirm_prompt_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.prompt_dialog.as_ref() else { return };
        let mut request = dialog.request.clone();
        request.agent = Some(crate::brand::kind_id(dialog.kind).to_string());
        request.submit = dialog.submit;
        request.cwd = Some(dialog.cwd.clone());
        match dialog.destination {
            Destination::NewWorkspace => request.target = PromptTarget::NewWorkspace,
            Destination::NewTab => request.target = PromptTarget::NewTab,
            Destination::Workspace(id) => {
                request.target = PromptTarget::Workspace;
                request.workspace_id = Some(id);
            }
        }
        match self.deliver_prompt(request, window, cx) {
            Ok(_) => self.prompt_dialog = None,
            Err(error) => {
                if let Some(dialog) = self.prompt_dialog.as_mut() {
                    dialog.error = Some(error);
                }
            }
        }
        cx.notify();
    }

    fn browse_prompt_folder(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = paths.await {
                if let Some(path) = paths.pop() {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(dialog) = this.prompt_dialog.as_mut() {
                            dialog.cwd = path;
                            dialog.destination = super::prompt_dialog::Destination::NewWorkspace;
                        }
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub(super) fn render_prompt_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.prompt_dialog.as_ref()?;
        let request = &dialog.request;
        let preview: String = {
            let text: String = request.text.chars().take(PREVIEW_CHARS).collect();
            if request.text.chars().count() > PREVIEW_CHARS {
                format!("{text}\n…")
            } else {
                text
            }
        };
        let lines = request.text.lines().count();

        let option = |id: SharedString,
                      glyph: &'static str,
                      title: String,
                      detail: String,
                      selected: bool,
                      value: Destination,
                      cx: &mut Context<Self>| {
            div()
                .id(id)
                .px_3()
                .py_2()
                .flex()
                .items_center()
                .gap_3()
                .rounded_md()
                .cursor_pointer()
                .border_1()
                .border_color(if selected { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
                .bg(if selected { hex_alpha(Chrome::ACCENT, 0.18) } else { hex_alpha(0, 0.) })
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(dialog) = this.prompt_dialog.as_mut() {
                        dialog.destination = value;
                        dialog.error = None;
                    }
                    cx.notify();
                }))
                .child(icon(glyph, crate::ui::IconSize::BUTTON, hex(if selected { Chrome::BRIGHT } else { Chrome::FOREGROUND })))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().t_body().text_color(hex(Chrome::BRIGHT)).truncate().child(title))
                        .child(div().t_small().text_color(hex(Chrome::MUTED)).truncate().child(detail)),
                )
        };

        let mut destinations = div().flex().flex_col().gap_1();
        destinations = destinations.child(
            option(
                "prompt-dest-new-workspace".into(),
                "folder-plus",
                t(cx, "prompt.new_workspace").to_string(),
                tilde(&dialog.cwd),
                dialog.destination == Destination::NewWorkspace,
                Destination::NewWorkspace,
                cx,
            )
            .child(crate::ui::action_button(
                "prompt-browse",
                t(cx, "prompt.change_folder"),
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.browse_prompt_folder(cx);
                }),
            )),
        );
        if let Some(ws) = self.workspaces.get(self.active_workspace) {
            destinations = destinations.child(option(
                "prompt-dest-new-tab".into(),
                "square-plus",
                tf(cx, "prompt.new_tab", &[("name", &self.workspace_title(ws, cx))]),
                tilde(&ws.cwd),
                dialog.destination == Destination::NewTab,
                Destination::NewTab,
                cx,
            ));
        }
        if !self.workspaces.is_empty() {
            destinations = destinations.child(
                div().pt_2().pb_1().px_1().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "prompt.open_workspaces").to_uppercase()),
            );
        }
        for ws in &self.workspaces {
            let panes: Vec<_> = ws.tabs.iter().flat_map(|t| t.root.leaves()).collect();
            let agent = panes.iter().find(|p| p.read(cx).agent_kind() == Some(dialog.kind));
            let detail = match agent {
                Some(pane) => format!("{} · {}", tilde(&ws.cwd), status_label(pane.read(cx), cx).0),
                None if ws.dormant.is_some() => format!("{} · {}", tilde(&ws.cwd), t(cx, "prompt.dormant")),
                None => format!("{} · {}", tilde(&ws.cwd), t(cx, "prompt.new_agent_tab")),
            };
            destinations = destinations.child(option(
                SharedString::from(format!("prompt-dest-ws-{}", ws.id)),
                "layout-panel-left",
                self.workspace_title(ws, cx),
                detail,
                dialog.destination == Destination::Workspace(ws.id),
                Destination::Workspace(ws.id),
                cx,
            ));
        }

        let mut agents = div().flex().gap_1();
        for (kind, label) in [(PaneKind::Claude, "Claude Code"), (PaneKind::Codex, "Codex"), (PaneKind::Shell, t(cx, "welcome.terminal"))] {
            agents = agents.child(chip(
                SharedString::from(format!("prompt-agent-{label}")),
                label.to_string(),
                dialog.kind == kind,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(dialog) = this.prompt_dialog.as_mut() {
                        dialog.kind = kind;
                    }
                    cx.notify();
                }),
            ));
        }
        let submit = dialog.submit;
        let is_shell = dialog.kind == PaneKind::Shell;
        let button = |id: &'static str, label: &str, primary: bool| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded_md()
                .t_body()
                .cursor_pointer()
                .bg(if primary { hex(Chrome::ACCENT) } else { hex(0x2d2d30) })
                .text_color(hex(Chrome::BRIGHT))
                .hover(|s| s.opacity(0.85))
                .child(label.to_string())
        };
        let heading = request.title.clone().filter(|t| !t.trim().is_empty());
        let source = request.source.clone().unwrap_or_default();

        Some(
            div()
                .id("prompt-dialog-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0x000000, 0.45))
                .occlude()
                .child(
                    div()
                        .id("prompt-dialog")
                        .w(px(620.))
                        .max_h(gpui::relative(0.9))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(icon("send", crate::ui::IconSize::BUTTON, hex(Chrome::BRIGHT)))
                                .child(
                                    div()
                                        .t_title()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(hex(Chrome::BRIGHT))
                                        .child(t(cx, "prompt.title")),
                                )
                                .child(div().flex_1())
                                .when(!source.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .px_1p5()
                                            .py_0p5()
                                            .rounded_sm()
                                            .bg(hex(0x2a2a2a))
                                            .t_small()
                                            .text_color(hex(Chrome::MUTED))
                                            .child(tf(cx, "prompt.from", &[("name", &source)])),
                                    )
                                }),
                        )
                        .children(heading.map(|h| div().t_body().text_color(hex(Chrome::FOREGROUND)).truncate().child(h)))
                        .child(
                            div()
                                .id("prompt-preview")
                                .max_h(px(170.))
                                .overflow_y_scroll()
                                .track_scroll(&dialog.scroll)
                                .p_2()
                                .rounded_md()
                                .bg(hex(0x1a1a1a))
                                .border_1()
                                .border_color(hex(Chrome::BORDER))
                                .t_small()
                                .font_family("JetBrains Mono")
                                .text_color(hex(Chrome::FOREGROUND))
                                .child(preview),
                        )
                        .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(tf(
                            cx,
                            "prompt.size",
                            &[("lines", &lines.to_string()), ("chars", &request.text.chars().count().to_string())],
                        )))
                        // Everything is sent, not just what the preview shows.
                        .when(request.text.chars().count() > PREVIEW_CHARS, |d| {
                            d.child(div().t_small().text_color(hex(Chrome::WARNING)).child(tf(
                                cx,
                                "prompt.truncated",
                                &[("shown", &PREVIEW_CHARS.to_string()), ("chars", &request.text.chars().count().to_string())],
                            )))
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "prompt.agent")))
                                .child(agents),
                        )
                        .child(
                            div().id("prompt-destinations").flex_1().min_h(px(80.)).max_h(px(260.)).overflow_y_scroll().child(destinations),
                        )
                        .when(is_shell, |d| d.child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "prompt.submit_shell"))))
                        .when(!is_shell, |d| {
                            d.child(
                                div()
                                    .id("prompt-submit")
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .t_small()
                                    .text_color(hex(Chrome::FOREGROUND))
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        if let Some(dialog) = this.prompt_dialog.as_mut() {
                                            dialog.submit = !dialog.submit;
                                        }
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .size(px(14.))
                                            .rounded_sm()
                                            .border_1()
                                            .border_color(hex(if submit { Chrome::ACCENT } else { Chrome::OVERLAY_BORDER }))
                                            .bg(if submit { hex(Chrome::ACCENT) } else { hex_alpha(0, 0.) })
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(submit, |d| d.child(icon("check", 11., hex(Chrome::BRIGHT)))),
                                    )
                                    .child(t(cx, "prompt.submit")),
                            )
                        })
                        .children(dialog.error.clone().map(|e| div().t_small().text_color(hex(Chrome::ERROR)).child(e)))
                        .child(
                            div()
                                .pt_1()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(button("prompt-cancel", t(cx, "confirm.cancel"), false).on_click(cx.listener(
                                    |this, _: &ClickEvent, window, cx| {
                                        this.prompt_dialog = None;
                                        this.focus_active(window, cx);
                                        cx.notify();
                                    },
                                )))
                                .child(
                                    button("prompt-send", t(cx, "prompt.send"), true)
                                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.confirm_prompt_dialog(window, cx))),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
