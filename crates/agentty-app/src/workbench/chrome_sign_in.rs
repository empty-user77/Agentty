//! Signing in to a page with what Chrome already holds.
//!
//! When a page in the in-app browser asks to sign in, a bar over it offers Chrome's sign-in for
//! that site. Once the user said "always", it is looked up by itself. When Chrome holds nothing
//! that works, the page is covered, blurred, until the sign-in shows up in Chrome (picked up by
//! itself) or the user chooses to sign in right there.
//!
//! Only the cookies a browser would send to the page's own host are read, into the store of that
//! tab. Plugins' pages never take part. No command lets an agent answer the bar or ask for a site:
//! it sees where things stand (`status`). With "always" a sign-in page is enough, whoever opened it.

use super::browser::BrowserTab;
use super::Workbench;
use crate::i18n::{t, tf};
use crate::settings::{settings, update_settings, ChromeSignIn};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, TypeScale};
use crate::webview::Cookie;
use agentty_bridge::chrome_cookies::{self, ChromeCookie, ProfileCookies, ReadError};
use gpui::{div, prelude::*, AnyElement, ClickEvent, Context};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(500);
/// "Loaded from Chrome" stays this long once the page moved on.
const NOTICE_FOR: Duration = Duration::from_secs(6);
/// After loading a site's sign-in, a sign-in page there means the user signed out or the sign-in
/// was refused: it is asked about, not loaded back by itself.
const LOADED_RECENTLY: Duration = Duration::from_secs(30 * 60);
/// Chrome writes its cookies to disk about every 30 seconds.
const WAIT_EVERY: Duration = Duration::from_secs(6);
/// The cover stays until the user does something, so after a while Chrome is looked at less often.
const WAIT_BRISKLY_FOR: Duration = Duration::from_secs(5 * 60);
const WAIT_SLOWLY_EVERY: Duration = Duration::from_secs(30);
const WAIT_RELOADS: u32 = 10;
/// Cookies that keep changing in Chrome (analytics) must not keep reloading the page here.
const WAIT_RELOAD_EVERY: Duration = Duration::from_secs(30);
const WAIT_RELOAD_RARELY_EVERY: Duration = Duration::from_secs(5 * 60);
/// Width of the picture of the page behind the cover: stretched over the page it reads as a blur,
/// and nothing on the page can be made out in it.
const COVER_WIDTH: f64 = 56.;
/// A page that never answered was unloaded or replaced: it is asked again after this.
const ANSWER_WITHIN: Duration = Duration::from_secs(5);
/// Cookies not reported in by then are taken as in, so a tab can't wait on them for good.
const APPLY_WITHIN: Duration = Duration::from_secs(10);
/// Runs apart from the page's own scripts, which share only the DOM with it.
const PROBE_WORLD: &str = "agentty-sign-in";
const PROBE: &str = "const shown = (field) => {
  if (!field.getClientRects().length) return false;
  const style = getComputedStyle(field);
  return style.visibility !== 'hidden' && style.display !== 'none';
};
return [...document.querySelectorAll('input[type=password]')]
  .some((field) => shown(field) && field.autocomplete !== 'new-password') ? '1' : '0';";

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) enum Stage {
    #[default]
    None,
    /// The bar asks whether to use Chrome's sign-in.
    Ask { host: String },
    /// Chrome's file is being read (macOS may be asking the user).
    Reading { host: String },
    /// Cookies are in place and the page reloaded. `confirmed`: the page is no longer a sign-in.
    Loaded { host: String, profile: String, confirmed_at: Option<Instant> },
    /// Chrome holds nothing that signs in here.
    NotFound { host: String },
    /// Chrome could not be read; `refused`: the user denied macOS's question.
    Failed { host: String, refused: bool },
    /// The user went to sign in in Chrome: looked at again until it shows up there. `loaded`: the
    /// profile whose cookies went in last.
    Waiting { host: String, since: Instant, next: Instant, reloads: u32, loaded: Option<String> },
}

impl Stage {
    fn host(&self) -> Option<&str> {
        match self {
            Stage::None => None,
            Stage::Ask { host }
            | Stage::Reading { host }
            | Stage::Loaded { host, .. }
            | Stage::NotFound { host }
            | Stage::Failed { host, .. }
            | Stage::Waiting { host, .. } => Some(host),
        }
    }
}

/// A tab, for as long as it lasts: its native view is replaced when its page is unloaded.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct TabId(u64);

impl Default for TabId {
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// A page's answer: the address asked about, the load it was asked in, and whether it shows a
/// password field (`None`: it could not say).
type PageAnswer = (String, u32, Option<bool>);

/// What a tab knows about signing in.
#[derive(Default)]
pub(super) struct TabSignIn {
    id: TabId,
    pub(super) stage: Stage,
    /// The address last looked at, and whether a load was seen going on.
    checked_url: String,
    was_loading: bool,
    /// When to look at the page: soon after it loads, then again for forms that come late.
    checks: Vec<Instant>,
    answer: Rc<RefCell<Option<PageAnswer>>>,
    asked_at: Option<Instant>,
    /// The cookies went in and the page reloaded (from a WebKit callback).
    applied: Rc<Cell<bool>>,
    /// Since when cookies are going in: the page in front is still the one from before them, and
    /// what it says is not an answer about them.
    applying: Option<Instant>,
    /// Counts the reloads after cookies went in; an answer from before one is out of date.
    generation: u32,
    /// A read of Chrome's file is under way (it waits while macOS asks the user).
    read_pending: bool,
    /// Other profiles holding this site, tried when the first one doesn't sign in.
    other_profiles: Vec<ProfileCookies>,
    /// The blurred picture of the page behind the cover, once it is taken.
    cover: Rc<RefCell<Option<CoverPicture>>>,
    /// When a change in Chrome last reloaded the page.
    wait_reloaded_at: Option<Instant>,
    /// What each profile held for the site while waiting for a sign-in in Chrome (`None`: not
    /// looked yet, so what is there first is what the sign-in has to change).
    seen: Option<HashMap<String, u64>>,
}

impl TabSignIn {
    fn look_soon(&mut self, now: Instant) {
        self.checks = vec![now + Duration::from_millis(300), now + Duration::from_millis(2300)];
    }

    fn is_applying(&self, now: Instant) -> bool {
        self.applying.is_some_and(|since| now.duration_since(since) < APPLY_WITHIN)
    }

    /// Cookies are about to go in: nothing asked until now counts any more.
    fn start_applying(&mut self, now: Instant) {
        self.applying = Some(now);
        self.checks.clear();
    }

    /// The reload that follows the cookies is on its way: it gets looked at.
    fn note_applied(&mut self, now: Instant) {
        if self.applied.replace(false) {
            self.applying = None;
            self.generation = self.generation.wrapping_add(1);
            self.look_soon(now + Duration::from_millis(900));
        }
    }

    /// The page's answer about `url`, unless it is about something else or out of date.
    fn take_answer(&mut self, url: &str, now: Instant) -> Option<bool> {
        let (asked, generation, has_password) = self.answer.borrow_mut().take()?;
        self.asked_at = None;
        (asked == url && generation == self.generation && !self.is_applying(now)).then_some(has_password).flatten()
    }

    /// Cookies went in and nothing more will be learned about them: no claim either way.
    fn gave_up_looking(&self, now: Instant) -> bool {
        matches!(self.stage, Stage::Loaded { confirmed_at: None, .. })
            && self.checks.is_empty()
            && self.asked_at.is_none()
            && !self.is_applying(now)
            && !self.was_loading
    }

    fn may_reload_for_a_change(&self, reloads: u32, now: Instant) -> bool {
        let every = if reloads < WAIT_RELOADS { WAIT_RELOAD_EVERY } else { WAIT_RELOAD_RARELY_EVERY };
        self.wait_reloaded_at.is_none_or(|at| now.duration_since(at) >= every)
    }

    /// Whether the page is covered, and waiting for a sign-in in Chrome.
    pub(super) fn covers_page(&self) -> bool {
        matches!(self.stage, Stage::NotFound { .. } | Stage::Waiting { .. })
    }

    /// Chrome holds nothing that works here: from now on it is watched for a sign-in.
    fn start_waiting(&mut self, host: String, now: Instant) {
        self.seen = None;
        self.wait_reloaded_at = None;
        self.stage = Stage::Waiting { host, since: now, next: now, reloads: 0, loaded: None };
    }

    /// Whether to ask the page now, taking the look off the list.
    fn due_for_a_look(&mut self, now: Instant) -> bool {
        let waiting = self.asked_at.is_some_and(|at| now.duration_since(at) < ANSWER_WITHIN);
        if waiting || self.is_applying(now) || self.checks.first().is_none_or(|at| *at > now) {
            return false;
        }
        self.checks.remove(0);
        true
    }

    /// Where it stands, for the agent driving this tab. Says nothing about accounts or profiles.
    pub(super) fn for_agent(&self) -> Option<&'static str> {
        match self.stage {
            Stage::None => None,
            Stage::Ask { .. } => Some("waiting-for-user"),
            Stage::Reading { .. } => Some("loading-from-chrome"),
            Stage::Loaded { confirmed_at: None, .. } => Some("loading-from-chrome"),
            Stage::Loaded { .. } => Some("loaded-from-chrome"),
            Stage::Waiting { .. } | Stage::NotFound { .. } | Stage::Failed { .. } => Some("user-must-sign-in"),
        }
    }

    pub(super) fn debug_stage(&self) -> String {
        match &self.stage {
            Stage::None => "none".into(),
            Stage::Ask { host } => format!("ask {host}"),
            Stage::Reading { host } => format!("reading {host}"),
            Stage::Loaded { host, confirmed_at, .. } => format!("loaded {host} confirmed={}", confirmed_at.is_some()),
            Stage::NotFound { host } => format!("not-found {host}"),
            Stage::Failed { host, refused } => format!("failed {host} refused={refused}"),
            Stage::Waiting { host, reloads, .. } => format!("waiting {host} reloads={reloads}"),
        }
    }
}

/// What the workbench remembers about sites for this run.
#[derive(Default)]
pub(super) struct SignInMemory {
    /// The user said no, or closed the bar: not brought up again.
    declined: HashSet<String>,
    /// When a site's cookies last went in from Chrome, or reading them failed. Chrome having none
    /// doesn't count: there is nothing to hold back, and it may have some by the next look.
    tried_at: HashMap<String, Instant>,
    /// The user denied macOS's question: asked in the bar from then on, not by macOS again.
    refused: bool,
}

/// What looking at a page calls for.
#[derive(Debug, PartialEq)]
enum Next {
    Nothing,
    Read(String),
    TryOtherProfile,
}

/// The next stage after looking at a page on `host`.
fn decide(stage: &Stage, host: &str, sign_in: bool, always: bool, declined: bool, loaded_recently: bool, now: Instant) -> (Stage, Next) {
    if !sign_in {
        return match stage {
            Stage::Loaded { host, profile, confirmed_at } => (
                Stage::Loaded { host: host.clone(), profile: profile.clone(), confirmed_at: Some(confirmed_at.unwrap_or(now)) },
                Next::Nothing,
            ),
            // What was just picked up from Chrome signed in.
            Stage::Waiting { host, loaded: Some(profile), .. } => {
                (Stage::Loaded { host: host.clone(), profile: profile.clone(), confirmed_at: Some(now) }, Next::Nothing)
            }
            _ => (Stage::None, Next::Nothing),
        };
    }
    if stage.host() == Some(host) {
        return match stage {
            // The page is still asking after the cookies went in.
            Stage::Loaded { .. } => (Stage::NotFound { host: host.into() }, Next::TryOtherProfile),
            same => (same.clone(), Next::Nothing),
        };
    }
    if declined {
        return (Stage::None, Next::Nothing);
    }
    if always && !loaded_recently {
        return (Stage::Reading { host: host.into() }, Next::Read(host.into()));
    }
    (Stage::Ask { host: host.into() }, Next::Nothing)
}

fn to_cookie(cookie: ChromeCookie) -> Cookie {
    Cookie {
        name: cookie.name,
        value: cookie.value,
        domain: cookie.domain,
        path: cookie.path,
        expires: cookie.expires,
        secure: cookie.secure,
        http_only: cookie.http_only,
        same_site: cookie.same_site,
    }
}

/// Whether macOS's answer stands as "no" after a read: a read that got through ends it, and one that
/// failed for another reason leaves it as it was.
fn still_refused<T>(refused: bool, result: &Result<T, ReadError>) -> bool {
    match result {
        Ok(_) => false,
        Err(ReadError::Refused) => true,
        Err(ReadError::Unreadable) => refused,
    }
}

/// What a look at Chrome's file calls for while waiting for a sign-in there.
#[derive(Debug, PartialEq)]
enum WaitLook {
    KeepWaiting,
    Load(ProfileCookies),
    /// No more looks: the next would ask macOS's question again, every few seconds.
    Stop {
        refused: bool,
    },
}

fn wait_look(seen: &mut Option<HashMap<String, u64>>, result: Result<Vec<ProfileCookies>, ReadError>, may_reload: bool) -> WaitLook {
    let found = match result {
        Ok(found) => found,
        Err(error) => return WaitLook::Stop { refused: error == ReadError::Refused },
    };
    let held: HashMap<String, u64> = found.iter().map(|p| (p.profile.dir.clone(), digest(&p.cookies))).collect();
    let changed = seen.as_ref().and_then(|seen| found.into_iter().find(|p| seen.get(&p.profile.dir) != held.get(&p.profile.dir)));
    match changed {
        // Left as seen: it is still news at the next look.
        Some(_) if !may_reload => WaitLook::KeepWaiting,
        Some(found) => {
            *seen = Some(held);
            WaitLook::Load(found)
        }
        None => {
            *seen = Some(held);
            WaitLook::KeepWaiting
        }
    }
}

/// Short digest of what a profile holds, only compared with the next one. Kept in memory.
fn digest(cookies: &[ChromeCookie]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut parts: Vec<(&str, &str, &str)> = cookies.iter().map(|c| (c.domain.as_str(), c.name.as_str(), c.value.as_str())).collect();
    parts.sort();
    parts.hash(&mut hasher);
    hasher.finish()
}

/// Puts `found`'s cookies into the tab's store, then reloads its page.
fn apply(tab: &mut BrowserTab, found: ProfileCookies) {
    tab.sign_in.start_applying(Instant::now());
    let webview = tab.webview.clone();
    let applied = tab.sign_in.applied.clone();
    let cookies: Vec<Cookie> = found.cookies.into_iter().map(to_cookie).collect();
    crate::webview::set_cookies_of(tab.profile, &cookies, move || {
        if let Ok(borrowed) = webview.try_borrow() {
            if let Some(view) = borrowed.as_ref() {
                view.reload();
            }
        }
        applied.set(true);
    });
}

/// Loads `found` into the tab, the rest kept to try next.
fn load_into(tab: &mut BrowserTab, host: &str, mut found: Vec<ProfileCookies>, memory: &mut SignInMemory) {
    if found.is_empty() {
        tab.sign_in.stage = Stage::NotFound { host: host.into() };
        return;
    }
    memory.tried_at.insert(host.to_string(), Instant::now());
    let first = found.remove(0);
    tab.sign_in.other_profiles = found;
    tab.sign_in.stage = Stage::Loaded { host: host.into(), profile: first.profile.name.clone(), confirmed_at: None };
    apply(tab, first);
}

impl Workbench {
    /// Follows the pages of every terminal's browser, on screen or not: an agent may be working in
    /// one nobody is looking at.
    pub(super) fn watch_chrome_sign_in(&mut self, cx: &mut Context<Self>) {
        if !crate::platform::HAS_WEBVIEW {
            return;
        }
        // Pictures left by the last run. The first window only: another's may be in use.
        if self.slot == 0 {
            let _ = std::fs::remove_dir_all(covers_dir());
        }
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            if this.update(cx, |this, cx| this.chrome_tick(cx)).is_err() {
                break;
            }
        })
        .detach();
    }

    fn chrome_tick(&mut self, cx: &mut Context<Self>) {
        let prefs = settings(cx).browser.clone();
        let off = prefs.private_mode || prefs.chrome_sign_in == ChromeSignIn::Off || !chrome_available();
        let always = prefs.chrome_sign_in == ChromeSignIn::Always;
        let now = Instant::now();
        let mut reads: Vec<(TabId, String)> = Vec::new();
        let mut changed = false;
        let Workbench { browser, stashed_browser, pane_browsers, chrome_sign_ins: memory, .. } = self;
        let tabs = browser.iter_mut().chain(stashed_browser.iter_mut()).chain(pane_browsers.values_mut()).flat_map(|b| b.tabs.iter_mut());
        for tab in tabs.filter(|tab| tab.owner.is_none()) {
            if off {
                if tab.sign_in.stage != Stage::None {
                    changed = true;
                    tab.sign_in = TabSignIn::default();
                }
                continue;
            }
            changed |= tick_tab(tab, always, memory, now, &mut reads);
        }
        for (tab, host) in reads {
            self.read_chrome(tab, host, cx);
        }
        if changed {
            cx.notify();
        }
    }

    /// Reads Chrome's file for `host` off the main thread (macOS may be asking the user).
    fn read_chrome(&mut self, tab: TabId, host: String, cx: &mut Context<Self>) {
        let only = settings(cx).browser.chrome_profile.clone();
        let asked = host.clone();
        let read = cx.background_executor().spawn(async move { chrome_cookies::read(&asked, only.as_deref()) });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |this, cx| this.finish_chrome_read(tab, &host, result, cx));
        })
        .detach();
    }

    fn finish_chrome_read(&mut self, id: TabId, host: &str, result: Result<Vec<ProfileCookies>, ReadError>, cx: &mut Context<Self>) {
        let Workbench { browser, stashed_browser, pane_browsers, chrome_sign_ins: memory, .. } = self;
        let tabs = browser.iter_mut().chain(stashed_browser.iter_mut()).chain(pane_browsers.values_mut()).flat_map(|b| b.tabs.iter_mut());
        memory.refused = still_refused(memory.refused, &result);
        let Some(tab) = tabs.into_iter().find(|tab| tab.sign_in.id == id) else { return };
        tab.sign_in.read_pending = false;
        // The page moved on meanwhile: its site's cookies are not wanted any more.
        if tab.sign_in.stage.host() != Some(host) {
            return;
        }
        match (tab.sign_in.stage.clone(), result) {
            (Stage::Reading { .. }, Ok(found)) => load_into(tab, host, found, memory),
            (Stage::Reading { .. }, Err(error)) => {
                memory.tried_at.insert(host.to_string(), Instant::now());
                tab.sign_in.stage = Stage::Failed { host: host.into(), refused: error == ReadError::Refused };
            }
            (Stage::Waiting { since, next, reloads, .. }, result) => {
                let now = Instant::now();
                let may_reload = tab.sign_in.may_reload_for_a_change(reloads, now);
                match wait_look(&mut tab.sign_in.seen, result, may_reload) {
                    WaitLook::KeepWaiting => {}
                    WaitLook::Stop { refused } => tab.sign_in.stage = Stage::Failed { host: host.into(), refused },
                    WaitLook::Load(found) => {
                        let loaded = Some(found.profile.name.clone());
                        tab.sign_in.stage = Stage::Waiting { host: host.into(), since, next, reloads: reloads + 1, loaded };
                        tab.sign_in.wait_reloaded_at = Some(now);
                        memory.tried_at.insert(host.to_string(), now);
                        apply(tab, found);
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn front_sign_in_tab(&mut self) -> Option<&mut BrowserTab> {
        self.browser.as_mut().map(|browser| browser.tab_mut()).filter(|tab| tab.owner.is_none())
    }

    /// "Always" or "just this once" in the bar, and "load again".
    fn chrome_load(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.front_sign_in_tab() else { return };
        let Some(host) = tab.sign_in.stage.host().map(str::to_string) else { return };
        tab.sign_in.stage = Stage::Reading { host: host.clone() };
        tab.sign_in.read_pending = true;
        let id = tab.sign_in.id;
        self.read_chrome(id, host, cx);
        cx.notify();
    }

    /// "Sign in here": the page comes back, and the user's clicks reach it even if an agent is at
    /// work in it. The site is then left alone like one declined.
    fn chrome_sign_in_here(&mut self, cx: &mut Context<Self>) {
        if let Some(browser) = self.browser.as_mut() {
            browser.unlocked = true;
        }
        self.chrome_decline(cx);
    }

    /// "No" and "close": this site is left alone until Agentty restarts.
    fn chrome_decline(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.front_sign_in_tab() else { return };
        let host = tab.sign_in.stage.host().map(str::to_string);
        tab.sign_in.stage = Stage::None;
        self.chrome_sign_ins.declined.extend(host);
        cx.notify();
    }

    /// Opens the page in Chrome and keeps looking until the sign-in shows up there.
    fn chrome_open(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.front_sign_in_tab() else { return };
        let Some(host) = tab.sign_in.stage.host().map(str::to_string) else { return };
        let Some(url) = tab.webview.borrow().as_ref().and_then(|view| view.current_url()) else { return };
        chrome_cookies::open_in_chrome(&url);
        // Looked at briskly again. What was seen stays what a sign-in has to change.
        let now = Instant::now();
        if let Stage::Waiting { since, next, .. } = &mut tab.sign_in.stage {
            *since = now;
            *next = now;
        } else {
            tab.sign_in.start_waiting(host, now);
        }
        cx.notify();
    }

    /// Over the page while Chrome holds no sign-in for it: the page blurred, what to do, and the
    /// way to sign in right here instead.
    pub(super) fn render_chrome_cover(&self, tab: &BrowserTab, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !tab.sign_in.covers_page() {
            return None;
        }
        let picture = tab.sign_in.cover.borrow().as_ref().map(|picture| picture.0.clone());
        Some(
            div()
                .id("chrome-cover")
                .absolute()
                .inset_0()
                .overflow_hidden()
                .bg(hex(Chrome::EDITOR))
                .when_some(picture, |d, picture| d.child(gpui::img(picture).absolute().size_full().object_fit(gpui::ObjectFit::Cover)))
                .child(div().absolute().inset_0().bg(hex_alpha(Chrome::EDITOR, 0.72)))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_3()
                        .px_6()
                        .child(div().t_body().text_center().text_color(hex(Chrome::BRIGHT)).child(t(cx, "browser.chrome_cover")))
                        .child(crate::ui::action_button(
                            "chrome-sign-in-here",
                            t(cx, "browser.chrome_direct"),
                            cx.listener(|this, _: &ClickEvent, _, cx| this.chrome_sign_in_here(cx)),
                        ))
                        .child(
                            div()
                                .id("chrome-open")
                                .px_2()
                                .py_0p5()
                                .rounded_sm()
                                .cursor_pointer()
                                .t_small()
                                .text_color(hex(Chrome::FOREGROUND))
                                .hover(|d| d.text_color(hex(Chrome::BRIGHT)))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.chrome_open(cx)))
                                .child(t(cx, "browser.chrome_open")),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The bar over the page, while there is something to say or ask.
    pub(super) fn render_chrome_sign_in(&self, tab: &BrowserTab, cx: &mut Context<Self>) -> Option<AnyElement> {
        let always = settings(cx).browser.chrome_sign_in == ChromeSignIn::Always;
        let stage = &tab.sign_in.stage;
        let host = stage.host()?;
        let button =
            |id: &'static str, label: &'static str, primary: bool, cx: &mut Context<Self>, on: fn(&mut Self, &mut Context<Self>)| {
                div()
                    .id(id)
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .cursor_pointer()
                    .t_small()
                    .flex_shrink_0()
                    .text_color(hex(if primary { Chrome::ACCENT } else { Chrome::FOREGROUND }))
                    .hover(|d| d.bg(hex_alpha(Chrome::FOREGROUND, 0.1)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| on(this, cx)))
                    .child(t(cx, label))
                    .into_any_element()
            };
        let (text, tone, buttons): (String, u32, Vec<AnyElement>) = match stage {
            // The cover says it (see `render_chrome_cover`).
            Stage::None | Stage::NotFound { .. } | Stage::Waiting { .. } => return None,
            Stage::Ask { .. } => {
                let mut buttons = Vec::new();
                if always {
                    buttons.push(button("chrome-load", "browser.chrome_load", true, cx, Self::chrome_load));
                } else {
                    buttons.push(button("chrome-always", "browser.chrome_always", true, cx, |this, cx| {
                        update_settings(cx, |s| s.browser.chrome_sign_in = ChromeSignIn::Always);
                        this.chrome_load(cx);
                    }));
                    buttons.push(button("chrome-once", "browser.chrome_once", false, cx, Self::chrome_load));
                }
                buttons.push(button("chrome-no", "browser.chrome_no", false, cx, Self::chrome_decline));
                (tf(cx, "browser.chrome_ask", &[("host", host)]), Chrome::ACCENT, buttons)
            }
            Stage::Reading { .. } | Stage::Loaded { confirmed_at: None, .. } => {
                (t(cx, "browser.chrome_reading").to_string(), Chrome::ACCENT, Vec::new())
            }
            Stage::Loaded { profile, .. } => (
                tf(cx, "browser.chrome_done", &[("host", host), ("profile", profile)]),
                Chrome::SUCCESS,
                vec![button("chrome-close", "browser.chrome_close", false, cx, |this, cx| {
                    if let Some(tab) = this.front_sign_in_tab() {
                        tab.sign_in.stage = Stage::None;
                    }
                    cx.notify();
                })],
            ),
            Stage::Failed { refused, .. } => (
                t(cx, if *refused { "browser.chrome_refused" } else { "browser.chrome_unreadable" }).to_string(),
                Chrome::WARNING,
                vec![
                    button("chrome-again", "browser.chrome_retry", true, cx, Self::chrome_load),
                    button("chrome-close", "browser.chrome_close", false, cx, Self::chrome_decline),
                ],
            ),
        };
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x_2()
                .gap_y_1()
                .px_3()
                .py_2()
                // Solid and dark: the page area under it is white, and a tint would wash out.
                .bg(hex(Chrome::EDITOR))
                .border_b_2()
                .border_color(hex(tone))
                .child(icon("key-round", 14., hex(tone)))
                .child(div().flex_1().min_w(gpui::px(180.)).t_small().text_color(hex(Chrome::BRIGHT)).child(text))
                .children(buttons)
                .into_any_element(),
        )
    }
}

fn covers_dir() -> std::path::PathBuf {
    agentty_bridge::fsutil::data_dir().join("browser-covers")
}

/// A picture of a page on disk, for as long as something holds it: its tab may go at any moment, with
/// the workspace it is in, or while the picture is still being taken.
struct CoverPicture(std::path::PathBuf);

impl Drop for CoverPicture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A very small picture of the page, in the background: the cover shows it stretched.
fn take_cover_picture(view: &crate::webview::WebView, cover: &Rc<RefCell<Option<CoverPicture>>>) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = covers_dir();
    if agentty_bridge::fsutil::create_private_dir(&dir).is_err() {
        return;
    }
    // A new name each time: pictures are cached by their path.
    let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir.join(format!("{}-{unique}.png", std::process::id()));
    let cover = cover.clone();
    let taken = path.clone();
    view.snapshot_png_sized(
        path,
        COVER_WIDTH,
        Box::new(move |result| {
            // Without a picture the cover is plain, which says the same.
            if result.is_ok() {
                *cover.borrow_mut() = Some(CoverPicture(taken));
            }
        }),
    );
}

/// Looks after one tab; whether anything on screen changed.
fn tick_tab(tab: &mut BrowserTab, always: bool, memory: &mut SignInMemory, now: Instant, reads: &mut Vec<(TabId, String)>) -> bool {
    let webview = tab.webview.clone();
    let borrowed = webview.borrow();
    let Some(view) = borrowed.as_ref() else { return false };
    let before = tab.sign_in.stage.clone();
    let sign_in = &mut tab.sign_in;
    sign_in.note_applied(now);
    if view.is_loading() {
        sign_in.was_loading = true;
        return false;
    }
    let Some(url) = view.current_url() else { return false };
    if sign_in.was_loading || url != sign_in.checked_url {
        sign_in.was_loading = false;
        sign_in.checked_url = url.clone();
        sign_in.look_soon(now);
    }
    // The page could not be opened: it says nothing about signing in.
    if view.load_error().is_some() {
        sign_in.answer.borrow_mut().take();
        sign_in.asked_at = None;
        sign_in.checks.clear();
        if matches!(sign_in.stage, Stage::Loaded { confirmed_at: None, .. }) {
            sign_in.stage = Stage::None;
        }
        return before != sign_in.stage;
    }
    if let Some(has_password) = sign_in.take_answer(&url, now) {
        let host = chrome_cookies::page_host(&url);
        let is_sign_in = host.is_some() && chrome_cookies::is_sign_in_page(&url, has_password);
        let host = host.unwrap_or_default();
        let declined = memory.declined.contains(&host);
        let hold_back = memory.refused || memory.tried_at.get(&host).is_some_and(|at| at.elapsed() < LOADED_RECENTLY);
        let (stage, next) = decide(&sign_in.stage, &host, is_sign_in, always, declined, hold_back, now);
        sign_in.stage = stage;
        match next {
            Next::Nothing => {}
            Next::Read(host) => {
                sign_in.read_pending = true;
                reads.push((sign_in.id, host));
            }
            Next::TryOtherProfile => {
                if !sign_in.other_profiles.is_empty() {
                    let others = std::mem::take(&mut sign_in.other_profiles);
                    drop(borrowed);
                    load_into(tab, &host, others, memory);
                    return true;
                }
            }
        }
    } else if sign_in.due_for_a_look(now) {
        if chrome_cookies::page_host(&url).is_some() {
            sign_in.asked_at = Some(now);
            let inbox = sign_in.answer.clone();
            let asked = url.clone();
            let generation = sign_in.generation;
            view.call_in_world(
                PROBE,
                &[],
                Some(PROBE_WORLD),
                Box::new(move |result| {
                    *inbox.borrow_mut() = Some((asked, generation, result.ok().map(|text| text == "1")));
                }),
            );
        } else {
            sign_in.stage = Stage::None;
        }
    }
    if sign_in.gave_up_looking(now) {
        sign_in.stage = Stage::None;
    }
    match sign_in.stage.clone() {
        Stage::Loaded { confirmed_at: Some(at), .. } if now.duration_since(at) >= NOTICE_FOR => sign_in.stage = Stage::None,
        Stage::NotFound { host } => {
            sign_in.start_waiting(host, now);
            take_cover_picture(view, &sign_in.cover);
        }
        Stage::Waiting { host, since, next, reloads, loaded } if now >= next => {
            let every = if now.duration_since(since) >= WAIT_BRISKLY_FOR { WAIT_SLOWLY_EVERY } else { WAIT_EVERY };
            sign_in.stage = Stage::Waiting { host: host.clone(), since, next: now + every, reloads, loaded };
            // One at a time: a read waiting on macOS's question must not pile up others.
            if !sign_in.read_pending {
                sign_in.read_pending = true;
                reads.push((sign_in.id, host));
            }
        }
        _ => {}
    }
    if !sign_in.covers_page() {
        sign_in.cover.borrow_mut().take();
    }
    // Other profiles' cookies are only held while the first one's are being tried.
    if !matches!(sign_in.stage, Stage::Loaded { confirmed_at: None, .. }) {
        sign_in.other_profiles.clear();
    }
    // The next waiting look is not news on screen.
    !matches!((&before, &sign_in.stage), (Stage::Waiting { .. }, Stage::Waiting { .. })) && before != sign_in.stage
}

/// Whether Chrome is on this computer, looked up once a minute at most.
pub(super) fn chrome_available() -> bool {
    thread_local! { static KNOWN: Cell<Option<(Instant, bool)>> = const { Cell::new(None) }; }
    KNOWN.with(|known| match known.get() {
        Some((at, value)) if at.elapsed() < Duration::from_secs(60) => value,
        _ => {
            let value = chrome_cookies::available();
            known.set(Some((Instant::now(), value)));
            value
        }
    })
}

/// Chrome's profiles for the settings page, looked up once a minute at most.
pub(super) fn chrome_profiles() -> Vec<chrome_cookies::ChromeProfile> {
    thread_local! { static KNOWN: RefCell<Option<(Instant, Vec<chrome_cookies::ChromeProfile>)>> = const { RefCell::new(None) }; }
    KNOWN.with(|known| {
        let mut known = known.borrow_mut();
        if let Some((at, list)) = known.as_ref() {
            if at.elapsed() < Duration::from_secs(60) {
                return list.clone();
            }
        }
        let list = chrome_cookies::profiles();
        *known = Some((Instant::now(), list.clone()));
        list
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(stage: &Stage, host: &str, sign_in: bool, always: bool) -> (Stage, Next) {
        decide(stage, host, sign_in, always, false, false, Instant::now())
    }

    #[test]
    fn a_sign_in_page_is_asked_about_until_the_user_said_always() {
        let (stage, next) = run(&Stage::None, "a.test", true, false);
        assert_eq!(stage, Stage::Ask { host: "a.test".into() });
        assert_eq!(next, Next::Nothing);
        let (stage, next) = run(&Stage::None, "a.test", true, true);
        assert_eq!(stage, Stage::Reading { host: "a.test".into() });
        assert_eq!(next, Next::Read("a.test".into()));
    }

    #[test]
    fn a_page_that_is_no_sign_in_clears_the_bar() {
        for stage in [
            Stage::Ask { host: "a.test".into() },
            Stage::Reading { host: "a.test".into() },
            Stage::NotFound { host: "a.test".into() },
            Stage::Failed { host: "a.test".into(), refused: true },
        ] {
            assert_eq!(run(&stage, "a.test", false, true).0, Stage::None);
        }
    }

    #[test]
    fn leaving_the_sign_in_page_after_loading_confirms_it_and_staying_does_not() {
        let loaded = Stage::Loaded { host: "a.test".into(), profile: "Work".into(), confirmed_at: None };
        let (stage, _) = run(&loaded, "a.test", false, true);
        assert!(matches!(stage, Stage::Loaded { confirmed_at: Some(_), .. }));
        let (stage, next) = run(&loaded, "a.test", true, true);
        assert_eq!(stage, Stage::NotFound { host: "a.test".into() });
        assert_eq!(next, Next::TryOtherProfile);
    }

    #[test]
    fn being_sent_on_to_another_sites_sign_in_starts_over_for_that_site() {
        let loaded = Stage::Loaded { host: "app.test".into(), profile: "Work".into(), confirmed_at: None };
        let (stage, next) = run(&loaded, "idp.test", true, true);
        assert_eq!(stage, Stage::Reading { host: "idp.test".into() });
        assert_eq!(next, Next::Read("idp.test".into()));
    }

    #[test]
    fn what_was_picked_up_while_waiting_is_told_and_leaving_without_it_is_not() {
        let now = Instant::now();
        let waiting = |loaded: Option<&str>| Stage::Waiting {
            host: "a.test".into(),
            since: now,
            next: now,
            reloads: 1,
            loaded: loaded.map(str::to_string),
        };
        assert!(matches!(run(&waiting(Some("Work")), "a.test", false, true).0, Stage::Loaded { confirmed_at: Some(_), .. }));
        assert_eq!(run(&waiting(None), "a.test", false, true).0, Stage::None);
        assert_eq!(run(&waiting(Some("Work")), "a.test", true, true).0, waiting(Some("Work")));
    }

    #[test]
    fn what_the_page_said_before_the_cookies_went_in_does_not_count() {
        let now = Instant::now();
        let mut sign_in = TabSignIn::default();
        sign_in.look_soon(now);
        // Asked about the old page, then the cookies start going in.
        *sign_in.answer.borrow_mut() = Some(("http://a.test/login".into(), sign_in.generation, Some(true)));
        sign_in.start_applying(now);
        assert_eq!(sign_in.take_answer("http://a.test/login", now), None, "while they go in");
        assert!(!sign_in.due_for_a_look(now + Duration::from_secs(5)), "the old page is not looked at");
        // An answer still under way from before the reload, arriving after it.
        let old = sign_in.generation;
        sign_in.applied.set(true);
        sign_in.note_applied(now);
        *sign_in.answer.borrow_mut() = Some(("http://a.test/login".into(), old, Some(true)));
        assert_eq!(sign_in.take_answer("http://a.test/login", now), None, "from before the reload");
        // The reloaded page does get looked at, and its answer counts.
        assert!(!sign_in.due_for_a_look(now));
        assert!(sign_in.due_for_a_look(now + Duration::from_secs(2)));
        *sign_in.answer.borrow_mut() = Some(("http://a.test/login".into(), sign_in.generation, Some(true)));
        assert_eq!(sign_in.take_answer("http://a.test/login", now), Some(true));
        // A page that could not say is no answer, least of all "no password field".
        *sign_in.answer.borrow_mut() = Some(("http://a.test/login".into(), sign_in.generation, None));
        assert_eq!(sign_in.take_answer("http://a.test/login", now), None);
    }

    #[test]
    fn a_tab_never_waits_for_good() {
        let now = Instant::now();
        let mut sign_in = TabSignIn::default();
        sign_in.look_soon(now);
        assert!(sign_in.due_for_a_look(now + Duration::from_secs(1)));
        sign_in.asked_at = Some(now + Duration::from_secs(1));
        sign_in.look_soon(now);
        assert!(!sign_in.due_for_a_look(now + Duration::from_secs(2)), "an answer is on its way");
        assert!(sign_in.due_for_a_look(now + Duration::from_secs(7)), "a page that never answers is asked again");
        // Cookies that never report in.
        let mut sign_in = TabSignIn::default();
        sign_in.start_applying(now);
        sign_in.look_soon(now);
        assert!(!sign_in.due_for_a_look(now + Duration::from_secs(1)));
        assert!(sign_in.due_for_a_look(now + Duration::from_secs(11)));
    }

    fn held(dir: &str, value: &str) -> ProfileCookies {
        ProfileCookies {
            profile: chrome_cookies::ChromeProfile { dir: dir.into(), name: dir.into() },
            cookies: vec![ChromeCookie {
                name: "sid".into(),
                value: value.into(),
                domain: ".a.test".into(),
                path: "/".into(),
                expires: None,
                secure: true,
                http_only: true,
                same_site: None,
            }],
        }
    }

    #[test]
    fn waiting_for_chrome_picks_up_a_change_and_nothing_else() {
        let mut seen = None;
        assert_eq!(
            wait_look(&mut seen, Ok(vec![held("Default", "before")]), true),
            WaitLook::KeepWaiting,
            "the first look is the baseline"
        );
        assert_eq!(wait_look(&mut seen, Ok(vec![held("Default", "before")]), true), WaitLook::KeepWaiting);
        // A change while reloads are held back stays news until they are not.
        assert_eq!(wait_look(&mut seen, Ok(vec![held("Default", "after")]), false), WaitLook::KeepWaiting);
        assert_eq!(wait_look(&mut seen, Ok(vec![held("Default", "after")]), true), WaitLook::Load(held("Default", "after")));
        assert_eq!(wait_look(&mut seen, Ok(vec![held("Default", "after")]), true), WaitLook::KeepWaiting, "once");
        // A profile that appears is a change too.
        let both = vec![held("Default", "after"), held("Profile 1", "new")];
        assert_eq!(wait_look(&mut seen, Ok(both), true), WaitLook::Load(held("Profile 1", "new")));
    }

    #[test]
    fn waiting_stops_rather_than_ask_macos_over_and_over() {
        let mut seen = None;
        assert_eq!(wait_look(&mut seen, Err(ReadError::Refused), true), WaitLook::Stop { refused: true });
        assert_eq!(wait_look(&mut seen, Err(ReadError::Unreadable), true), WaitLook::Stop { refused: false });
    }

    #[test]
    fn reloads_for_changes_in_chrome_are_spaced_and_counted() {
        let now = Instant::now();
        let mut sign_in = TabSignIn::default();
        assert!(sign_in.may_reload_for_a_change(0, now));
        sign_in.wait_reloaded_at = Some(now);
        assert!(!sign_in.may_reload_for_a_change(1, now + Duration::from_secs(29)));
        assert!(sign_in.may_reload_for_a_change(1, now + Duration::from_secs(30)));
        // A cover can stay for long, so a sign-in is never missed for good: only more slowly.
        assert!(!sign_in.may_reload_for_a_change(WAIT_RELOADS, now + Duration::from_secs(299)));
        assert!(sign_in.may_reload_for_a_change(WAIT_RELOADS, now + Duration::from_secs(300)));
    }

    #[test]
    fn the_cover_is_up_exactly_while_waiting_for_chrome() {
        let now = Instant::now();
        let mut sign_in = TabSignIn::default();
        for (stage, covered) in [
            (Stage::None, false),
            (Stage::Ask { host: "a.test".into() }, false),
            (Stage::Reading { host: "a.test".into() }, false),
            (Stage::Loaded { host: "a.test".into(), profile: "P".into(), confirmed_at: None }, false),
            (Stage::Failed { host: "a.test".into(), refused: true }, false),
            (Stage::NotFound { host: "a.test".into() }, true),
            (Stage::Waiting { host: "a.test".into(), since: now, next: now, reloads: 0, loaded: None }, true),
        ] {
            sign_in.stage = stage;
            assert_eq!(sign_in.covers_page(), covered, "{:?}", sign_in.stage);
        }
    }

    #[test]
    fn waiting_starts_from_a_clean_look_at_chrome() {
        let now = Instant::now();
        let mut sign_in = TabSignIn { seen: Some(HashMap::new()), wait_reloaded_at: Some(now), ..Default::default() };
        sign_in.start_waiting("a.test".into(), now);
        assert_eq!(sign_in.seen, None);
        assert_eq!(sign_in.wait_reloaded_at, None);
        assert_eq!(sign_in.stage, Stage::Waiting { host: "a.test".into(), since: now, next: now, reloads: 0, loaded: None });
    }

    #[test]
    fn a_covered_page_leaving_sign_in_uncovers_it_and_says_where_from_if_it_came_from_chrome() {
        let now = Instant::now();
        let waiting = Stage::Waiting { host: "a.test".into(), since: now, next: now, reloads: 1, loaded: Some("Work".into()) };
        let (stage, _) = run(&waiting, "a.test", false, true);
        assert!(matches!(stage, Stage::Loaded { confirmed_at: Some(_), .. }));
        let mut sign_in = TabSignIn { stage, ..Default::default() };
        assert!(!sign_in.covers_page());
        sign_in.stage = Stage::Waiting { host: "a.test".into(), since: now, next: now, reloads: 0, loaded: None };
        let (stage, _) = run(&sign_in.stage, "other.test", false, true);
        assert_eq!(stage, Stage::None, "gone to a page that asks nothing");
    }

    #[test]
    fn no_claim_is_made_about_cookies_nobody_could_check() {
        let now = Instant::now();
        let mut sign_in =
            TabSignIn { stage: Stage::Loaded { host: "a.test".into(), profile: "P".into(), confirmed_at: None }, ..Default::default() };
        assert!(sign_in.gave_up_looking(now));
        sign_in.look_soon(now);
        assert!(!sign_in.gave_up_looking(now), "looks are still to come");
        sign_in.checks.clear();
        sign_in.asked_at = Some(now);
        assert!(!sign_in.gave_up_looking(now), "an answer is on its way");
        sign_in.asked_at = None;
        sign_in.was_loading = true;
        assert!(!sign_in.gave_up_looking(now), "the reload is under way");
        sign_in.was_loading = false;
        sign_in.start_applying(now);
        assert!(!sign_in.gave_up_looking(now), "the cookies are going in");
    }

    #[test]
    fn a_tab_stays_itself_and_no_two_are_the_same() {
        assert!(TabSignIn::default().id != TabSignIn::default().id);
    }

    #[test]
    fn a_no_from_macos_stands_until_a_read_gets_through() {
        assert!(still_refused(false, &Err::<(), _>(ReadError::Refused)));
        assert!(still_refused(true, &Err::<(), _>(ReadError::Unreadable)));
        assert!(!still_refused(false, &Err::<(), _>(ReadError::Unreadable)));
        assert!(!still_refused(true, &Ok(())));
        // And while it stands, a sign-in page is asked about, not read by itself.
        let (stage, next) = decide(&Stage::None, "a.test", true, true, false, true, Instant::now());
        assert_eq!(stage, Stage::Ask { host: "a.test".into() });
        assert_eq!(next, Next::Nothing);
    }

    #[test]
    fn an_answer_about_another_address_does_not_count() {
        let now = Instant::now();
        let mut sign_in = TabSignIn::default();
        *sign_in.answer.borrow_mut() = Some(("http://a.test/login".into(), 0, Some(true)));
        assert_eq!(sign_in.take_answer("http://a.test/other", now), None);
        assert!(sign_in.asked_at.is_none());
    }

    /// Every stage, host, page and setting: what may and may not start a read of Chrome's file.
    #[test]
    fn every_combination_keeps_to_the_rules() {
        let now = Instant::now();
        let stages = |host: &str| {
            vec![
                Stage::None,
                Stage::Ask { host: host.into() },
                Stage::Reading { host: host.into() },
                Stage::Loaded { host: host.into(), profile: "P".into(), confirmed_at: None },
                Stage::Loaded { host: host.into(), profile: "P".into(), confirmed_at: Some(now) },
                Stage::NotFound { host: host.into() },
                Stage::Failed { host: host.into(), refused: true },
                Stage::Failed { host: host.into(), refused: false },
                Stage::Waiting { host: host.into(), since: now, next: now, reloads: 0, loaded: None },
                Stage::Waiting { host: host.into(), since: now, next: now, reloads: 1, loaded: Some("P".into()) },
            ]
        };
        let mut seen = 0;
        for stage_host in ["a.test", "b.test"] {
            for stage in stages(stage_host) {
                for sign_in in [false, true] {
                    for always in [false, true] {
                        for declined in [false, true] {
                            for held_back in [false, true] {
                                seen += 1;
                                let same_site = stage.host() == Some("a.test");
                                let (next_stage, next) = decide(&stage, "a.test", sign_in, always, declined, held_back, now);
                                if matches!(next, Next::Read(_)) {
                                    assert!(sign_in && always && !declined && !held_back && !same_site, "{stage:?}");
                                    assert_eq!(next_stage, Stage::Reading { host: "a.test".into() });
                                }
                                if !sign_in {
                                    assert_eq!(next, Next::Nothing, "{stage:?}");
                                    assert!(matches!(next_stage, Stage::None | Stage::Loaded { confirmed_at: Some(_), .. }), "{stage:?}");
                                }
                                if sign_in && declined && !same_site {
                                    assert_eq!(next_stage, Stage::None, "{stage:?}");
                                }
                                // Looking again settles: a page can't keep things moving by itself.
                                let (again, then) = decide(&next_stage, "a.test", sign_in, always, declined, held_back, now);
                                let (third, last) = decide(&again, "a.test", sign_in, always, declined, held_back, now);
                                assert_eq!(third, again, "{stage:?}");
                                assert_eq!(last, Next::Nothing, "{stage:?} then {then:?}");
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(seen, 2 * 10 * 16);
    }

    /// Thousands of random walks through pages, with reads answered as the workbench does: Chrome's
    /// file is read by itself at most once for a site that was loaded, however a page behaves.
    #[test]
    fn a_page_can_not_make_it_read_or_reload_over_and_over() {
        let hosts = ["a.test", "b.test", "c.test"];
        let mut seed: u64 = 0x5eed_1234;
        let mut random = move |below: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) % below
        };
        for _ in 0..2000 {
            let mut stage = Stage::None;
            let mut declined: HashSet<&str> = HashSet::new();
            let mut loaded: HashSet<String> = HashSet::new();
            let mut reads: HashMap<String, u32> = HashMap::new();
            let now = Instant::now();
            for _ in 0..60 {
                let host = hosts[random(3) as usize];
                let sign_in = random(4) != 0;
                let held_back = loaded.contains(host);
                let (next_stage, next) = decide(&stage, host, sign_in, true, declined.contains(host), held_back, now);
                stage = next_stage;
                if let Next::Read(read) = next {
                    *reads.entry(read.clone()).or_default() += 1;
                    assert!(reads[&read] <= 1, "{read} read again after loading");
                    // The file holds something, and it goes in.
                    loaded.insert(read.clone());
                    stage = Stage::Loaded { host: read, profile: "P".into(), confirmed_at: None };
                }
                if random(10) == 0 {
                    if let Some(host) = stage.host().and_then(|h| hosts.iter().find(|known| **known == h)) {
                        declined.insert(host);
                        stage = Stage::None;
                    }
                }
            }
        }
    }

    #[test]
    fn nothing_repeats_on_the_same_page() {
        for stage in [
            Stage::Ask { host: "a.test".into() },
            Stage::Reading { host: "a.test".into() },
            Stage::NotFound { host: "a.test".into() },
            Stage::Failed { host: "a.test".into(), refused: false },
        ] {
            let (next_stage, next) = run(&stage, "a.test", true, true);
            assert_eq!(next_stage, stage);
            assert_eq!(next, Next::Nothing);
        }
    }

    #[test]
    fn a_cover_picture_goes_with_the_last_thing_holding_it() {
        let path = std::env::temp_dir().join(format!("agentty-cover-test-{}.png", std::process::id()));
        std::fs::write(&path, b"fake picture").unwrap();
        let cover: Rc<RefCell<Option<CoverPicture>>> = Rc::default();
        // The tab goes while the picture is still being taken: its callback holds the other handle.
        let in_flight = cover.clone();
        drop(cover);
        *in_flight.borrow_mut() = Some(CoverPicture(path.clone()));
        assert!(path.exists());
        drop(in_flight);
        assert!(!path.exists());
    }

    #[test]
    fn a_site_the_user_declined_stays_quiet_and_signing_out_is_not_undone() {
        let now = Instant::now();
        assert_eq!(decide(&Stage::None, "a.test", true, true, true, false, now).0, Stage::None);
        let (stage, next) = decide(&Stage::None, "a.test", true, true, false, true, now);
        assert_eq!(stage, Stage::Ask { host: "a.test".into() });
        assert_eq!(next, Next::Nothing);
    }

    #[test]
    fn an_agent_hears_where_it_stands_and_nothing_about_the_account() {
        let mut sign_in = TabSignIn::default();
        assert_eq!(sign_in.for_agent(), None);
        sign_in.stage = Stage::Ask { host: "a.test".into() };
        assert_eq!(sign_in.for_agent(), Some("waiting-for-user"));
        sign_in.stage = Stage::Loaded { host: "a.test".into(), profile: "Private name".into(), confirmed_at: Some(Instant::now()) };
        assert_eq!(sign_in.for_agent(), Some("loaded-from-chrome"));
        sign_in.stage = Stage::NotFound { host: "a.test".into() };
        assert_eq!(sign_in.for_agent(), Some("user-must-sign-in"));
    }

    #[test]
    fn a_digest_follows_what_changed_not_the_order() {
        let cookie = |name: &str, value: &str| ChromeCookie {
            name: name.into(),
            value: value.into(),
            domain: ".a.test".into(),
            path: "/".into(),
            expires: None,
            secure: true,
            http_only: true,
            same_site: None,
        };
        assert_eq!(digest(&[cookie("a", "1"), cookie("b", "2")]), digest(&[cookie("b", "2"), cookie("a", "1")]));
        assert_ne!(digest(&[cookie("a", "1")]), digest(&[cookie("a", "2")]));
    }
}
