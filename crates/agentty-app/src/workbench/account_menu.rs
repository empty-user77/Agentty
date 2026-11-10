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

/// A Claude Code command for the account in `folder`, and only that one: a key in the app's own
/// environment would answer "logged in" for an empty folder, and the Keychain variable would point
/// it at another account's entry.
fn claude_in(command: &mut Command, folder: &Path) {
    command.env("CLAUDE_CONFIG_DIR", folder).env_remove(agent_auth::CLAUDE_KEYCHAIN_VARIABLE);
    for name in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN"] {
        command.env_remove(name);
    }
}

/// What the CLI's own status says about the login in `folder` (the user's own without one):
/// `Some(None)` when signed out, `None` when it couldn't tell (no CLI yet, no answer). Runs a
/// process: off the UI thread.
fn claude_status(folder: Option<&Path>) -> Option<Option<Account>> {
    let mut command = Command::new(crate::agent_bins::program("claude"));
    command.args(["auth", "status", "--json"]).stdin(Stdio::null()).stderr(Stdio::null());
    if let Some(folder) = folder {
        claude_in(&mut command, folder);
    }
    agentty_bridge::process::hide_window(&mut command);
    let output = command.output().ok()?;
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    Some(accounts::parse_claude_status(&status))
}

fn claude_login(folder: Option<&Path>) -> Option<Account> {
    claude_status(folder).flatten()
}

/// The login in a Codex folder, as [`claude_status`]. A login Codex keeps outside `auth.json` (its
/// keyring store) is still a login, only one Agentty can't name.
fn codex_status(folder: &Path) -> Option<Option<Account>> {
    if let Some(account) = accounts::read_codex_login(folder) {
        return Some(Some(account));
    }
    let mut command = Command::new(crate::agent_bins::program("codex"));
    command.args(["login", "status"]).env("CODEX_HOME", folder).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    agentty_bridge::process::hide_window(&mut command);
    Some(command.status().ok()?.success().then(Account::default))
}

fn codex_login(folder: &Path) -> Option<Account> {
    codex_status(folder).flatten()
}

fn login_state(agent: AuthAgent, folder: &Path) -> Option<Option<Account>> {
    match agent {
        AuthAgent::Claude => claude_status(Some(folder)),
        AuthAgent::Codex => codex_status(folder),
    }
}

fn login_in(agent: AuthAgent, folder: &Path) -> Option<Account> {
    login_state(agent, folder).flatten()
}

/// Signs the account in `folder` out: its Keychain entry (or keyring login) isn't in the folder.
/// Runs a process: off the UI thread.
fn sign_out(agent: AuthAgent, folder: &Path) {
    let mut command = match agent {
        AuthAgent::Claude => {
            let mut command = Command::new(crate::agent_bins::program("claude"));
            command.args(["auth", "logout"]);
            claude_in(&mut command, folder);
            command
        }
        AuthAgent::Codex => {
            let mut command = Command::new(crate::agent_bins::program("codex"));
            command.arg("logout").env("CODEX_HOME", folder);
            command
        }
    };
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    agentty_bridge::process::hide_window(&mut command);
    let _ = command.status();
}

/// The line a login pane runs: the CLI's login in `folder`, or in the agents' own folders without
/// one. The folder is named on the line itself, after the shell profile has run, so a profile
/// exporting `CLAUDE_CONFIG_DIR` or `CODEX_HOME` can't send the login into the user's own folder.
/// The pane closes itself once the login worked, and stays to show why when it didn't.
fn login_line(agent: AuthAgent, folder: Option<&Path>) -> String {
    let keychain = agent_auth::CLAUDE_KEYCHAIN_VARIABLE;
    if cfg!(windows) {
        let quote = |folder: &Path| crate::launch::powershell_quote(&folder.display().to_string());
        let set = match (agent, folder) {
            (AuthAgent::Claude, Some(folder)) => {
                format!("$env:CLAUDE_CONFIG_DIR = {}; Remove-Item Env:{keychain} -ErrorAction Ignore; ", quote(folder))
            }
            (AuthAgent::Codex, Some(folder)) => format!("$env:CODEX_HOME = {}; ", quote(folder)),
            (AuthAgent::Claude, None) => "Remove-Item Env:CLAUDE_CONFIG_DIR -ErrorAction Ignore; ".into(),
            (AuthAgent::Codex, None) => "Remove-Item Env:CODEX_HOME -ErrorAction Ignore; ".into(),
        };
        let login = if agent == AuthAgent::Claude { "claude auth login" } else { "codex login" };
        return format!("{set}{login}; if ($?) {{ exit }}");
    }
    let quote = |folder: &Path| crate::launch::shell_quote(&folder.display().to_string());
    let line = match (agent, folder) {
        (AuthAgent::Claude, Some(folder)) => format!("env -u {keychain} CLAUDE_CONFIG_DIR={} claude auth login", quote(folder)),
        (AuthAgent::Codex, Some(folder)) => format!("env CODEX_HOME={} codex login", quote(folder)),
        (AuthAgent::Claude, None) => "env -u CLAUDE_CONFIG_DIR claude auth login".into(),
        (AuthAgent::Codex, None) => "env -u CODEX_HOME codex login".into(),
    };
    format!("{line} && exit")
}

fn own_login(agent: AuthAgent) -> Option<Account> {
    match agent {
        AuthAgent::Claude => claude_login(None),
        AuthAgent::Codex => codex_login(&agent_auth::user_codex_home()),
    }
}

/// Signs out and deletes an account's folder. Signing out comes first: the CLI's Keychain entry
/// isn't in the folder. Only a folder the CLI says is signed out skips it. Runs a process: off the
/// UI thread.
fn discard_folder(agent: AuthAgent, id: &str) -> anyhow::Result<()> {
    let folder = accounts::home(agent, id)?;
    if login_state(agent, &folder) != Some(None) {
        sign_out(agent, &folder);
    }
    accounts::delete_folder(agent, id)
}

/// Lists a login that arrived in folder `id`, unless the same account is known already: then the
/// new folder goes (unless it holds sessions) and the known one is returned. `(account, newly
/// listed)`. Off the UI thread.
fn keep_login(agent: AuthAgent, id: String, login: Account) -> anyhow::Result<(Option<String>, bool)> {
    let folder = accounts::home(agent, &id)?;
    let drop_new = || {
        if !accounts::holds_sessions(agent, &folder) {
            let _ = discard_folder(agent, &id);
        }
    };
    if own_login(agent).is_some_and(|own| own.same_login(&login)) {
        drop_new();
        return Ok((None, false));
    }
    match AccountStore::change(|store| store.add(agent, login))? {
        Added::Duplicate(known) => {
            drop_new();
            Ok((Some(known), false))
        }
        Added::Listed => Ok((Some(id), true)),
    }
}

/// Folders left by a login the app quit during. One that signed in after all is listed; one the
/// CLI says is signed out goes, unless it holds sessions (a damaged list must not cost them).
/// Folders made since the app started belong to a login under way and are left alone, and nothing
/// is decided while the list can't be read. Runs processes: off the UI thread, once at start.
pub(crate) fn tidy_account_folders() {
    let started = std::time::SystemTime::now();
    for agent in [AuthAgent::Claude, AuthAgent::Codex] {
        let Ok(store) = AccountStore::read() else { return };
        for folder in accounts::homes(agent) {
            let Some(id) = folder.file_name().and_then(|name| name.to_str()).map(str::to_string) else { continue };
            if store.contains(agent, &id) || accounts::home(agent, &id).is_err() {
                continue;
            }
            let modified = std::fs::metadata(&folder).and_then(|meta| meta.modified());
            if modified.map_or(true, |at| at + Duration::from_secs(60) > started) {
                continue;
            }
            match login_state(agent, &folder) {
                Some(Some(mut login)) => {
                    login.id = id.clone();
                    let _ = keep_login(agent, id, login);
                }
                Some(None) if !accounts::holds_sessions(agent, &folder) => {
                    let _ = accounts::delete_folder(agent, &id);
                }
                _ => {}
            }
        }
    }
}

/// After signing in again to account `id`: a login as somebody else is undone, or that person's
/// login would run this account's sessions under the wrong name. Returns the account's name then.
/// Runs processes: off the UI thread.
fn check_relogin(agent: AuthAgent, id: &str) -> Option<String> {
    let folder = accounts::home(agent, id).ok()?;
    let live = login_in(agent, &folder)?;
    let store = AccountStore::load();
    let known = store.of(agent).iter().find(|account| account.id == id)?;
    let other = known.email.is_some() && live.email.is_some() && !known.same_login(&live);
    if !other {
        return None;
    }
    sign_out(agent, &folder);
    Some(name_of(known).unwrap_or_default().to_string())
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

/// "5h 12% · weekly 40%", or "-" before the status line recorded any limits.
pub(super) fn usage_line(row: &AccountRow, cx: &gpui::App) -> String {
    let windows = row.limits.iter().flat_map(|limits| [("tray.session", limits.session), ("tray.weekly", limits.weekly)]);
    let used: Vec<String> =
        windows.filter_map(|(label, window)| window.map(|w| format!("{} {:.0}%", t(cx, label), w.used_percent))).collect();
    if used.is_empty() {
        "-".into()
    } else {
        used.join(" · ")
    }
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
        // A login picked for this pane stays, the agent's own login included.
        let fixed = spec.fixed_account.map(|agent| (agent, spec.accounts.get(agent).map(str::to_string)));
        spec.accounts.fill_from(&self.accounts);
        if let Some((agent, id)) = fixed {
            spec.accounts.set(agent, id);
            return;
        }
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

    /// What a login is called: its email (or organization), else "Default login".
    pub(super) fn login_name(&self, agent: AuthAgent, id: Option<&str>, cx: &gpui::App) -> String {
        let listed = self.account_rows.iter().find(|row| row.agent == agent && row.id.as_deref() == id);
        let name = match (listed, id) {
            (Some(row), _) => name_of(&row.account).map(str::to_string),
            // Before the rows are read: the saved account.
            (None, Some(id)) => AccountStore::load().of(agent).iter().find(|a| a.id == id).and_then(name_of).map(str::to_string),
            (None, None) => None,
        };
        name.unwrap_or_else(|| t(cx, "account.own_login").to_string())
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
        let made = accounts::create_folder(agent).and_then(|id| accounts::prepare(agent, &id).map(|folder| (id, folder)));
        let (id, folder) = match made {
            Ok(made) => made,
            Err(err) => {
                self.show_toast(format!("{err:#}"), cx);
                return;
            }
        };
        let cwd = crate::launch::home_dir();
        let mut spec = LaunchSpec::shell_command(login_line(agent, Some(&folder)), t(cx, "account.signing_in").to_string(), cwd);
        spec.accounts.set(agent, Some(id.clone()));
        self.open_tab(spec, window, cx);
        self.show_toast(t(cx, "account.login_hint"), cx);
        self.watch_login(agent, id, cx);
    }

    /// Signs in again to an account whose login ended or is about to: the default login in the
    /// agents' own folders (which replaces it), another in its own folder. Once the pane closes, a
    /// login as somebody else is undone and the rows are read again.
    pub(super) fn relogin(&mut self, agent: AuthAgent, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.account_menu_open = false;
        // The default login lives in the agents' own folders, whatever account this window is on.
        let folder = match id.as_deref().map(|id| accounts::prepare(agent, id)).transpose() {
            Ok(folder) => folder,
            Err(err) => {
                self.show_toast(format!("{err:#}"), cx);
                return;
            }
        };
        let line = login_line(agent, folder.as_deref());
        let mut spec = LaunchSpec::shell_command(line, t(cx, "account.signing_in").to_string(), crate::launch::home_dir());
        spec.accounts.set(agent, id.clone());
        self.open_tab(spec, window, cx);
        self.show_toast(t(cx, "account.login_hint"), cx);
        let pane = self.active_pane().map(|pane| pane.downgrade());
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            loop {
                cx.background_executor().timer(LOGIN_POLL).await;
                let gone = pane.as_ref().is_none_or(|pane| pane.upgrade().is_none());
                if gone || started.elapsed() > LOGIN_TIMEOUT {
                    let other = match id {
                        Some(id) => cx.background_spawn(async move { check_relogin(agent, &id) }).await,
                        None => None,
                    };
                    let _ = this.update(cx, |this, cx| {
                        if let Some(name) = other {
                            this.show_toast(tf(cx, "account.relogin_other", &[("name", &name)]), cx);
                        }
                        this.refresh_account_rows(cx);
                    });
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

    /// Whether a pane of this window runs on account `id`: an agent pane of that agent, or a shell,
    /// where the agent may be typed.
    fn runs_account(&self, agent: AuthAgent, id: &str, cx: &gpui::App) -> bool {
        self.workspaces.iter().flat_map(|ws| ws.tabs.iter()).any(|tab| {
            tab.root.leaves().into_iter().any(|pane| {
                let spec = &pane.read(cx).spec;
                let uses = match spec.kind {
                    crate::launch::PaneKind::Shell => true,
                    crate::launch::PaneKind::Claude => agent == AuthAgent::Claude,
                    crate::launch::PaneKind::Codex => agent == AuthAgent::Codex,
                };
                uses && spec.accounts.get(agent) == Some(id)
            })
        })
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
        // The account this window runs on has the green dot; every row keeps the dot's room, so
        // names and the lines under them start at the same place.
        const DOT: f32 = 7.;
        let dot = div().flex_shrink_0().size(px(DOT)).rounded_full().when(current, |d| d.bg(hex(super::remote_page::NEON_GREEN)));
        let now = crate::ui::now_ms();
        let (detail, color) = match row.login_warning(now, cx) {
            Some(warning) => (warning.to_string(), Chrome::WARNING),
            None => (usage_line(row, cx), Chrome::MUTED),
        };
        let lines = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(dot)
                    .child(div().flex_1().min_w_0().truncate().t_small().text_color(hex(Chrome::BRIGHT)).child(name))
                    .children(
                        row.account.plan_name().map(|plan| div().flex_shrink_0().t_caption().text_color(hex(Chrome::MUTED)).child(plan)),
                    ),
            )
            .child(div().pl(px(DOT + 8.)).truncate().t_caption().text_color(hex(color)).child(detail));
        let open = div()
            .id(SharedString::from(format!("account-open-{key}")))
            .flex_1()
            .min_w_0()
            .px_2()
            .py_1p5()
            .rounded_md()
            .cursor_pointer()
            .hover(crate::ui::hover_fill)
            .child(lines)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.account_menu_open = false;
                cx.notify();
                crate::open_account_window(agent, id.clone(), cx);
            }));
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
        div().flex().items_center().gap_0p5().child(open).children(relogin).into_any_element()
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
    // The list goes last: an account whose folder couldn't go stays listed, to be removed again,
    // instead of coming back unlisted with its login at the next start.
    let task = cx.background_spawn(async move {
        agentty_bridge::sync::SyncProfile::of_account(agent, &id).delete()?;
        discard_folder(agent, &id)?;
        if agent == AuthAgent::Claude {
            agentty_bridge::limits::forget_claude(&id);
        }
        AccountStore::change(|store| store.forget(agent, &id))
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

    #[cfg(unix)]
    #[test]
    fn a_login_line_names_its_folder_after_the_profile_ran() {
        let folder = Path::new("/tmp/example data/accounts/claude/a1");
        let claude = login_line(AuthAgent::Claude, Some(folder));
        assert_eq!(
            claude,
            "env -u CLAUDE_SECURESTORAGE_CONFIG_DIR CLAUDE_CONFIG_DIR='/tmp/example data/accounts/claude/a1' claude auth login && exit"
        );
        assert_eq!(login_line(AuthAgent::Codex, Some(folder)), "env CODEX_HOME='/tmp/example data/accounts/claude/a1' codex login && exit");
        // The agents' own login leaves out whatever account the window is on.
        assert_eq!(login_line(AuthAgent::Claude, None), "env -u CLAUDE_CONFIG_DIR claude auth login && exit");
        assert_eq!(login_line(AuthAgent::Codex, None), "env -u CODEX_HOME codex login && exit");
    }

    #[test]
    fn a_window_on_the_default_logins_has_no_label() {
        assert_eq!(window_label(&accounts::AccountChoice::default()), None);
    }
}
