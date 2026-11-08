//! The account icon above Settings: which logins this machine has, what each has left of its plan
//! limits, adding another one. An account runs in a window of its own, next to the others, so
//! picking one brings its window forward instead of changing what is on screen.

use super::ask::{Ask, AskAction, AskChoice};
use super::Workbench;
use crate::i18n::{t, tf};
use crate::launch::LaunchSpec;
use crate::theme::{hex, Chrome};
use crate::ui::{icon, popover, IconSize, TypeScale};
use agentty_bridge::accounts::{self, Account, AccountStore, Added};
use agentty_bridge::agent_auth::{self, AuthAgent};
use agentty_bridge::limits::AgentLimits;
use agentty_bridge::model::Agent;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, SharedString, Window};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MENU_KEY: &str = "account-menu";
const LOGIN_POLL: Duration = Duration::from_secs(2);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
pub(super) const MENU_WIDTH: f32 = 320.;

/// One line of the menu. `id` is `None` for the agent's own login.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct AccountRow {
    pub agent: AuthAgent,
    pub id: Option<String>,
    pub account: Account,
    pub limits: Option<AgentLimits>,
    /// Whether the CLI says this login works right now.
    pub signed_in: bool,
}

impl AccountRow {
    /// Signed out, or about to be: the row offers to sign in again.
    pub(super) fn needs_login(&self, now_ms: u64) -> bool {
        !self.signed_in || self.account.renew_soon(now_ms)
    }

    /// "Signed out" or "Login ends soon", when one applies.
    pub(super) fn login_warning(&self, now_ms: u64, cx: &gpui::App) -> Option<&'static str> {
        if !self.signed_in {
            Some(t(cx, "account.signed_out"))
        } else if self.account.renew_soon(now_ms) {
            Some(t(cx, "account.ends_soon"))
        } else {
            None
        }
    }
}

fn agent_of(agent: AuthAgent) -> Agent {
    match agent {
        AuthAgent::Claude => Agent::Claude,
        AuthAgent::Codex => Agent::Codex,
    }
}

/// What the CLI's own status says about the login in `folder` (the user's own without one).
/// Runs a process: off the UI thread.
fn claude_login(folder: Option<&Path>) -> Option<Account> {
    let mut command = Command::new(crate::agent_bins::program("claude"));
    command.args(["auth", "status", "--json"]).stdin(Stdio::null()).stderr(Stdio::null());
    if let Some(folder) = folder {
        // A key in the app's own environment would answer "logged in" for an empty folder.
        command.env("CLAUDE_CONFIG_DIR", folder);
        for name in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN"] {
            command.env_remove(name);
        }
    }
    agentty_bridge::process::hide_window(&mut command);
    let output = command.output().ok()?;
    accounts::parse_claude_status(&serde_json::from_slice(&output.stdout).ok()?)
}

/// The login in a Codex folder. A login Codex keeps outside `auth.json` (its keyring store) is
/// still a login, only one Agentty can't name.
fn codex_login(folder: &Path) -> Option<Account> {
    if let Some(account) = accounts::read_codex_login(folder) {
        return Some(account);
    }
    let mut command = Command::new(crate::agent_bins::program("codex"));
    command.args(["login", "status"]).env("CODEX_HOME", folder).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    agentty_bridge::process::hide_window(&mut command);
    command.status().ok()?.success().then(Account::default)
}

fn login_in(agent: AuthAgent, folder: &Path) -> Option<Account> {
    match agent {
        AuthAgent::Claude => claude_login(Some(folder)),
        AuthAgent::Codex => codex_login(folder),
    }
}

fn own_login(agent: AuthAgent) -> Option<Account> {
    match agent {
        AuthAgent::Claude => claude_login(None),
        AuthAgent::Codex => codex_login(&agent_auth::user_codex_home()),
    }
}

/// Signs out and deletes an account's folder. Signing out comes first: the CLI's Keychain entry
/// isn't in the folder. A folder that never signed in has nothing to sign out of, and isn't asked
/// to. Runs a process: off the UI thread.
fn discard_folder(agent: AuthAgent, id: &str) -> anyhow::Result<()> {
    let folder = accounts::home(agent, id)?;
    if agent == AuthAgent::Claude && claude_login(Some(&folder)).is_some() {
        let mut command = Command::new(crate::agent_bins::program("claude"));
        command.args(["auth", "logout"]).env("CLAUDE_CONFIG_DIR", folder).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        agentty_bridge::process::hide_window(&mut command);
        let _ = command.status();
    }
    accounts::delete_folder(agent, id)
}

/// Lists a login that arrived in folder `id`, unless the same account is known already: then the
/// new folder goes and the known one is returned. `(account, newly listed)`. Off the UI thread.
fn keep_login(agent: AuthAgent, id: String, login: Account) -> anyhow::Result<(Option<String>, bool)> {
    let own = own_login(agent);
    let mut store = AccountStore::load();
    if own.is_some_and(|own| own.same_login(&login)) {
        let _ = discard_folder(agent, &id);
        return Ok((None, false));
    }
    match store.add(agent, login) {
        Added::Duplicate(known) => {
            let _ = discard_folder(agent, &id);
            Ok((Some(known), false))
        }
        Added::Listed => store.save().map(|()| (Some(id), true)),
    }
}

/// Folders left by a login the app quit during. One that signed in after all is listed; an empty
/// one goes. A folder holding sessions stays as it is (a damaged list must not cost them). Runs
/// processes: off the UI thread, once at start, before any login pane exists.
pub(crate) fn tidy_account_folders() {
    for agent in [AuthAgent::Claude, AuthAgent::Codex] {
        let store = AccountStore::load();
        for folder in accounts::homes(agent) {
            let Some(id) = folder.file_name().and_then(|name| name.to_str()).map(str::to_string) else { continue };
            if store.contains(agent, &id) || accounts::home(agent, &id).is_err() {
                continue;
            }
            match login_in(agent, &folder) {
                Some(mut login) => {
                    login.id = id.clone();
                    let _ = keep_login(agent, id, login);
                }
                None if !accounts::holds_sessions(agent, &folder) => {
                    let _ = accounts::delete_folder(agent, &id);
                }
                None => {}
            }
        }
    }
}

/// Every account's line: the agent's own login, then the others. Off the UI thread.
fn read_rows(now_ms: u64) -> Vec<AccountRow> {
    let store = AccountStore::load();
    let mut rows = Vec::new();
    for agent in [AuthAgent::Claude, AuthAgent::Codex] {
        let own = own_login(agent);
        rows.push(AccountRow {
            agent,
            id: None,
            signed_in: own.is_some(),
            account: own.unwrap_or_default(),
            limits: agentty_bridge::limits::latest(agent_of(agent), None, now_ms),
        });
        for account in store.of(agent) {
            let live = accounts::home(agent, &account.id).ok().and_then(|folder| login_in(agent, &folder));
            rows.push(AccountRow {
                agent,
                id: Some(account.id.clone()),
                signed_in: live.is_some(),
                account: live.map_or_else(|| account.clone(), |live| account.refreshed(live)),
                limits: agentty_bridge::limits::latest(agent_of(agent), Some(&account.id), now_ms),
            });
        }
    }
    rows
}

/// "5h 12% · weekly 40%", or nothing before the status line recorded any limits.
pub(super) fn usage_line(row: &AccountRow, cx: &gpui::App) -> Option<String> {
    let windows = row.limits.iter().flat_map(|limits| [("tray.session", limits.session), ("tray.weekly", limits.weekly)]);
    let used: Vec<String> =
        windows.filter_map(|(label, window)| window.map(|w| format!("{} {:.0}%", t(cx, label), w.used_percent))).collect();
    (!used.is_empty()).then(|| used.join(" · "))
}

/// The line for an account name: email, else the organization.
pub(super) fn name_of(account: &Account) -> Option<&str> {
    account.email.as_deref().or(account.org.as_deref())
}

/// What a window is told apart by: the accounts it runs other than the agents' own.
pub(super) fn window_label(choice: &accounts::AccountChoice) -> Option<String> {
    if choice.claude.is_none() && choice.codex.is_none() {
        return None;
    }
    let store = AccountStore::load();
    let name = |agent: AuthAgent| {
        let id = choice.get(agent)?;
        store.of(agent).iter().find(|account| account.id == id).and_then(name_of).map(str::to_string)
    };
    let mut names: Vec<String> = [name(AuthAgent::Claude), name(AuthAgent::Codex)].into_iter().flatten().collect();
    names.dedup();
    (!names.is_empty()).then(|| names.join(" · "))
}

impl Workbench {
    /// Fills in what a new pane runs on. A session goes on with the account that holds its
    /// transcript, wherever it is resumed from. Everything else takes this window's accounts, except
    /// where the spec already names one (a login pane).
    pub(super) fn choose_accounts(&self, spec: &mut LaunchSpec) {
        spec.accounts.fill_from(&self.accounts);
        let (crate::launch::Start::Resume(id) | crate::launch::Start::Fork(id)) = &spec.start else { return };
        let agent = match spec.kind {
            crate::launch::PaneKind::Claude => AuthAgent::Claude,
            crate::launch::PaneKind::Codex => AuthAgent::Codex,
            crate::launch::PaneKind::Shell => return,
        };
        if let Some(path) = agentty_bridge::transcript_path(agent_of(agent), id) {
            spec.accounts.set(agent, accounts::owner_of(agent, &path));
        }
    }

    /// A saved window's accounts, without those removed since.
    pub(super) fn restore_accounts(&mut self, saved: &accounts::AccountChoice) {
        let store = AccountStore::load();
        for agent in [AuthAgent::Claude, AuthAgent::Codex] {
            let id = saved.get(agent).filter(|id| store.contains(agent, id)).map(str::to_string);
            self.accounts.set(agent, id);
        }
        self.window_account_label = window_label(&self.accounts);
    }

    pub(super) fn window_account_label(&self) -> Option<String> {
        self.window_account_label.clone()
    }

    pub(super) fn toggle_account_menu(&mut self, cx: &mut Context<Self>) {
        // The click that closed the menu from outside must not reopen it.
        if self.just_dismissed(MENU_KEY) {
            return;
        }
        self.account_menu_open = !self.account_menu_open;
        if self.account_menu_open {
            self.refresh_account_rows(cx);
        }
        cx.notify();
    }

    pub(super) fn refresh_account_rows(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_spawn(async { read_rows(crate::ui::now_ms()) });
        cx.spawn(async move |this, cx| {
            let rows = task.await;
            let _ = this.update(cx, |this, cx| {
                this.account_rows = rows;
                cx.notify();
            });
        })
        .detach();
    }

    /// New sessions of this window use `id` for `agent`. What already runs stays on its account,
    /// and the session list shows the new account's sessions.
    pub(super) fn use_account_here(&mut self, agent: AuthAgent, id: Option<String>, cx: &mut Context<Self>) {
        self.accounts.set(agent, id);
        self.window_account_label = window_label(&self.accounts);
        self.sync_follow_accounts(cx);
        // Made again for these accounts the next time it opens.
        self.usage = None;
        self.persist(cx);
        self.refresh_sessions(cx);
        self.refresh_account_usage(cx);
        cx.notify();
    }

    /// Opens a pane that signs in to a new account in a folder of its own, and watches that folder.
    pub(super) fn add_account(&mut self, agent: AuthAgent, window: &mut Window, cx: &mut Context<Self>) {
        self.account_menu_open = false;
        let id = match accounts::create_folder(agent).and_then(|id| accounts::prepare(agent, &id).map(|_| id)) {
            Ok(id) => id,
            Err(err) => {
                self.show_toast(format!("{err:#}"), cx);
                return;
            }
        };
        let line = match (agent, cfg!(windows)) {
            // The pane closes itself once the login worked, and stays to show why when it didn't.
            (AuthAgent::Claude, false) => "claude auth login && exit",
            (AuthAgent::Codex, false) => "codex login && exit",
            (AuthAgent::Claude, true) => "claude auth login; if ($?) { exit }",
            (AuthAgent::Codex, true) => "codex login; if ($?) { exit }",
        };
        let cwd = crate::launch::home_dir();
        let mut spec = LaunchSpec::shell_command(line.to_string(), t(cx, "account.signing_in").to_string(), cwd);
        spec.accounts.set(agent, Some(id.clone()));
        self.open_tab(spec, window, cx);
        self.show_toast(t(cx, "account.login_hint"), cx);
        self.watch_login(agent, id, cx);
    }

    /// Signs in again to an account whose login ended or is about to: the default login in the
    /// agents' own folders (which replaces it), another in its own folder. The rows are read again
    /// once the pane closes.
    pub(super) fn relogin(&mut self, agent: AuthAgent, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.account_menu_open = false;
        // The default login lives in the agents' own folders, whatever account this window is on.
        let line = match (agent, id.is_some(), cfg!(windows)) {
            (AuthAgent::Claude, true, false) => "claude auth login && exit",
            (AuthAgent::Codex, true, false) => "codex login && exit",
            (AuthAgent::Claude, false, false) => "env -u CLAUDE_CONFIG_DIR claude auth login && exit",
            (AuthAgent::Codex, false, false) => "env -u CODEX_HOME codex login && exit",
            (AuthAgent::Claude, true, true) => "claude auth login; if ($?) { exit }",
            (AuthAgent::Codex, true, true) => "codex login; if ($?) { exit }",
            (AuthAgent::Claude, false, true) => {
                "Remove-Item Env:CLAUDE_CONFIG_DIR -ErrorAction Ignore; claude auth login; if ($?) { exit }"
            }
            (AuthAgent::Codex, false, true) => "Remove-Item Env:CODEX_HOME -ErrorAction Ignore; codex login; if ($?) { exit }",
        };
        let mut spec = LaunchSpec::shell_command(line.to_string(), t(cx, "account.signing_in").to_string(), crate::launch::home_dir());
        spec.accounts.set(agent, id);
        self.open_tab(spec, window, cx);
        self.show_toast(t(cx, "account.login_hint"), cx);
        let pane = self.active_pane().map(|pane| pane.downgrade());
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            loop {
                cx.background_executor().timer(LOGIN_POLL).await;
                let gone = pane.as_ref().is_none_or(|pane| pane.upgrade().is_none());
                if gone || started.elapsed() > LOGIN_TIMEOUT {
                    let _ = this.update(cx, |this, cx| this.refresh_account_rows(cx));
                    return;
                }
            }
        })
        .detach();
    }

    /// Waits for the login in account `id`'s folder. A login that never comes (the pane closed, or
    /// nothing for a long time) takes its empty folder with it.
    fn watch_login(&mut self, agent: AuthAgent, id: String, cx: &mut Context<Self>) {
        let pane = self.active_pane().map(|pane| pane.downgrade());
        cx.spawn(async move |this, cx| {
            let Ok(folder) = accounts::home(agent, &id) else { return };
            let started = Instant::now();
            loop {
                cx.background_executor().timer(LOGIN_POLL).await;
                let check = folder.clone();
                if let Some(login) = cx.background_spawn(async move { login_in(agent, &check) }).await {
                    let _ = this.update(cx, |this, cx| this.finish_login(agent, id, login, cx));
                    return;
                }
                let gone = pane.as_ref().is_none_or(|pane| pane.upgrade().is_none());
                if gone || started.elapsed() > LOGIN_TIMEOUT {
                    let _ = cx.background_spawn(async move { accounts::delete_folder(agent, &id) }).await;
                    return;
                }
            }
        })
        .detach();
    }

    /// A login arrived. The same login twice would split one account's sessions over two folders,
    /// so the new folder goes and the known account comes forward.
    fn finish_login(&mut self, agent: AuthAgent, id: String, mut login: Account, cx: &mut Context<Self>) {
        login.id = id.clone();
        let task = cx.background_spawn(async move { keep_login(agent, id, login) });
        cx.spawn(async move |this, cx| {
            let result: anyhow::Result<(Option<String>, bool)> = task.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok((account, added)) => {
                    if !added {
                        this.show_toast(t(cx, "account.already_added"), cx);
                    }
                    this.refresh_account_rows(cx);
                    crate::open_account_window(agent, account, cx);
                }
                Err(err) => this.show_toast(format!("{err:#}"), cx),
            });
        })
        .detach();
    }

    /// Whether a pane of this window runs on account `id`.
    fn runs_account(&self, agent: AuthAgent, id: &str, cx: &gpui::App) -> bool {
        self.workspaces
            .iter()
            .flat_map(|ws| ws.tabs.iter())
            .any(|tab| tab.root.leaves().into_iter().any(|pane| pane.read(cx).spec.accounts.get(agent) == Some(id)))
    }

    /// Asks before removing an account: its sessions are deleted with it.
    pub(super) fn ask_remove_account(&mut self, agent: AuthAgent, id: String, name: String, cx: &mut Context<Self>) {
        self.ask(
            Ask {
                title: tf(cx, "account.remove_title", &[("name", &name)]).into(),
                body: Some(t(cx, "account.remove_body").into()),
                choices: vec![AskChoice {
                    label: t(cx, "account.remove").into(),
                    action: AskAction::RemoveAccount(agent, id),
                    primary: false,
                    danger: true,
                }],
                cancel: true,
            },
            cx,
        );
    }

    /// Removes an account, once this window's update is over: every window gets read, this one too.
    pub(super) fn remove_account(&mut self, agent: AuthAgent, id: String, cx: &mut Context<Self>) {
        cx.defer(move |cx| remove_everywhere(agent, id, cx));
    }

    fn render_account_row(&self, row: &AccountRow, cx: &mut Context<Self>) -> AnyElement {
        let current = self.accounts.get(row.agent) == row.id.as_deref();
        let key = format!("{:?}-{}", row.agent, row.id.as_deref().unwrap_or("own"));
        let (agent, id) = (row.agent, row.id.clone());
        let name = name_of(&row.account).map(str::to_string).unwrap_or_else(|| t(cx, "account.own_login").to_string());
        let mut lines = div().flex_1().min_w_0().flex().flex_col().gap_0p5().child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().min_w_0().truncate().t_small().text_color(hex(Chrome::BRIGHT)).child(name))
                .children(row.account.plan_name().map(|plan| div().flex_shrink_0().t_caption().text_color(hex(Chrome::MUTED)).child(plan))),
        );
        let now = crate::ui::now_ms();
        if let Some(warning) = row.login_warning(now, cx) {
            lines = lines.child(div().truncate().t_caption().text_color(hex(Chrome::WARNING)).child(warning));
        } else if let Some(used) = usage_line(row, cx) {
            lines = lines.child(div().truncate().t_caption().text_color(hex(Chrome::MUTED)).child(used));
        }
        let open_id = id.clone();
        let open = div()
            .id(SharedString::from(format!("account-open-{key}")))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1p5()
            .rounded_md()
            .cursor_pointer()
            .hover(crate::ui::hover_fill)
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(7.))
                    .rounded_full()
                    .when(current, |d| d.bg(hex(super::remote_page::NEON_GREEN)))
                    .when(!current, |d| d.border_1().border_color(hex(Chrome::MUTED))),
            )
            .child(lines)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.account_menu_open = false;
                cx.notify();
                crate::open_account_window(agent, open_id.clone(), cx);
            }));
        let use_here = (!current).then(|| {
            div()
                .id(SharedString::from(format!("account-here-{key}")))
                .flex_shrink_0()
                .p_1()
                .rounded_md()
                .cursor_pointer()
                .hover(crate::ui::hover_fill)
                .child(icon("check", IconSize::INLINE, hex(Chrome::MUTED)))
                .tooltip(crate::ui::Tooltip::text(t(cx, "account.use_here"), None))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.account_menu_open = false;
                    this.use_account_here(agent, id.clone(), cx);
                }))
        });
        let relogin = row.needs_login(now).then(|| {
            let id = row.id.clone();
            div()
                .id(SharedString::from(format!("account-relogin-{key}")))
                .flex_shrink_0()
                .p_1()
                .rounded_md()
                .cursor_pointer()
                .hover(crate::ui::hover_fill)
                .child(icon("rotate-cw", IconSize::INLINE, hex(Chrome::WARNING)))
                .tooltip(crate::ui::Tooltip::text(t(cx, "account.relogin"), None))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.relogin(agent, id.clone(), window, cx)))
        });
        div().flex().items_center().gap_0p5().child(open).children(relogin).children(use_here).into_any_element()
    }

    /// The menu beside the rail, open or not.
    pub(super) fn render_account_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.account_menu_open {
            return None;
        }
        let mut panel = popover().w(px(MENU_WIDTH)).on_mouse_down_out(cx.listener(|this, _, _, cx| {
            this.account_menu_open = false;
            this.note_dismissed(MENU_KEY);
            cx.notify();
        }));
        for agent in [AuthAgent::Claude, AuthAgent::Codex] {
            panel = panel.child(
                div().px_2().pt_1p5().pb_0p5().t_caption().text_color(hex(Chrome::MUTED)).child(agent_of(agent).display_name().to_string()),
            );
            for row in self.account_rows.iter().filter(|row| row.agent == agent) {
                panel = panel.child(self.render_account_row(row, cx));
            }
            let (id, label) = match agent {
                AuthAgent::Claude => ("account-add-claude", "account.add_claude"),
                AuthAgent::Codex => ("account-add-codex", "account.add_codex"),
            };
            panel = panel.child(
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .t_small()
                    .hover(crate::ui::hover_fill)
                    .child(icon("plus", IconSize::INLINE, hex(Chrome::MUTED)))
                    .child(t(cx, label))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.add_account(agent, window, cx))),
            );
        }
        panel = panel.child(div().my_1().h(px(1.)).bg(hex(Chrome::OVERLAY_BORDER))).child(
            div()
                .id("account-manage")
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .t_small()
                .hover(crate::ui::hover_fill)
                .child(t(cx, "account.manage"))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.account_menu_open = false;
                    this.open_settings_section(super::settings_page::SettingsSection::Accounts, cx);
                })),
        );
        Some(
            div()
                .absolute()
                .left(px(super::chrome::ACTIVITY_BAR_WIDTH + 4.))
                .bottom(px(0.))
                .child(gpui::deferred(crate::ui::fade_in("account-menu-fade", panel)).with_priority(3))
                .into_any_element(),
        )
    }
}

/// Removes an account, unless something still runs on it. Windows that used it go back to the
/// agent's own login.
fn remove_everywhere(agent: AuthAgent, id: String, cx: &mut gpui::App) {
    let windows = crate::workbenches(cx);
    if windows.iter().any(|handle| handle.read(cx).is_ok_and(|wb| wb.runs_account(agent, &id, cx))) {
        crate::with_active_workbench(cx, |wb, _, cx| wb.show_toast(t(cx, "account.in_use"), cx));
        return;
    }
    for handle in &windows {
        let _ = handle.update(cx, |wb, _, cx| {
            if wb.accounts.get(agent) == Some(id.as_str()) {
                wb.use_account_here(agent, None, cx);
            }
        });
    }
    let task = cx.background_spawn(async move {
        let mut store = AccountStore::load();
        store.forget(agent, &id);
        store.save()?;
        agentty_bridge::sync::SyncProfile::of_account(agent, &id).delete()?;
        discard_folder(agent, &id)
    });
    cx.spawn(async move |cx| {
        let result = task.await;
        let _ = cx.update(|cx| {
            crate::with_active_workbench(cx, |wb, _, cx| {
                if let Err(err) = result {
                    wb.show_toast(format!("{err:#}"), cx);
                }
                wb.refresh_account_rows(cx);
                wb.refresh_account_usage(cx);
            });
        });
    })
    .detach();
}

/// Lays out window `slot` to open on `id` for `agent`, before it opens: it lists that account's
/// sessions from its first paint.
pub(crate) fn prepare_window(slot: usize, agent: AuthAgent, id: Option<String>) -> anyhow::Result<()> {
    let mut state = super::LayoutState::default();
    state.accounts.set(agent, id);
    state.save(slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_is_named_by_email_then_organization() {
        let account = |email: Option<&str>, org: Option<&str>| Account {
            email: email.map(str::to_string),
            org: org.map(str::to_string),
            ..Default::default()
        };
        assert_eq!(name_of(&account(Some("me@example.com"), Some("Org"))), Some("me@example.com"));
        assert_eq!(name_of(&account(None, Some("Org"))), Some("Org"));
        assert_eq!(name_of(&account(None, None)), None);
    }

    #[test]
    fn a_plan_reads_as_a_name() {
        let plan = |plan: &str| Account { plan: Some(plan.into()), ..Account::default() }.plan_name();
        assert_eq!(plan("max").as_deref(), Some("Max"));
        assert_eq!(plan(""), None);
    }

    #[test]
    fn a_window_on_the_default_logins_has_no_label() {
        assert_eq!(window_label(&accounts::AccountChoice::default()), None);
    }
}
