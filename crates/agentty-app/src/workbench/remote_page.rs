//! Remote access on the workbench side: what the remote page may see and touch (every terminal
//! outside plugin workspaces), its input applied the way typing would, and Settings → Remote.

use super::settings_page::{row_with_hint, switch};
use super::Workbench;
use crate::i18n::{t, tf};
use crate::remote::server::Command;
use crate::remote::snapshot::{Screen, SessionInfo, TabInfo, WorkspaceInfo};
use crate::remote::{self, Phase, Problem};
use crate::settings::{settings, terminal_theme, update_settings};
use crate::terminal::AgentStatus;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{action_button, TypeScale};
use gpui::{div, prelude::*, ClickEvent, Context, Div, Entity, Window};

const TAILSCALE_ADMIN_DNS: &str = "https://login.tailscale.com/admin/dns";
const TAILSCALE_IOS: &str = "https://apps.apple.com/app/tailscale/id1470499037";
const TAILSCALE_ANDROID: &str = "https://play.google.com/store/apps/details?id=com.tailscale.ipn";
const TAILSCALE_DOWNLOAD: &str = "https://tailscale.com/download";

#[derive(Default)]
pub(super) struct RemotePageState {
    /// The address's QR code, kept while the address stays the same: (address, rows of modules).
    qr: Option<(String, Vec<Vec<bool>>)>,
    password: Option<Entity<TextInput>>,
    confirm: Option<Entity<TextInput>>,
    /// The last thing to tell about the password: (text, is an error).
    message: Option<(String, bool)>,
    saving: bool,
    /// The password popup is open, and what it has to say (a rule not met, a save that failed).
    dialog_open: bool,
    dialog_error: Option<String>,
    input_events: Vec<gpui::Subscription>,
    looked_up: bool,
}

impl Workbench {
    /// Terminals the remote page may use: everything except plugins' own workspaces, whose
    /// automations are not the user's to type into.
    fn remote_panes(&self) -> impl Iterator<Item = (&super::Workspace, super::Pane)> + '_ {
        self.workspaces
            .iter()
            .filter(|ws| ws.plugin.is_none() && !ws.remote_hidden)
            .flat_map(|ws| ws.tabs.iter().flat_map(|tab| tab.root.leaves()).map(move |p| (ws, p)))
    }

    /// What the remote page shows, laid out as the sidebar is: workspaces in its order (outside
    /// any group first, then each group's), each with its tabs and their split panes, and every
    /// terminal's state.
    pub fn remote_overview(&self, cx: &gpui::App) -> (Vec<WorkspaceInfo>, Vec<SessionInfo>) {
        let hex_color = |c: u32| format!("#{c:06x}");
        let user = |ws: &&super::Workspace| ws.plugin.is_none() && !ws.remote_hidden;
        let ungrouped_label = (!self.groups.is_empty()).then(|| t(cx, "ungrouped").to_string());
        let mut order: Vec<(&super::Workspace, Option<&super::Group>)> =
            self.workspaces.iter().filter(user).filter(|ws| ws.group.is_none()).map(|ws| (ws, None)).collect();
        for group in &self.groups {
            order.extend(self.workspaces.iter().filter(user).filter(|ws| ws.group == Some(group.id)).map(|ws| (ws, Some(group))));
        }
        let mut workspaces = Vec::new();
        let mut sessions = Vec::new();
        for (ws, group) in order {
            let name = self.workspace_label(ws, cx);
            let active = ws.tabs.get(ws.active_tab).map(|t| t.active.read(cx));
            let cwd = active.map(|v| v.display_cwd()).unwrap_or_else(|| ws.cwd.clone());
            let branch = active
                .and_then(|v| v.git_branch.clone())
                .or_else(|| self.folder_branch(&cwd).map(str::to_string))
                .or_else(|| ws.dormant.as_ref().and_then(|d| d.branch.clone()));
            let mut tabs = Vec::new();
            for (index, tab) in ws.tabs.iter().enumerate() {
                let panes: Vec<u64> = tab.root.leaves().iter().map(|p| p.read(cx).pane_id).collect();
                tabs.push(TabInfo { title: tab.active.read(cx).display_title(), active_pane: tab.active.read(cx).pane_id, panes });
                for pane in tab.root.leaves() {
                    let view = pane.read(cx);
                    let (status, color) = super::status_label(view, cx);
                    let asks = match &view.status {
                        AgentStatus::Permission(Some(what)) | AgentStatus::Question(Some(what)) => Some(what.chars().take(300).collect()),
                        _ => None,
                    };
                    sessions.push(SessionInfo {
                        pane: view.pane_id,
                        workspace: name.clone(),
                        workspace_id: ws.id,
                        tab: index,
                        title: view.display_title(),
                        tool: view.tool_id().to_string(),
                        status,
                        color: hex_color(color),
                        needs_user: view.status.needs_user(),
                        working: view.status.in_turn(),
                        unread: view.attention,
                        elapsed: view.working_since.map(|t| t.elapsed().as_secs()),
                        last_activity_ms: view.last_activity_ms,
                        asks,
                    });
                }
            }
            workspaces.push(WorkspaceInfo {
                id: ws.id,
                name,
                group: group.map(|g| g.name.clone()).or_else(|| ungrouped_label.clone()),
                group_color: group.and_then(|g| super::accent_color(g.color)).map(hex_color),
                color: super::accent_color(ws.color).map(hex_color),
                branch,
                folder: crate::ui::tilde(&cwd),
                sleeping: ws.tabs.is_empty(),
                active_tab: ws.active_tab.min(tabs.len().saturating_sub(1)),
                tabs,
            });
        }
        (workspaces, sessions)
    }

    pub fn remote_screen(&self, pane_id: u64, cx: &gpui::App) -> Option<Screen> {
        let (_, pane) = self.remote_panes().find(|(_, p)| p.read(cx).pane_id == pane_id)?;
        let theme = terminal_theme(cx).clone();
        pane.read(cx).remote_screen(&theme)
    }

    /// Applies one of the page's inputs if its terminal is in this window.
    pub fn remote_apply(&mut self, command: &Command, cx: &mut Context<Self>) -> bool {
        let Some((_, pane)) = self.remote_panes().find(|(_, p)| p.read(cx).pane_id == command.pane()) else { return false };
        pane.update(cx, |view, cx| match command {
            Command::Key { key, ctrl, alt, shift, .. } => {
                let modifiers = gpui::Modifiers { control: *ctrl, alt: *alt, shift: *shift, ..Default::default() };
                view.remote_key(&gpui::Keystroke { modifiers, key: key.clone(), key_char: None }, cx);
            }
            Command::Text { text, .. } => view.remote_text(text, cx),
            Command::Prompt { text, .. } => view.submit_prompt(text.clone(), cx),
            Command::Resize { cols, rows, .. } => view.set_remote_size(Some((*cols as usize, *rows as usize)), cx),
            Command::Release { .. } => view.set_remote_size(None, cx),
        });
        true
    }

    /// Leaves a workspace out of remote access, or lets it back in. Left out, the page loses it at
    /// its next update (its terminals refuse input from then on) and its terminals go back to
    /// their panes' size.
    pub(super) fn toggle_remote_hidden(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == id) else { return };
        ws.remote_hidden = !ws.remote_hidden;
        if ws.remote_hidden {
            let panes: Vec<_> = ws.tabs.iter().flat_map(|t| t.root.leaves()).collect();
            for pane in panes {
                pane.update(cx, |view, cx| view.set_remote_size(None, cx));
            }
        }
        self.workspace_menu = None;
        self.persist(cx);
        cx.notify();
    }

    /// Every terminal back to its pane's own size (the server stopped).
    pub fn remote_release_all(&mut self, cx: &mut Context<Self>) {
        let panes: Vec<_> = self.remote_panes().map(|(_, pane)| pane.clone()).collect();
        for pane in panes {
            pane.update(cx, |view, cx| view.set_remote_size(None, cx));
        }
    }

    /// The Remote access page, opened from its activity-bar item.
    pub(super) fn render_remote_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("remote-page")
            .size_full()
            .overflow_y_scroll()
            .child(div().w_full().px_6().py_5().child(self.render_remote_settings(window, cx)))
    }

    /// Redraws the Remote access page when it is the one showing (its counters and log move).
    pub fn refresh_remote_page(&mut self, cx: &mut Context<Self>) {
        if self.page == Some(super::Page::Remote) {
            cx.notify();
        }
    }

    /// The feature was turned off: its page goes with it.
    pub fn close_remote_page(&mut self, cx: &mut Context<Self>) {
        if self.page == Some(super::Page::Remote) {
            self.page = None;
            cx.notify();
        }
    }

    /// Debug driver: `on`, `off`, `dialog`, `disable`, `state` (printed; never the password).
    /// There is no way to set the password from here, in any build: it is only ever typed into
    /// the popup by the user. Turning it on only in a debug build: in a release, anything running
    /// as this user could otherwise open the Mac's terminals to the tailnet through the driver's
    /// socket whenever `AGENTTY_DEBUG=1` happens to be set.
    pub(super) fn debug_remote(&mut self, argument: &str, window: &mut Window, cx: &mut Context<Self>) {
        let dev = cfg!(debug_assertions);
        match argument.split_once(' ').unwrap_or((argument, "")) {
            ("on", _) if dev => remote::set_enabled(true, cx),
            ("off", _) => remote::set_enabled(false, cx),
            ("dialog", _) if dev => self.open_password_dialog(window, cx),
            // What the page's "Disable remote access" button does, from inside a workbench update as it is.
            ("disable", _) => {
                remote::set_feature(false, cx);
                self.page = Some(super::Page::Settings);
                self.settings_section = super::settings_page::SettingsSection::General;
                cx.notify();
            }
            _ => {
                let state = cx.global::<remote::Remote>();
                eprintln!(
                    "remote: phase={:?} password_set={:?} tailscale={:?} devices={} log={:?}",
                    state.phase,
                    state.password_set,
                    state.tailscale,
                    remote::hub(cx).map_or(0, |h| h.devices().len()),
                    remote::hub(cx).map(|h| h.log().iter().map(|e| e.what).collect::<Vec<_>>()).unwrap_or_default()
                );
            }
        }
    }

    /// The address as a QR code a phone's camera opens: dark modules on white, with the quiet
    /// border scanners need, drawn as runs of cells.
    fn remote_qr(&mut self, url: &str, hint: &'static str) -> gpui::AnyElement {
        const CELL: f32 = 4.;
        const QUIET: usize = 3;
        if self.remote_page.qr.as_ref().is_none_or(|(cached, _)| cached != url) {
            let modules = qrcode::QrCode::new(url.as_bytes()).ok().map(|code| {
                let width = code.width();
                let colors = code.to_colors();
                (0..width).map(|y| (0..width).map(|x| colors[y * width + x] == qrcode::Color::Dark).collect()).collect()
            });
            self.remote_page.qr = modules.map(|m| (url.to_string(), m));
        }
        let Some((_, modules)) = &self.remote_page.qr else { return div().into_any_element() };
        let mut grid = div().flex().flex_col().bg(gpui::white()).p(gpui::px(CELL * QUIET as f32)).rounded_md();
        for row in modules {
            let mut line = div().flex().h(gpui::px(CELL));
            let mut x = 0;
            while x < row.len() {
                let dark = row[x];
                let run = row[x..].iter().take_while(|m| **m == dark).count();
                line = line.child(div().w(gpui::px(CELL * run as f32)).h_full().when(dark, |d| d.bg(gpui::black())));
                x += run;
            }
            grid = grid.child(line);
        }
        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .child(grid)
            .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(hint))
            .into_any_element()
    }

    fn remote_input(&mut self, which: u8, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextInput> {
        let slot = if which == 0 { &mut self.remote_page.password } else { &mut self.remote_page.confirm };
        if let Some(input) = slot {
            return input.clone();
        }
        let hint = if which == 0 { t(cx, "remote.password_new") } else { t(cx, "remote.password_again") };
        let input = cx.new(|cx| TextInput::new("", hint, window, cx).masked());
        // Enter in either field saves, as the button does; Tab and Shift+Tab move between the two
        // fields; Esc closes the popup.
        let events = cx.subscribe_in(&input, window, move |this, _, event: &TextInputEvent, window, cx| match event {
            TextInputEvent::Confirmed if !this.remote_page.saving => this.save_remote_password(cx),
            TextInputEvent::Next | TextInputEvent::Previous => {
                let other = if which == 0 { this.remote_page.confirm.clone() } else { this.remote_page.password.clone() };
                if let Some(other) = other {
                    window.focus(&gpui::Focusable::focus_handle(other.read(cx), cx));
                    cx.notify();
                }
            }
            TextInputEvent::Cancelled => this.close_password_dialog(cx),
            _ => {}
        });
        self.remote_page.input_events.push(events);
        let slot = if which == 0 { &mut self.remote_page.password } else { &mut self.remote_page.confirm };
        *slot = Some(input.clone());
        input
    }

    /// Opens the password popup with empty fields, the first one focused.
    pub(super) fn open_password_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let first = self.remote_input(0, window, cx);
        let second = self.remote_input(1, window, cx);
        for input in [&first, &second] {
            input.update(cx, |input, cx| input.set_text("", cx));
        }
        window.focus(&gpui::Focusable::focus_handle(first.read(cx), cx));
        self.remote_page.dialog_open = true;
        self.remote_page.dialog_error = None;
        cx.notify();
    }

    fn close_password_dialog(&mut self, cx: &mut Context<Self>) {
        self.remote_page.dialog_open = false;
        self.remote_page.dialog_error = None;
        for input in [&self.remote_page.password, &self.remote_page.confirm].into_iter().flatten() {
            input.update(cx, |input, cx| input.set_text("", cx));
        }
        cx.notify();
    }

    /// Checks the two fields against the rules here, so the popup says what is wrong at once, then
    /// saves (hashing takes a moment) and closes on success.
    fn save_remote_password(&mut self, cx: &mut Context<Self>) {
        let (Some(first), Some(second)) = (self.remote_page.password.clone(), self.remote_page.confirm.clone()) else { return };
        let (a, b) = (first.read(cx).text().to_string(), second.read(cx).text().to_string());
        let problem = remote::auth::password_problem(&a).map(|key| t(cx, key).to_string());
        if let Some(problem) = problem.or_else(|| (a != b).then(|| t(cx, "remote.password_mismatch").to_string())) {
            self.remote_page.dialog_error = Some(problem);
            return cx.notify();
        }
        self.remote_page.saving = true;
        self.remote_page.dialog_error = None;
        cx.notify();
        let page = cx.entity().downgrade();
        remote::set_password(a, cx, move |result, cx| {
            let _ = page.update(cx, |wb, cx| {
                wb.remote_page.saving = false;
                match result {
                    Ok(()) => {
                        let saved = t(cx, "remote.password_saved").to_string();
                        wb.remote_page.message = Some((saved.clone(), false));
                        wb.close_password_dialog(cx);
                        // Said once, then gone: the button above already shows the password is set.
                        cx.spawn(async move |page, cx| {
                            cx.background_executor().timer(std::time::Duration::from_secs(4)).await;
                            let _ = page.update(cx, |wb, cx| {
                                if wb.remote_page.message.as_ref().is_some_and(|(text, _)| *text == saved) {
                                    wb.remote_page.message = None;
                                    cx.notify();
                                }
                            });
                        })
                        .detach();
                    }
                    Err(err) => wb.remote_page.dialog_error = Some(err),
                }
                cx.notify();
            });
        });
    }

    /// The password popup: two masked fields, the rules, and what is wrong; removing the
    /// password lives here too.
    pub(super) fn render_remote_password_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.remote_page.dialog_open {
            return None;
        }
        let password_set = cx.try_global::<remote::Remote>().is_some_and(|r| r.password_set == Some(true));
        let first = self.remote_input(0, window, cx);
        let second = self.remote_input(1, window, cx);
        let field = |input: Entity<TextInput>| {
            let focus = input.clone();
            div()
                .w_full()
                .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                    window.focus(&gpui::Focusable::focus_handle(&focus, cx));
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .cursor_text()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(hex_alpha(NEON_CYAN, 0.35))
                .bg(hex(0x0b0e11))
                .t_body()
                .child(input)
        };
        let saving = self.remote_page.saving;
        Some(
            div()
                .id("remote-password-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0x000000, 0.55))
                .occlude()
                .child(
                    div()
                        .w(gpui::px(420.))
                        .p_5()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_xl()
                        .bg(hex(PANEL))
                        .border_1()
                        .border_color(hex_alpha(NEON_CYAN, 0.4))
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().w(gpui::px(3.)).h(gpui::px(16.)).rounded_sm().bg(hex(NEON_CYAN)))
                                .child(div().t_caption().text_color(hex(NEON_CYAN)).child("KEY.WEB"))
                                .child(
                                    div()
                                        .t_title()
                                        .font_weight(crate::theme::EMPHASIS)
                                        .text_color(hex(Chrome::BRIGHT))
                                        .child(t(cx, "remote.password_dialog")),
                                ),
                        )
                        .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "remote.password_rules")))
                        .child(field(first))
                        .child(field(second))
                        .children(self.remote_page.dialog_error.clone().map(|e| div().t_small().text_color(hex(Chrome::ERROR)).child(e)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .pt_1()
                                .when(password_set, |d| {
                                    d.child(action_button(
                                        "remote-remove-password",
                                        t(cx, "remote.remove_password"),
                                        cx.listener(|this, _: &ClickEvent, _, cx| {
                                            remote::remove_password(cx);
                                            this.remote_page.message = Some((t(cx, "remote.password_removed").to_string(), false));
                                            this.close_password_dialog(cx);
                                        }),
                                    ))
                                })
                                .child(div().flex_1())
                                .child(action_button(
                                    "remote-password-cancel",
                                    t(cx, "remote.cancel"),
                                    cx.listener(|this, _: &ClickEvent, _, cx| this.close_password_dialog(cx)),
                                ))
                                .child(action_button(
                                    "remote-save-password",
                                    if saving { t(cx, "remote.saving") } else { t(cx, "remote.save_password") },
                                    cx.listener(|this, _: &ClickEvent, _, cx| {
                                        if !this.remote_page.saving {
                                            this.save_remote_password(cx)
                                        }
                                    }),
                                )),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The Remote access page as a dashboard: a status panel with the address and its QR code,
    /// live counters, the security checks, the activity log, and the cards to set it up.
    pub(super) fn render_remote_settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        if !self.remote_page.looked_up {
            self.remote_page.looked_up = true;
            remote::refresh_tailscale(cx);
        }
        let prefs = settings(cx).remote.clone();
        let state = cx.global::<remote::Remote>();
        let phase = state.phase.clone();
        let tailscale = state.tailscale.clone();
        let password_set = state.password_set == Some(true);
        let hub = remote::hub(cx);
        let mono = crate::settings::terminal_font(cx);
        let muted = |text: String| div().t_small().text_color(hex(Chrome::MUTED)).child(text);
        let online = matches!(phase, Phase::On { .. });

        // ── Status panel ─────────────────────────────────────────────────────────────────
        let (word, color) = match &phase {
            Phase::Off => (t(cx, "remote.dash.offline"), Chrome::MUTED),
            Phase::Starting => (t(cx, "remote.dash.starting"), NEON_AMBER),
            Phase::On { .. } => (t(cx, "remote.dash.online"), NEON_GREEN),
            Phase::Failed(_) => (t(cx, "remote.dash.error"), Chrome::ERROR),
        };
        let detail = match &phase {
            Phase::Off => t(cx, "remote.off").to_string(),
            Phase::Starting => t(cx, "remote.starting").to_string(),
            Phase::On { .. } => t(cx, "remote.on").to_string(),
            Phase::Failed(problem) => match problem {
                Problem::NotInstalled => t(cx, "remote.problem_not_installed").to_string(),
                Problem::NotRunning => t(cx, "remote.problem_not_running").to_string(),
                Problem::NoHttps => t(cx, "remote.problem_no_https").to_string(),
                Problem::NoPassword => t(cx, "remote.problem_no_password").to_string(),
                Problem::NeedsEnabling(_) => t(cx, "remote.problem_enable_serve").to_string(),
                Problem::Funnel => t(cx, "remote.problem_funnel").to_string(),
                Problem::PortTaken(port) => tf(cx, "remote.problem_port_taken", &[("port", &port.to_string())]),
                Problem::Other(text) => tf(cx, "remote.problem_other", &[("reason", text)]),
            },
        };
        let lamp = div().size(gpui::px(12.)).rounded_full().bg(hex(color)).when(online, |d| {
            d.shadow(vec![gpui::BoxShadow {
                color: hex_alpha(NEON_GREEN, 0.75),
                offset: gpui::point(gpui::px(0.), gpui::px(0.)),
                blur_radius: gpui::px(12.),
                spread_radius: gpui::px(1.),
            }])
        });
        let mut status = div()
            .flex_1()
            .min_w(gpui::px(300.))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div().flex().items_center().gap_3().child(lamp).child(
                    div()
                        .font_family(mono.clone())
                        .text_size(gpui::px(30.))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(hex(color))
                        .child(word),
                ),
            )
            .child(div().t_body().text_color(hex(Chrome::BRIGHT)).child(detail));
        if let Phase::On { url } = &phase {
            let (copy, open) = (url.clone(), url.clone());
            status = status.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(hex_alpha(NEON_CYAN, 0.35))
                    .bg(hex_alpha(NEON_CYAN, 0.06))
                    .child(div().font_family(mono.clone()).text_color(hex(NEON_CYAN)).child("›"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(mono.clone())
                            .t_body()
                            .text_color(hex(NEON_CYAN))
                            .child(url.clone()),
                    )
                    .child(action_button("remote-copy", t(cx, "remote.copy"), move |_, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()))
                    }))
                    .child(action_button("remote-open", t(cx, "remote.open"), move |_, _, cx| cx.open_url(&open))),
            );
        }
        match &phase {
            Phase::Failed(Problem::NeedsEnabling(url)) => {
                let url = url.clone();
                status = status.child(
                    div().child(action_button("remote-enable-serve", t(cx, "remote.enable_serve"), move |_, _, cx| cx.open_url(&url))),
                );
            }
            Phase::Failed(Problem::NoHttps) => {
                status = status
                    .child(div().child(action_button("remote-admin-dns", t(cx, "remote.open_admin_dns"), |_, _, cx| {
                        cx.open_url(TAILSCALE_ADMIN_DNS)
                    })))
                    .child(muted(t(cx, "remote.https_note").to_string()));
            }
            Phase::Failed(Problem::NotInstalled) => {
                status =
                    status.child(div().child(action_button("remote-install", t(cx, "remote.install_tailscale"), |_, _, cx| {
                        cx.open_url(TAILSCALE_DOWNLOAD)
                    })));
            }
            _ => {}
        }
        // No password, no remote access: turning it on then asks for one first.
        let can_turn_on = password_set;
        let on = prefs.enabled;
        status = status.child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .pt_1()
                .child(switch(
                    "remote-enabled",
                    on,
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        if on {
                            remote::set_enabled(false, cx);
                        } else if can_turn_on {
                            remote::set_enabled(true, cx);
                        } else {
                            this.open_password_dialog(window, cx);
                        }
                    }),
                ))
                .child(div().flex().flex_col().child(div().t_body().text_color(hex(Chrome::BRIGHT)).child(t(cx, "remote.enable"))).child(
                    muted(if can_turn_on { t(cx, "remote.enable_hint") } else { t(cx, "remote.enable_needs_password") }.to_string()),
                )),
        );
        let mut hero = div().flex().flex_wrap().gap_6().items_start().child(status);
        if let Phase::On { url } = &phase {
            let hint = t(cx, "remote.qr_hint");
            hero = hero.child(self.remote_qr(url, hint));
        }
        let hero = card("SYS.LINK", t(cx, "page.remote")).child(muted(t(cx, "remote.intro").to_string())).child(hero);

        // ── Counters ─────────────────────────────────────────────────────────────────────
        let (panes, waiting) = self.remote_panes().fold((0, 0), |(n, w), (_, p)| (n + 1, w + p.read(cx).status.needs_user() as usize));
        let devices_now = hub.as_ref().map_or(0, |h| h.devices().len());
        let pages_now = hub.as_ref().map_or(0, |h| h.viewers());
        let ts_value = match &tailscale {
            Some(s) if s.running => t(cx, "remote.dash.ts_on").to_string(),
            Some(s) if s.installed => t(cx, "remote.dash.ts_off").to_string(),
            Some(_) => t(cx, "remote.dash.ts_missing").to_string(),
            None => "…".to_string(),
        };
        let ts_sub = tailscale.as_ref().and_then(|s| s.dns_name.clone()).unwrap_or_default();
        let stat = |code: &'static str, label: &str, value: String, sub: String, accent: u32| {
            div()
                .flex_1()
                .min_w(gpui::px(150.))
                .flex()
                .flex_col()
                .gap_1()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(hex_alpha(accent, 0.3))
                .bg(hex(PANEL))
                .child(div().font_family(mono.clone()).t_caption().text_color(hex(accent)).child(format!("{code} · {label}")))
                .child(
                    div()
                        .font_family(mono.clone())
                        .text_size(gpui::px(24.))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(hex(Chrome::BRIGHT))
                        .child(value),
                )
                .child(div().t_caption().truncate().text_color(hex(Chrome::MUTED)).child(sub))
        };
        let stats = div()
            .flex()
            .flex_wrap()
            .gap_3()
            .child(stat(
                "DEV",
                t(cx, "remote.dash.devices"),
                devices_now.to_string(),
                t(cx, "remote.dash.devices_sub").to_string(),
                NEON_CYAN,
            ))
            .child(stat("WEB", t(cx, "remote.dash.pages"), pages_now.to_string(), t(cx, "remote.dash.pages_sub").to_string(), NEON_CYAN))
            .child(stat(
                "TTY",
                t(cx, "remote.dash.sessions"),
                panes.to_string(),
                tf(cx, "remote.dash.waiting", &[("n", &waiting.to_string())]),
                if waiting > 0 { NEON_AMBER } else { NEON_CYAN },
            ))
            .child(stat(
                "NET",
                "Tailscale",
                ts_value,
                ts_sub,
                if tailscale.as_ref().is_some_and(|s| s.running) { NEON_GREEN } else { Chrome::MUTED },
            ));

        // ── Security checks ──────────────────────────────────────────────────────────────
        let login = tailscale.as_ref().and_then(|s| s.login.clone());
        let check = |ok: Option<bool>, label: String, sub: String| {
            let (mark, color) = match ok {
                Some(true) => ("✓", NEON_GREEN),
                Some(false) => ("✗", Chrome::ERROR),
                None => ("·", Chrome::MUTED),
            };
            div()
                .flex()
                .items_start()
                .gap_3()
                .child(
                    div()
                        .w(gpui::px(16.))
                        .flex_shrink_0()
                        .font_family(mono.clone())
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(hex(color))
                        .child(mark),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().t_body().text_color(hex(Chrome::BRIGHT)).child(label))
                        .when(!sub.is_empty(), |d| d.child(muted(sub))),
                )
        };
        let security = card("SEC.LAYERS", t(cx, "remote.sec.title"))
            .child(check(
                login.as_ref().map(|_| true),
                t(cx, "remote.sec.identity").to_string(),
                login.clone().unwrap_or_else(|| t(cx, "remote.ts_stopped").to_string()),
            ))
            .child(check(Some(password_set), t(cx, "remote.sec.password").to_string(), t(cx, "remote.sec.password_sub").to_string()))
            .child(check(
                tailscale.as_ref().map(|s| s.https),
                t(cx, "remote.sec.https").to_string(),
                tailscale.as_ref().and_then(|s| s.dns_name.clone()).unwrap_or_default(),
            ))
            .child(check(online.then_some(true), t(cx, "remote.sec.private").to_string(), t(cx, "remote.sec.private_sub").to_string()))
            .child(check(
                Some(true),
                t(cx, "remote.sec.local").to_string(),
                match hub.as_ref().map(|h| &h.endpoint) {
                    #[cfg(unix)]
                    Some(remote::conn::Endpoint::Unix(_)) => t(cx, "remote.sec.local_socket").to_string(),
                    Some(_) => t(cx, "remote.sec.local_secret").to_string(),
                    None => "127.0.0.1".to_string(),
                },
            ));

        // ── Activity log and devices ─────────────────────────────────────────────────────
        let mut activity = card("LOG.ACCESS", t(cx, "remote.log"));
        match &hub {
            None => activity = activity.child(muted(t(cx, "remote.devices_off").to_string())),
            Some(hub) => {
                let now = crate::ui::now_ms();
                let devices = hub.devices();
                if devices.is_empty() {
                    activity = activity.child(muted(t(cx, "remote.devices_none").to_string()));
                }
                for (device, idle) in devices {
                    activity = activity.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().size(gpui::px(7.)).rounded_full().bg(hex(NEON_GREEN)))
                            .child(div().flex_1().t_body().child(device))
                            .child(
                                div()
                                    .t_small()
                                    .text_color(hex(Chrome::MUTED))
                                    .child(crate::ui::relative_time(now, now.saturating_sub(idle.as_millis() as u64))),
                            ),
                    );
                }
                let mut lines =
                    div().flex().flex_col().gap_1().p_3().rounded_md().bg(hex(0x0b0e11)).border_1().border_color(hex(Chrome::BORDER));
                let log = hub.log();
                if log.is_empty() {
                    lines = lines.child(
                        div().font_family(mono.clone()).t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "remote.dash.no_events")),
                    );
                }
                for entry in log.iter().take(14) {
                    let warn = matches!(entry.what, "remote.log.wrong_password" | "remote.log.locked" | "remote.log.refused_login");
                    lines = lines.child(
                        div()
                            .flex()
                            .gap_2()
                            .font_family(mono.clone())
                            .t_small()
                            .child(
                                div()
                                    .w(gpui::px(80.))
                                    .flex_shrink_0()
                                    .text_color(hex(Chrome::MUTED))
                                    .child(crate::ui::relative_time(now, entry.at_ms)),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_color(hex(if warn { Chrome::ERROR } else { NEON_GREEN }))
                                    .child(t(cx, entry.what)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(hex(Chrome::MUTED))
                                    .child(format!("{} · {}", entry.login, entry.device)),
                            ),
                    );
                }
                // Locked by wrong passwords: maybe by someone else on purpose. The owner, here at
                // the Mac, can lift it.
                if let Some(left) = hub.locked_for() {
                    let minutes = left.as_secs().div_ceil(60).max(1).to_string();
                    let unlock = hub.clone();
                    activity = activity.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_3()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(hex_alpha(Chrome::ERROR, 0.5))
                            .child(div().flex_1().min_w(gpui::px(200.)).t_body().text_color(hex(Chrome::ERROR)).child(tf(
                                cx,
                                "remote.locked_now",
                                &[("minutes", &minutes)],
                            )))
                            .child(action_button("remote-unlock", t(cx, "remote.unlock"), move |_, _, cx| {
                                unlock.unlock();
                                cx.refresh_windows();
                            })),
                    );
                }
                activity = activity.child(lines).child(div().child(action_button(
                    "remote-sign-out-all",
                    t(cx, "remote.sign_out_all"),
                    |_, _, cx| remote::sign_out_everywhere(cx),
                )));
            }
        }

        // ── Web password: its state as a button; setting it happens in a popup ─────────────
        let (badge, badge_color) = if password_set {
            (format!("✓  {}", t(cx, "remote.password_is_set")), NEON_GREEN)
        } else {
            (format!("!  {}", t(cx, "remote.password_not_set")), NEON_AMBER)
        };
        let mut password = card("KEY.WEB", t(cx, "remote.password")).child(muted(t(cx, "remote.password_intro").to_string())).child(
            div()
                .id("remote-password-state")
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .px_4()
                .py_3()
                .rounded_md()
                .cursor_pointer()
                .border_1()
                .border_color(hex_alpha(badge_color, 0.55))
                .bg(hex_alpha(badge_color, 0.08))
                .hover(|s| s.bg(hex_alpha(badge_color, 0.16)))
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.open_password_dialog(window, cx)))
                .child(
                    div().font_family(mono.clone()).t_body().font_weight(gpui::FontWeight::BOLD).text_color(hex(badge_color)).child(badge),
                )
                .child(div().t_small().text_color(hex(badge_color)).child(if password_set {
                    t(cx, "remote.password_change")
                } else {
                    t(cx, "remote.password_set_now")
                })),
        );
        if let Some((text, error)) = &self.remote_page.message {
            password = password.child(div().t_small().text_color(hex(if *error { Chrome::ERROR } else { NEON_GREEN })).child(text.clone()));
        }

        // ── From another device ──────────────────────────────────────────────────────────
        let login_text = login.unwrap_or_else(|| "—".into());
        let url = match &phase {
            Phase::On { url } => Some(url.clone()),
            _ => None,
        };
        let step = |n: &str, text: String| {
            div()
                .flex()
                .gap_2()
                .t_small()
                .child(
                    div()
                        .flex_shrink_0()
                        .size(gpui::px(20.))
                        .rounded_md()
                        .border_1()
                        .border_color(hex_alpha(NEON_CYAN, 0.5))
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_family(mono.clone())
                        .t_caption()
                        .text_color(hex(NEON_CYAN))
                        .child(n.to_string()),
                )
                .child(div().flex_1().min_w_0().pt(gpui::px(2.)).child(text))
        };
        let guide = card("OPS.CONNECT", t(cx, "remote.howto"))
            .child(step("1", t(cx, "remote.howto_install").to_string()))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .pl(gpui::px(28.))
                    .child(action_button("remote-app-store", "App Store", |_, _, cx| cx.open_url(TAILSCALE_IOS)))
                    .child(action_button("remote-play-store", "Google Play", |_, _, cx| cx.open_url(TAILSCALE_ANDROID)))
                    .child(action_button("remote-other-download", t(cx, "remote.howto_other_devices"), |_, _, cx| {
                        cx.open_url(TAILSCALE_DOWNLOAD)
                    })),
            )
            .child(step("2", tf(cx, "remote.howto_account", &[("login", &login_text)])))
            .child(step(
                "3",
                match &url {
                    Some(url) => tf(cx, "remote.howto_open", &[("url", url)]),
                    None => t(cx, "remote.howto_open_later").to_string(),
                },
            ))
            .child(step("4", t(cx, "remote.howto_password").to_string()));

        // ── Options ──────────────────────────────────────────────────────────────────────
        let options = card("CFG.OPTIONS", t(cx, "remote.options"))
            .child(row_with_hint(
                t(cx, "remote.keep_awake"),
                t(cx, "remote.keep_awake_hint"),
                switch("remote-keep-awake", prefs.keep_awake, move |_, _, cx| {
                    update_settings(cx, |s| s.remote.keep_awake = !s.remote.keep_awake);
                    // Takes effect now: restarting re-reads it (and keeps the password, not the sessions).
                    if settings(cx).remote.enabled {
                        remote::start(cx);
                    }
                }),
            ))
            .child(row_with_hint(
                t(cx, "remote.disable"),
                t(cx, "remote.disable_hint"),
                action_button(
                    "remote-disable-feature",
                    t(cx, "remote.disable"),
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        remote::set_feature(false, cx);
                        // Straight to where it comes back on.
                        this.page = Some(super::Page::Settings);
                        this.settings_section = super::settings_page::SettingsSection::General;
                        cx.notify();
                    }),
                ),
            ));

        // Two columns where there is room, one otherwise.
        let column = || div().flex_1().min_w(gpui::px(340.)).flex().flex_col().gap_4();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(hero)
            .child(stats)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_4()
                    .items_start()
                    .child(column().child(security).child(options))
                    .child(column().child(activity))
                    .child(column().child(password).child(guide)),
            )
            .child(muted(tf(cx, "remote.warning", &[("login", &login_text)])))
    }
}

/// The dashboard's colours: phosphor green for what is live, cyan for the network, amber for
/// what waits, on a panel a shade off the background.
pub(super) const NEON_GREEN: u32 = 0x3dff9a;
const NEON_CYAN: u32 = 0x35d4e8;
const NEON_AMBER: u32 = 0xffb340;
const PANEL: u32 = 0x12171c;

/// A dashboard card: a code label in the network colour beside its title, on a framed panel.
fn card(code: &'static str, title: impl Into<gpui::SharedString>) -> Div {
    div().flex().flex_col().gap_3().p_4().rounded_lg().border_1().border_color(hex_alpha(NEON_CYAN, 0.22)).bg(hex(PANEL)).child(
        div()
            .flex()
            .items_center()
            .gap_2()
            .pb_2()
            .border_b_1()
            .border_color(hex_alpha(NEON_CYAN, 0.12))
            .child(div().w(gpui::px(3.)).h(gpui::px(14.)).rounded_sm().bg(hex(NEON_CYAN)))
            .child(div().t_caption().text_color(hex(NEON_CYAN)).child(code))
            .child(div().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(title.into())),
    )
}
