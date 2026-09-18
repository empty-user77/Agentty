//! Settings → System check (Windows and Linux): which helper tools are installed, what each is
//! for, and one click to install a missing one in a new terminal tab (winget / the package
//! manager / the vendor installer), copy its command, or open its download page.

use super::settings_page::section;
use super::Workbench;
use crate::i18n::t;
use crate::launch::LaunchSpec;
use crate::setup_check::{self, Need, Tool};
use crate::theme::{hex, Chrome};
use crate::ui::{action_button, TypeScale};
use gpui::{div, prelude::*, ClickEvent, ClipboardItem, Context, Div, SharedString, Window};

impl Workbench {
    /// Runs the checks in the background; `first_run` opens the page when something important
    /// is missing (once per install).
    pub(super) fn run_system_check(&mut self, first_run: bool, cx: &mut Context<Self>) {
        if self.system_checking {
            return;
        }
        self.system_checking = true;
        cx.notify();
        let task = cx.background_spawn(async { setup_check::check() });
        cx.spawn(async move |this, cx| {
            let tools = task.await;
            let _ = this.update(cx, |this, cx| {
                if first_run && setup_check::needs_attention(&tools) {
                    this.settings_section = super::settings_page::SettingsSection::System;
                    this.open_page(super::Page::Settings, cx);
                }
                this.system_check = Some(tools);
                this.system_checking = false;
                cx.notify();
            });
        })
        .detach();
    }

    /// First launch on Windows / Linux: check once and show the page if needed.
    pub(super) fn first_run_system_check(&mut self, cx: &mut Context<Self>) {
        if cfg!(target_os = "macos") || self.slot != 0 || crate::settings::settings(cx).setup_check_shown {
            return;
        }
        crate::settings::update_settings(cx, |s| s.setup_check_shown = true);
        self.run_system_check(true, cx);
    }

    fn install_tool(&mut self, tool: &Tool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(command) = tool.install.clone() else { return };
        let spec = LaunchSpec::shell_command(command, format!("Install {}", tool.name), setup_check::install_dir());
        self.open_tab(spec, window, cx);
    }

    pub(super) fn render_system_check(&mut self, cx: &mut Context<Self>) -> Div {
        if self.system_check.is_none() && !self.system_checking {
            self.run_system_check(false, cx);
        }
        let tools = self.system_check.clone().unwrap_or_default();
        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().flex_1().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "system.intro")))
            .child(action_button(
                "system-recheck",
                if self.system_checking { t(cx, "system.checking") } else { t(cx, "system.recheck") },
                cx.listener(|this, _: &ClickEvent, _, cx| this.run_system_check(false, cx)),
            ));

        let mut list = div().flex().flex_col().gap_2();
        if !tools.is_empty() && !setup_check::needs_attention(&tools) {
            list = list.child(div().t_body().text_color(hex(Chrome::SUCCESS)).child(t(cx, "system.all_good")));
        }
        for (index, tool) in tools.iter().enumerate() {
            let (glyph, color) = match (tool.missing(), tool.need) {
                (false, _) => ("circle-check", Chrome::SUCCESS),
                (true, Need::Required) => ("circle-x", Chrome::ERROR),
                (true, Need::Recommended) => ("shield-alert", Chrome::WARNING),
                (true, Need::Optional) => ("circle-dot", Chrome::MUTED),
            };
            let detail = match &tool.found {
                Some(found) => found.clone(),
                None => t(cx, "system.missing").to_string(),
            };
            let mut buttons = div().flex().gap_1();
            if tool.missing() {
                if let Some(command) = tool.install.clone() {
                    let target = tool.clone();
                    buttons = buttons
                        .child(action_button(
                            SharedString::from(format!("system-install-{index}")),
                            t(cx, "system.install"),
                            cx.listener(move |this, _: &ClickEvent, window, cx| this.install_tool(&target, window, cx)),
                        ))
                        .child(action_button(SharedString::from(format!("system-copy-{index}")), t(cx, "system.copy"), move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(command.clone()))
                        }));
                }
            }
            let guide = tool.guide;
            buttons = buttons.child(action_button(
                SharedString::from(format!("system-guide-{index}")),
                t(cx, "system.guide"),
                move |_, _, cx| cx.open_url(guide),
            ));
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(hex(0x232323))
                    .child(crate::ui::icon(glyph, crate::ui::IconSize::INLINE, hex(color)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .items_center()
                                    .child(div().t_body().text_color(hex(Chrome::BRIGHT)).child(tool.name))
                                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, tool.need.label_key()))),
                            )
                            .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(t(cx, tool.purpose)))
                            .child(div().t_small().truncate().text_color(hex(Chrome::MUTED)).child(detail)),
                    )
                    .child(buttons),
            );
        }
        section(t(cx, "system.tools"))
            .child(header)
            .child(list)
            .child(div().pt_2().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "system.install_note")))
    }
}
