//! Browser pages a plugin drives (`browser/*`, permission `browser.control`).
//!
//! A plugin opens pages of the sites its manifest names, in the in-app browser the user is signed
//! in to, and runs scripts on them to read or act. A page runs out of sight (parked outside the
//! window, where it keeps working) or as a tab of the browser panel, where the user sees it and
//! can take over — to sign in, to answer a check the site puts up, or just to watch.
//!
//! What bounds it:
//! - Scripts run only on pages of the named sites. The check is made again inside the page, so a
//!   page that navigated elsewhere between the call and the run is refused there.
//! - Scripts run in a content world of their own: the page cannot see or replace them.
//! - A plugin never gets a cookie. It hears whether a site is signed in and until when.
//! - A plugin that a link reached has no browser (see `plugins::link_guarded`).
//! - A page belongs to one run of one plugin: restarted or stopped, its pages close.

use super::browser::BrowserTab;
use super::Workbench;
use crate::i18n::tf;
use crate::plugins::PluginCall;
use crate::settings::settings;
use crate::webview::WebView;
use agentty_bridge::plugins::codes;
use agentty_bridge::plugins::sites::{BrowserContribution, Site};
use gpui::{Context, Window};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Pages one plugin may have open at once.
const MAX_PAGES_PER_PLUGIN: usize = 3;
/// Largest script a plugin may send, and the largest answer it gets back.
const MAX_SCRIPT_BYTES: usize = 256 * 1024;
const MAX_RESULT_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a sign-in waits for the user.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// The content world plugin scripts run in.
const WORLD: &str = "agentty-plugin";

static NEXT_PAGE: AtomicU64 = AtomicU64::new(1);

/// Where a plugin's page runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserMode {
    /// Out of sight, shown when the plugin asks the user for something (signing in).
    Auto,
    Background,
    Visible,
}

impl BrowserMode {
    pub const ALL: [BrowserMode; 3] = [BrowserMode::Auto, BrowserMode::Background, BrowserMode::Visible];

    pub fn id(self) -> &'static str {
        match self {
            BrowserMode::Auto => "auto",
            BrowserMode::Background => "background",
            BrowserMode::Visible => "visible",
        }
    }

    pub fn from_id(id: &str) -> Option<BrowserMode> {
        BrowserMode::ALL.into_iter().find(|mode| mode.id() == id)
    }

    /// What the user picked for `plugin` in its settings (`Auto` lets the plugin decide).
    pub fn chosen(plugin: &str, cx: &gpui::App) -> BrowserMode {
        settings(cx).plugin_browser_modes.get(plugin).and_then(|id| BrowserMode::from_id(id)).unwrap_or(BrowserMode::Auto)
    }
}

pub struct PluginBrowser {
    pub id: u64,
    pub plugin: String,
    pub plugin_name: String,
    /// The plugin run the page was opened for.
    generation: u64,
    pub(super) webview: Rc<RefCell<Option<WebView>>>,
    /// A tab of the browser panel right now.
    pub(super) shown: bool,
    /// Opened by `browser/signIn`: the site the user is asked to sign in to, shown above the page.
    pub(super) sign_in: Option<String>,
}

/// What the plugin named, read fresh on every call: an update may have changed it.
fn sites_of(plugin: &str, cx: &gpui::App) -> BrowserContribution {
    crate::plugins::plugin(cx, plugin).and_then(|p| p.manifest.as_ref()).and_then(|m| m.browser.clone()).unwrap_or_default()
}

/// Whether the user allowed `plugin` the browser on every site it names now.
pub fn granted(plugin: &str, sites: &BrowserContribution, cx: &gpui::App) -> bool {
    let grants = settings(cx).plugin_browser_grants.get(plugin).cloned().unwrap_or_default();
    !sites.sites.is_empty() && sites.sites.iter().all(|site| grants.contains(&site.host))
}

/// Takes back what the user allowed (the plugin page's "Revoke"); its open pages close.
pub fn revoke(plugin: &str, cx: &mut gpui::App) {
    let plugin = plugin.to_string();
    crate::settings::update_settings(cx, move |settings| {
        settings.plugin_browser_grants.remove(&plugin);
    });
}

/// Every domain of a site, for the consent dialog: `x.com (twitter.com)`.
fn site_names(sites: &BrowserContribution) -> String {
    sites
        .sites
        .iter()
        .map(|site| match site.aliases.is_empty() {
            true => site.host.clone(),
            false => format!("{} ({})", site.host, site.aliases.join(", ")),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Wraps a plugin's script: checks the page is one of its sites, runs the script with `args`,
/// stops it after the timeout and hands back its value as JSON.
fn eval_body(script: &str) -> String {
    format!(
        r#"const input = JSON.parse(__agenttyInput);
const host = location.hostname.toLowerCase();
const loopback = host === 'localhost' || host === '127.0.0.1';
const secure = location.protocol === 'https:' || (location.protocol === 'http:' && loopback);
if (!secure || !input.domains.some((d) => host === d || host.endsWith('.' + d))) throw new Error('this page is not one of the plugin\'s sites');
const run = async (args) => {{
{script}
}};
let timer;
const late = new Promise((_, reject) => {{ timer = setTimeout(() => reject(new Error('the script did not finish in time')), input.timeout); }});
try {{ const value = await Promise.race([run(input.args), late]); return JSON.stringify(value === undefined ? null : value); }}
finally {{ clearTimeout(timer); }}"#
    )
}

/// Every domain of every site, for the check inside the page.
fn domains(sites: &BrowserContribution) -> Vec<String> {
    sites.sites.iter().flat_map(|site| site.domains().map(str::to_string).collect::<Vec<_>>()).collect()
}

fn timeout_of(params: &Value) -> Duration {
    params["timeoutMs"].as_u64().map(Duration::from_millis).unwrap_or(DEFAULT_TIMEOUT).clamp(Duration::from_millis(100), MAX_TIMEOUT)
}

/// Whether `site` is signed in (by its `signedInCookie`) and until when, in ms since the epoch.
/// `None` when the site did not say which cookie means signed in.
pub fn site_status(site: &Site, reply: impl FnOnce(Option<(bool, Option<u64>)>) + 'static) {
    let Some(name) = site.signed_in_cookie.clone() else { return reply(None) };
    let site = site.clone();
    crate::webview::cookies(
        move |domain| site.covers_cookie_domain(domain),
        move |cookies| {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.);
            let found = cookies.iter().filter(|c| c.name == name && !c.value.is_empty()).find(|c| c.expires.is_none_or(|at| at > now));
            reply(Some(match found {
                Some(cookie) => (true, cookie.expires.map(|at| (at * 1000.) as u64)),
                None => (false, None),
            }));
        },
    );
}

impl Workbench {
    pub fn holds_plugin_browser(&self, id: u64) -> bool {
        self.plugin_browsers.iter().any(|page| page.id == id)
    }

    fn plugin_page(&self, call: &PluginCall) -> Result<&PluginBrowser, (i64, String)> {
        let id = call.params["tabId"].as_u64().ok_or((codes::INVALID_PARAMS, "tabId is missing".to_string()))?;
        self.plugin_browsers
            .iter()
            .find(|page| page.id == id && page.plugin == call.plugin)
            .ok_or((codes::INVALID_PARAMS, "no such browser tab".to_string()))
    }

    /// `browser/*` from a plugin (its permission and the link guard were checked by the host).
    pub(super) fn plugin_browser_call(&mut self, call: PluginCall, window: &mut Window, cx: &mut Context<Self>) {
        let sites = sites_of(&call.plugin, cx);
        if !granted(&call.plugin, &sites, cx) {
            return self.ask_browser_consent(call, sites, window, cx);
        }
        match call.method.as_str() {
            "browser/sites" => self.plugin_browser_sites(call, cx),
            "browser/open" => {
                let result = self.open_plugin_page(&call, false, window, cx);
                call.reply(result.map(|id| json!({ "tabId": id })), cx);
            }
            "browser/navigate" => {
                let result = self.plugin_page(&call).and_then(|page| {
                    let url = call.params["url"].as_str().unwrap_or_default();
                    if sites_of(&call.plugin, cx).site_for_url(url).is_none() {
                        return Err((codes::INVALID_PARAMS, format!("{url} is not on one of the plugin's sites")));
                    }
                    match page.webview.borrow().as_ref() {
                        Some(view) => {
                            view.load(url);
                            Ok(Value::Null)
                        }
                        None => Err((codes::UNAVAILABLE, "the page is gone".to_string())),
                    }
                });
                call.reply(result, cx);
            }
            "browser/eval" => self.plugin_browser_eval(call, cx),
            "browser/wait" => self.plugin_browser_wait(call, cx),
            "browser/info" => {
                let result = self.plugin_page(&call).map(|page| {
                    let borrowed = page.webview.borrow();
                    let view = borrowed.as_ref();
                    let url = view.and_then(WebView::current_url);
                    let site = url.as_deref().and_then(|url| sites_of(&call.plugin, cx).site_for_url(url).map(|s| s.host.clone()));
                    json!({
                        "tabId": page.id,
                        "url": url,
                        "title": view.and_then(WebView::title),
                        "loading": view.is_some_and(WebView::is_loading),
                        "visible": page.shown,
                        "site": site,
                    })
                });
                call.reply(result, cx);
            }
            "browser/show" => {
                let result = self.plugin_page(&call).map(|page| page.id);
                if let Ok(id) = result {
                    let message = call.params["message"].as_str().map(|m| m.chars().take(200).collect::<String>());
                    self.show_plugin_page(id, message.as_deref(), cx);
                }
                call.reply(result.map(|_| Value::Null), cx);
            }
            "browser/hide" => {
                let result = self.plugin_page(&call).map(|page| page.id);
                if let Ok(id) = result {
                    self.hide_plugin_page(id, cx);
                }
                call.reply(result.map(|_| Value::Null), cx);
            }
            "browser/close" => {
                let result = self.plugin_page(&call).map(|page| page.id);
                if let Ok(id) = result {
                    self.close_plugin_page(id, cx);
                }
                call.reply(result.map(|_| Value::Null), cx);
            }
            "browser/signIn" => self.plugin_sign_in(call, window, cx),
            other => call.reply(Err((codes::METHOD_NOT_FOUND, format!("unknown method {other}"))), cx),
        }
    }

    /// The first browser call of a plugin (or the first after an update named a new site): the
    /// user is shown what it will be able to do, on which sites, and decides. Until then, and
    /// after a "no" for as long as that run of the plugin lasts, its browser calls are refused.
    fn ask_browser_consent(&mut self, call: PluginCall, sites: BrowserContribution, window: &mut Window, cx: &mut Context<Self>) {
        let generation = crate::plugins::generation(&call.plugin, cx);
        if generation.is_some() && self.browser_refused.get(&call.plugin) == generation.as_ref() {
            return call.reply(Err((codes::PERMISSION_DENIED, "the user did not allow this plugin to use the browser".into())), cx);
        }
        // Calls made while the question is open wait for its answer, and then run or are refused.
        if let Some(waiting) = self.browser_asking.get_mut(&call.plugin) {
            return waiting.push(call);
        }
        self.browser_asking.insert(call.plugin.clone(), Vec::new());
        let title = tf(cx, "plugin_browser.consent.title", &[("plugin", &call.plugin_name)]);
        let body = tf(cx, "plugin_browser.consent.body", &[("plugin", &call.plugin_name), ("sites", &site_names(&sites))]);
        let (allow, deny) = (crate::i18n::t(cx, "plugin_browser.consent.allow"), crate::i18n::t(cx, "plugin_browser.consent.deny"));
        window.activate_window();
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &title,
            Some(&body),
            &[gpui::PromptButton::new(allow), gpui::PromptButton::cancel(deny)],
            cx,
        );
        let plugin_of_answer = call.plugin.clone();
        cx.spawn_in(window, async move |this, cx| {
            let allowed = answer.await == Ok(0);
            let _ = this.update_in(cx, |this, window, cx| {
                let waiting = this.browser_asking.remove(&call.plugin).unwrap_or_default();
                let calls = std::iter::once(call).chain(waiting);
                if !allowed {
                    if let Some(generation) = generation {
                        this.browser_refused.insert(plugin_of_answer.clone(), generation);
                    }
                    for call in calls {
                        call.reply(Err((codes::PERMISSION_DENIED, "the user did not allow this plugin to use the browser".into())), cx);
                    }
                    return;
                }
                let hosts = sites.sites.iter().map(|site| site.host.clone()).collect::<Vec<_>>();
                crate::settings::update_settings(cx, move |settings| {
                    settings.plugin_browser_grants.insert(plugin_of_answer, hosts);
                });
                for call in calls {
                    this.plugin_browser_call(call, window, cx);
                }
            });
        })
        .detach();
    }

    /// Opens a page of one of the plugin's sites; `sign_in` opens it where the user can use it.
    fn open_plugin_page(
        &mut self,
        call: &PluginCall,
        sign_in: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<u64, (i64, String)> {
        let prefs = settings(cx).browser.clone();
        if prefs.private_mode {
            return Err((codes::UNAVAILABLE, "the in-app browser is in private mode, which keeps no sign-in".into()));
        }
        let url = call.params["url"].as_str().unwrap_or_default().to_string();
        if sites_of(&call.plugin, cx).site_for_url(&url).is_none() {
            return Err((codes::INVALID_PARAMS, format!("{url} is not on one of the plugin's sites")));
        }
        let Some(generation) = crate::plugins::generation(&call.plugin, cx) else {
            return Err((codes::UNAVAILABLE, "the plugin is not running".into()));
        };
        if self.plugin_browsers.iter().filter(|page| page.plugin == call.plugin).count() >= MAX_PAGES_PER_PLUGIN {
            return Err((codes::UNAVAILABLE, format!("at most {MAX_PAGES_PER_PLUGIN} browser tabs per plugin")));
        }
        let Some(view) = WebView::new_background(window, &prefs) else {
            return Err((codes::UNAVAILABLE, "the in-app browser is not available".into()));
        };
        view.load(&url);
        let id = NEXT_PAGE.fetch_add(1, Ordering::Relaxed);
        self.plugin_browsers.push(PluginBrowser {
            id,
            plugin: call.plugin.clone(),
            plugin_name: call.plugin_name.clone(),
            generation,
            webview: Rc::new(RefCell::new(Some(view))),
            shown: false,
            sign_in: None,
        });
        let requested = call.params["mode"].as_str().and_then(BrowserMode::from_id).unwrap_or(BrowserMode::Auto);
        let visible = match BrowserMode::chosen(&call.plugin, cx) {
            BrowserMode::Auto => requested == BrowserMode::Visible,
            chosen => chosen == BrowserMode::Visible,
        };
        if sign_in || visible {
            self.show_plugin_page(id, None, cx);
        }
        Ok(id)
    }

    fn plugin_browser_sites(&mut self, call: PluginCall, cx: &mut Context<Self>) {
        let sites = sites_of(&call.plugin, cx).sites;
        let mut waiting = Vec::new();
        for site in sites {
            let (sender, receiver) = futures::channel::oneshot::channel();
            site_status(&site, move |status| {
                let _ = sender.send(status);
            });
            waiting.push((site.host, receiver));
        }
        cx.spawn(async move |_, cx| {
            let mut list = Vec::new();
            for (host, receiver) in waiting {
                let (signed_in, expires) = match receiver.await.ok().flatten() {
                    Some((signed_in, expires)) => (Value::Bool(signed_in), json!(expires)),
                    None => (Value::Null, Value::Null),
                };
                list.push(json!({ "host": host, "signedIn": signed_in, "expiresAt": expires }));
            }
            let _ = cx.update(|cx| call.reply(Ok(Value::Array(list)), cx));
        })
        .detach();
    }

    fn plugin_browser_eval(&mut self, call: PluginCall, cx: &mut Context<Self>) {
        let script = call.params["script"].as_str().unwrap_or_default();
        if script.trim().is_empty() || script.len() > MAX_SCRIPT_BYTES {
            return call.reply(Err((codes::INVALID_PARAMS, format!("script must be 1 to {MAX_SCRIPT_BYTES} bytes"))), cx);
        }
        let page = match self.plugin_page(&call) {
            Ok(page) => page,
            Err(error) => return call.reply(Err(error), cx),
        };
        let sites = sites_of(&call.plugin, cx);
        // Checked here for a clear answer, and again inside the page, which is what counts.
        let url = page.webview.borrow().as_ref().and_then(WebView::current_url).unwrap_or_default();
        if sites.site_for_url(&url).is_none() {
            return call.reply(Err((codes::INVALID_PARAMS, "the page is not on one of the plugin's sites".into())), cx);
        }
        let input = json!({
            "domains": domains(&sites),
            "args": call.params.get("args").cloned().unwrap_or(Value::Null),
            "timeout": timeout_of(&call.params).as_millis() as u64,
        })
        .to_string();
        let body = eval_body(script);
        let (sender, receiver) = futures::channel::oneshot::channel();
        {
            let borrowed = page.webview.borrow();
            let Some(view) = borrowed.as_ref() else { return call.reply(Err((codes::UNAVAILABLE, "the page is gone".into())), cx) };
            view.call_in_world(&body, &[("__agenttyInput", &input)], Some(WORLD), Box::new(move |result| drop(sender.send(result))));
        }
        cx.spawn(async move |_, cx| {
            let answer = match receiver.await.unwrap_or_else(|_| Err("the page is gone".into())) {
                Ok(text) if text.len() > MAX_RESULT_BYTES => {
                    Err((codes::INVALID_PARAMS, format!("the result is over {MAX_RESULT_BYTES} bytes")))
                }
                Ok(text) => serde_json::from_str::<Value>(&text)
                    .map(|value| json!({ "value": value }))
                    .map_err(|_| (codes::INTERNAL, "the result could not be read".to_string())),
                Err(error) => Err((codes::INVALID_PARAMS, error.chars().take(2000).collect())),
            };
            let _ = cx.update(|cx| call.reply(answer, cx));
        })
        .detach();
    }

    /// `browser/wait`: answered once the page has finished loading (or the time is up).
    fn plugin_browser_wait(&mut self, call: PluginCall, cx: &mut Context<Self>) {
        let page = match self.plugin_page(&call) {
            Ok(page) => page,
            Err(error) => return call.reply(Err(error), cx),
        };
        let webview = page.webview.clone();
        let deadline = Instant::now() + timeout_of(&call.params);
        cx.spawn(async move |_, cx| {
            // A load that was just asked for may not have started yet: give it a moment.
            cx.background_executor().timer(Duration::from_millis(150)).await;
            loop {
                let state = webview.borrow().as_ref().map(|view| (view.is_loading(), view.current_url(), view.title()));
                let answer = match state {
                    None => Some(Err((codes::UNAVAILABLE, "the page is gone".to_string()))),
                    Some((false, Some(url), title)) => Some(Ok(json!({ "url": url, "title": title }))),
                    Some(_) if Instant::now() >= deadline => {
                        Some(Err((codes::UNAVAILABLE, "the page did not finish loading in time".into())))
                    }
                    Some(_) => None,
                };
                if let Some(answer) = answer {
                    let _ = cx.update(|cx| call.reply(answer, cx));
                    return;
                }
                cx.background_executor().timer(Duration::from_millis(100)).await;
            }
        })
        .detach();
    }

    /// `browser/signIn`: opens the site's sign-in page where the user can use it, and answers once
    /// the site says the user is signed in, the user closed the tab, or ten minutes passed.
    fn plugin_sign_in(&mut self, call: PluginCall, window: &mut Window, cx: &mut Context<Self>) {
        let host = call.params["host"].as_str().unwrap_or_default().to_string();
        let Some(site) = sites_of(&call.plugin, cx).site_named(&host).cloned() else {
            return call.reply(Err((codes::INVALID_PARAMS, format!("{host} is not one of the plugin's sites"))), cx);
        };
        let mut open = call.clone();
        open.params = json!({ "url": site.sign_in_url() });
        let id = match self.open_plugin_page(&open, true, window, cx) {
            Ok(id) => id,
            Err(error) => return call.reply(Err(error), cx),
        };
        if let Some(page) = self.plugin_browsers.iter_mut().find(|page| page.id == id) {
            page.sign_in = Some(site.host.clone());
        }
        // Signing in is the user's to do: ask where they will see it, even with Agentty behind
        // another window.
        let request = tf(cx, "plugin_browser.sign_in", &[("plugin", &call.plugin_name), ("site", &site.host)]);
        self.show_toast(request.clone(), cx);
        if !window.is_window_active() {
            crate::notifications::show(0, &call.plugin_name, &request);
        }
        if site.signed_in_cookie.is_none() {
            // Nothing tells Agentty when the user is done: the plugin looks for itself.
            return call.reply(Ok(json!({ "tabId": id, "signedIn": null })), cx);
        }
        let deadline = Instant::now() + SIGN_IN_TIMEOUT;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let (sender, receiver) = futures::channel::oneshot::channel();
            let checked = site.clone();
            let _ = cx.update(|_| {
                site_status(&checked, move |status| {
                    let _ = sender.send(status);
                })
            });
            let status = receiver.await.ok().flatten();
            let open = this.update(cx, |this, _| this.plugin_browsers.iter().any(|page| page.id == id && page.shown)).unwrap_or(false);
            let answer = match status {
                Some((true, expires)) => Some(json!({ "tabId": id, "signedIn": true, "expiresAt": expires })),
                _ if !open => Some(json!({ "tabId": id, "signedIn": false, "reason": "closed" })),
                _ if Instant::now() >= deadline => Some(json!({ "tabId": id, "signedIn": false, "reason": "timeout" })),
                _ => None,
            };
            if let Some(answer) = answer {
                let _ = this.update(cx, |this, cx| {
                    // The page was opened to sign in: with that done it goes, and the session stays.
                    this.close_plugin_page(id, cx);
                    if answer["signedIn"] == Value::Bool(true) {
                        crate::browser_keeper::remember(cx);
                    }
                    call.reply(Ok(answer), cx);
                });
                return;
            }
        })
        .detach();
    }

    /// The line above a page a plugin opened for signing in: who asks, for which site, and a way
    /// to say no. Signing in itself happens in the page below, typed by the user.
    pub(super) fn render_sign_in_banner(&self, owner: Option<u64>, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        use crate::theme::{hex, hex_alpha, Chrome};
        use crate::ui::{icon, TypeScale};
        use gpui::{div, prelude::*};
        let page = self.plugin_browsers.iter().find(|page| Some(page.id) == owner)?;
        let site = page.sign_in.clone()?;
        let id = page.id;
        let text = tf(cx, "plugin_browser.sign_in_banner", &[("plugin", &page.plugin_name), ("site", &site)]);
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                // Solid and dark: the page area under it is white, and a tint would wash out.
                .bg(hex(Chrome::EDITOR))
                .border_b_2()
                .border_color(hex(Chrome::WARNING))
                .child(icon("key-round", 14., hex(Chrome::WARNING)))
                .child(div().flex_1().min_w_0().t_small().text_color(hex(Chrome::BRIGHT)).child(text))
                .child(
                    div()
                        .id("plugin-sign-in-cancel")
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .cursor_pointer()
                        .t_small()
                        .text_color(hex(Chrome::FOREGROUND))
                        .hover(|d| d.bg(hex_alpha(Chrome::FOREGROUND, 0.1)))
                        .child(crate::i18n::t(cx, "confirm.cancel"))
                        .on_click(cx.listener(move |this, _, _, cx| this.close_plugin_page(id, cx))),
                )
                .into_any_element(),
        )
    }

    /// Opens `site` out of sight for a moment: a visit that keeps its session going.
    pub fn refresh_site(&mut self, site: &Site, window: &mut Window, cx: &mut Context<Self>) {
        let prefs = settings(cx).browser.clone();
        if prefs.private_mode {
            return;
        }
        let Some(view) = WebView::new_background(window, &prefs) else { return };
        view.load(&site.home_url());
        let id = NEXT_PAGE.fetch_add(1, Ordering::Relaxed);
        self.site_refreshes.push((id, view));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(crate::browser_keeper::REFRESH_STAY).await;
            let _ = this.update(cx, |this, cx| {
                this.site_refreshes.retain(|(refresh, _)| *refresh != id);
                // The visit may have renewed the session: keep what it set.
                crate::browser_keeper::remember(cx);
            });
        })
        .detach();
    }

    /// Puts a plugin's page in the browser panel, in front.
    pub(super) fn show_plugin_page(&mut self, id: u64, message: Option<&str>, cx: &mut Context<Self>) {
        let Some(page) = self.plugin_browsers.iter_mut().find(|page| page.id == id) else { return };
        page.shown = true;
        if let Some(message) = message {
            let text = format!("{}: {message}", page.plugin_name);
            self.show_toast(text, cx);
        }
        self.plugin_pages_to_show.push(id);
        cx.notify();
    }

    fn hide_plugin_page(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(browser) = self.browser.as_mut() {
            if let Some(index) = browser.tabs.iter().position(|tab| tab.owner == Some(id)) {
                if browser.tabs.len() == 1 {
                    self.browser = None;
                } else {
                    browser.tabs.remove(index);
                    browser.active = browser.active.min(browser.tabs.len() - 1);
                }
            }
        }
        if let Some(page) = self.plugin_browsers.iter_mut().find(|page| page.id == id) {
            page.shown = false;
            if let Some(view) = page.webview.borrow_mut().as_mut() {
                view.park();
            }
        }
        cx.notify();
    }

    pub(super) fn close_plugin_page(&mut self, id: u64, cx: &mut Context<Self>) {
        self.hide_plugin_page(id, cx);
        self.plugin_browsers.retain(|page| page.id != id);
    }

    /// Called every render: pages whose plugin stopped or restarted close, pages the user took
    /// out of the panel go back out of sight (and their plugin hears it). Returns the tabs to add
    /// for pages asked to be shown.
    pub(super) fn sync_plugin_browsers(&mut self, cx: &mut Context<Self>) -> Option<Vec<BrowserTab>> {
        let stale: Vec<u64> = self
            .plugin_browsers
            .iter()
            .filter(|page| {
                crate::plugins::generation(&page.plugin, cx) != Some(page.generation)
                    || !granted(&page.plugin, &sites_of(&page.plugin, cx), cx)
            })
            .map(|page| page.id)
            .collect();
        for id in stale {
            self.close_plugin_page(id, cx);
        }
        let in_panel: Vec<u64> = self.browser.as_ref().map(|b| b.tabs.iter().filter_map(|tab| tab.owner).collect()).unwrap_or_default();
        let waiting = std::mem::take(&mut self.plugin_pages_to_show);
        let mut hidden = Vec::new();
        for page in &mut self.plugin_browsers {
            if page.shown && !in_panel.contains(&page.id) && !waiting.contains(&page.id) {
                page.shown = false;
                if let Some(view) = page.webview.borrow_mut().as_mut() {
                    view.park();
                }
                hidden.push((page.plugin.clone(), page.id));
            }
        }
        for (plugin, id) in hidden {
            crate::plugins::send_if_running(&plugin, "browser/hidden", json!({ "tabId": id }), cx);
        }
        let mut tabs = Vec::new();
        for id in waiting {
            let Some(page) = self.plugin_browsers.iter().find(|page| page.id == id && page.shown) else { continue };
            if in_panel.contains(&id) {
                continue;
            }
            let url = page.webview.borrow().as_ref().and_then(WebView::current_url).unwrap_or_default();
            tabs.push(BrowserTab::for_plugin(page.webview.clone(), url, id, page.plugin_name.clone()));
        }
        if tabs.is_empty() {
            return None;
        }
        self.page = None;
        Some(tabs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_round_trip_and_unknown_ones_are_auto() {
        for mode in BrowserMode::ALL {
            assert_eq!(BrowserMode::from_id(mode.id()), Some(mode));
        }
        assert_eq!(BrowserMode::from_id("sideways"), None);
    }

    #[test]
    fn the_script_is_wrapped_in_a_site_check() {
        let body = eval_body("return document.title;");
        let check = body.find("not one of the plugin").unwrap();
        let run = body.find("return document.title;").unwrap();
        assert!(check < run, "the site check comes before the plugin's script");
        assert!(body.contains("JSON.stringify"));
    }

    #[test]
    fn timeouts_are_bounded() {
        assert_eq!(timeout_of(&json!({})), DEFAULT_TIMEOUT);
        assert_eq!(timeout_of(&json!({ "timeoutMs": 10 })), Duration::from_millis(100));
        assert_eq!(timeout_of(&json!({ "timeoutMs": 9_999_999 })), MAX_TIMEOUT);
    }
}
