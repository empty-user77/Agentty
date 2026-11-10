//! Which login a new agent starts on, when its agent has more than one: asked every time, as a list
//! of the logins with the agent they belong to, their plan and what is left of its limits. The
//! window's own login is marked and picked to begin with.

use super::account_menu::{name_of, usage_line};
use super::{LaunchTarget, Workbench};
use crate::i18n::t;
use crate::launch::LaunchChoice;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use agentty_bridge::accounts::AccountStore;
use agentty_bridge::agent_auth::AuthAgent;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Window};

/// The question while it is open.
pub(super) struct AccountPick {
    choice: LaunchChoice,
    target: LaunchTarget,
    agent: AuthAgent,
    /// Every login of the agent: its own first (`None`), then the added ones.
    logins: Vec<Option<String>>,
    /// The highlighted row (↑ / ↓).
    selected: usize,
}

impl Workbench {
    /// Asks which login `choice` starts on when its agent has more than one; the answer starts it.
    /// Returns whether it asked.
    pub(super) fn ask_launch_account(&mut self, choice: &LaunchChoice, target: LaunchTarget, cx: &mut Context<Self>) -> bool {
        let Some(agent) = choice.account_agent() else { return false };
        let store = AccountStore::load();
        if store.of(agent).is_empty() {
            return false;
        }
        let logins: Vec<Option<String>> = std::iter::once(None).chain(store.of(agent).iter().map(|a| Some(a.id.clone()))).collect();
        let current = self.accounts.get(agent);
        let selected = logins.iter().position(|id| id.as_deref() == current).unwrap_or(0);
        self.account_pick = Some(AccountPick { choice: choice.clone(), target, agent, logins, selected });
        // Plans and limits for the rows.
        self.refresh_account_rows(cx);
        cx.notify();
        true
    }

    fn start_on_login(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pick) = self.account_pick.take() else { return };
        let Some(id) = pick.logins.get(index).cloned() else { return };
        self.request_launch(LaunchChoice::OnAccount(Box::new(pick.choice), pick.agent, id), pick.target, window, cx);
        cx.notify();
    }

    fn close_account_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.account_pick.take().is_some() {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// ↑ / ↓ move, ↩ starts, Esc closes; the question owns the keyboard while it is open.
    pub(super) fn account_pick_key(&mut self, event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(pick) = self.account_pick.as_mut() else { return false };
        let count = pick.logins.len();
        match event.keystroke.key.as_str() {
            "up" => pick.selected = (pick.selected + count - 1) % count,
            "down" | "tab" => pick.selected = (pick.selected + 1) % count,
            "enter" => {
                let index = pick.selected;
                self.start_on_login(index, window, cx);
                return true;
            }
            "escape" => {
                self.close_account_pick(window, cx);
                return true;
            }
            _ => return true,
        }
        cx.notify();
        true
    }

    pub(super) fn render_account_pick(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pick = self.account_pick.as_ref()?;
        let brand_id = match pick.agent {
            AuthAgent::Claude => "claude",
            AuthAgent::Codex => "codex",
        };
        // "Claude Code", or "Claude Code · opus" for a model picked in the launcher.
        let what = pick.choice.label().unwrap_or_else(|| crate::brand::brand(brand_id).name.to_string());
        let current = self.accounts.get(pick.agent);
        let now = crate::ui::now_ms();

        let mut list = div().flex().flex_col().gap_1();
        for (index, id) in pick.logins.iter().enumerate() {
            let row = self.account_rows.iter().find(|row| row.agent == pick.agent && row.id == *id);
            let name = self.login_name(pick.agent, id.as_deref(), cx);
            let here = id.as_deref() == current;
            let selected = index == pick.selected;
            // Under the name: the plan and its limits, or that the login needs renewing.
            let (detail, detail_color) = match row {
                Some(row) => match row.login_warning(now, cx) {
                    Some(warning) => (Some(warning.to_string()), Chrome::WARNING),
                    None => {
                        let plan = row.account.plan_name();
                        let usage = Some(usage_line(row, cx)).filter(|line| line != "-");
                        let parts: Vec<String> = [plan, usage].into_iter().flatten().collect();
                        ((!parts.is_empty()).then(|| parts.join(" · ")), Chrome::MUTED)
                    }
                },
                None => (None, Chrome::MUTED),
            };
            let initial = row
                .and_then(|row| name_of(&row.account))
                .or(Some(name.as_str()))
                .and_then(|n| n.chars().next())
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_default();
            list = list.child(
                div()
                    .id(("account-pick", index))
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_3()
                    .rounded_lg()
                    .border_1()
                    .cursor_pointer()
                    .map(|d| {
                        if selected {
                            d.bg(hex_alpha(Chrome::ACCENT, 0.14)).border_color(hex_alpha(Chrome::ACCENT, 0.55))
                        } else {
                            d.border_color(hex_alpha(0, 0.)).hover(|s| s.bg(hex(Chrome::HOVER)))
                        }
                    })
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if let Some(pick) = this.account_pick.as_mut().filter(|p| *hovered && p.selected != index) {
                            pick.selected = index;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.start_on_login(index, window, cx)))
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(px(30.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(hex_alpha(Chrome::ACCENT, if here { 0.9 } else { 0.22 }))
                            .text_color(hex(Chrome::BRIGHT))
                            .t_body()
                            .font_weight(crate::theme::EMPHASIS)
                            .child(initial),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(div().t_body().text_color(hex(Chrome::BRIGHT)).truncate().child(name))
                            .children(detail.map(|detail| div().t_caption().text_color(hex(detail_color)).truncate().child(detail))),
                    )
                    .when(here, |d| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .px_2()
                                .py_0p5()
                                .rounded_full()
                                .t_caption()
                                .bg(hex_alpha(Chrome::ACCENT, 0.18))
                                .text_color(hex(Chrome::ACCENT))
                                .child(t(cx, "account.this_window")),
                        )
                    }),
            );
        }

        Some(
            div()
                .id("account-pick-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0x000000, 0.45))
                .occlude()
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.close_account_pick(window, cx)))
                .child(
                    div()
                        .id("account-pick")
                        .w(px(400.))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(Chrome::OVERLAY))
                        .border_1()
                        .border_color(hex(Chrome::OVERLAY_BORDER))
                        .shadow_lg()
                        // Clicking the dialog itself must not count as clicking the backdrop.
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div().px_1().flex().items_center().gap_2p5().child(crate::brand::avatar_colored(brand_id, 26.)).child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .t_body()
                                            .font_weight(crate::theme::EMPHASIS)
                                            .text_color(hex(Chrome::BRIGHT))
                                            .truncate()
                                            .child(what),
                                    )
                                    .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(t(cx, "account.launch_title"))),
                            ),
                        )
                        .child(list),
                )
                .into_any_element(),
        )
    }
}
