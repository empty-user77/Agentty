//! "That CLI isn't installed yet": what Agentty says when Claude Code or Codex is picked on the
//! start page before its command line tool is there, with a link to the official guide.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context};

impl Workbench {
    pub(super) fn render_install_hint(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (brand, name, url) = self.install_hint?;
        let button = |id: &'static str, label: String, primary: bool| {
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
                .child(label)
        };
        Some(
            div()
                .id("install-hint-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0x000000, 0.45))
                .occlude()
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.install_hint = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("install-hint")
                        .w(px(420.))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .shadow_lg()
                        // Clicks inside the dialog must not reach the overlay that closes it.
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(div().flex().items_center().gap_2().child(crate::brand::avatar(brand, 22.)).child(
                            div().t_title().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(tf(
                                cx,
                                "install.title",
                                &[("name", name)],
                            )),
                        ))
                        .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(tf(cx, "install.body", &[("name", name)])))
                        .child(div().t_small().font_family("JetBrains Mono").text_color(hex(Chrome::MUTED)).child(url))
                        .child(
                            div()
                                .pt_1()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(button("install-hint-close", t(cx, "confirm.close").to_string(), false).on_click(cx.listener(
                                    |this, _: &ClickEvent, _, cx| {
                                        this.install_hint = None;
                                        cx.notify();
                                    },
                                )))
                                .child(
                                    button(
                                        "install-hint-open",
                                        t(cx, if cfg!(target_os = "macos") { "install.open_guide" } else { "install.open_system_check" })
                                            .to_string(),
                                        true,
                                    )
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.install_hint = None;
                                        if cfg!(target_os = "macos") {
                                            cx.open_url(url);
                                        } else {
                                            // Windows / Linux: one-click install from Settings → System check.
                                            this.system_check = None;
                                            this.settings_section = super::settings_page::SettingsSection::System;
                                            this.open_page(super::Page::Settings, cx);
                                        }
                                        cx.notify();
                                    })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
