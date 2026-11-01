//! "Did you know?": a card at start that tells someone who updated about a feature they could not
//! have heard of, with the way to its setting. Shown once per feature, and never to a new install,
//! whose onboarding already asks.

use super::settings_page::SettingsSection;
use super::{chrome, Workbench};
use crate::i18n::t;
use crate::settings::{settings, update_settings};
use crate::theme::{hex, Chrome};
use crate::ui::TypeScale;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context};

/// Chat view: Claude Code and Codex shown as a chat over their CLI.
pub(super) const CHAT_VIEW: &str = "chat_view";

impl Workbench {
    /// At start, in the first window: the card for a feature this person has not met yet. Someone
    /// who already uses it is not told about it, and it is noted as seen.
    pub(super) fn prepare_feature_tip(&mut self, cx: &mut Context<Self>) {
        let prefs = settings(cx);
        if self.slot != 0 || !prefs.onboarding_done || prefs.tips_seen.iter().any(|tip| tip == CHAT_VIEW) {
            return;
        }
        if prefs.chat_view {
            self.feature_tip_seen(CHAT_VIEW, cx);
        } else {
            self.feature_tip = Some(CHAT_VIEW);
        }
    }

    /// The card was answered (or the feature reached another way): it does not come back.
    pub(super) fn feature_tip_seen(&mut self, tip: &'static str, cx: &mut Context<Self>) {
        if self.feature_tip == Some(tip) {
            self.feature_tip = None;
            cx.notify();
        }
        if !settings(cx).tips_seen.iter().any(|seen| seen == tip) {
            update_settings(cx, move |s| s.tips_seen.push(tip.to_string()));
            // Not a change the person made: no "Settings updated" when the card opens Settings.
            self.settings_seen = cx.global::<crate::settings::SettingsStore>().revision;
        }
    }

    pub(super) fn render_feature_tip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tip = self.feature_tip?;
        // While the onboarding or a page is up, the card waits.
        if self.onboarding.is_some() || self.page.is_some() {
            return None;
        }
        let button = |id: &'static str, label: String, primary: bool| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded_md()
                .t_body()
                .cursor_pointer()
                .bg(if primary { hex(Chrome::ACCENT) } else { hex(Chrome::PANEL) })
                .when(!primary && crate::theme::is_light(), crate::ui::outlined)
                .text_color(hex(if primary { Chrome::ON_ACCENT } else { Chrome::BRIGHT }))
                .hover(|s| s.opacity(0.85))
                .child(label)
        };
        let docked = self.overlay_right_inset(cx);
        Some(
            div()
                .absolute()
                .right(px(16. + docked))
                .bottom(px(chrome::STATUS_BAR_HEIGHT + 16.))
                .child(crate::ui::fade_in(
                    "feature-tip",
                    div()
                        .id("feature-tip-card")
                        .w(px(360.))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .shadow_lg()
                        .occlude()
                        .child(
                            div().flex().items_center().gap_2().child(crate::ui::icon("message-square", 16., hex(Chrome::ACCENT))).child(
                                div()
                                    .t_title()
                                    .font_weight(crate::theme::EMPHASIS)
                                    .text_color(hex(Chrome::BRIGHT))
                                    .child(t(cx, "tip.chat_view_title")),
                            ),
                        )
                        .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(t(cx, "tip.chat_view_body")))
                        .child(
                            div()
                                .pt_2()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    button("feature-tip-close", t(cx, "confirm.close").to_string(), false)
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.feature_tip_seen(tip, cx))),
                                )
                                .child(button("feature-tip-open", t(cx, "tip.open_setting").to_string(), true).on_click(cx.listener(
                                    move |this, _: &ClickEvent, _, cx| {
                                        this.feature_tip_seen(tip, cx);
                                        this.open_settings_row(SettingsSection::Ai, t(cx, "settings.chat_view").to_string(), cx);
                                    },
                                ))),
                        ),
                ))
                .into_any_element(),
        )
    }
}
