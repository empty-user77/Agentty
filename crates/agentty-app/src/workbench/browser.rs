//! In-app browser panel docked to the right of the terminals (like cmux's browser split).
//! Links open here or in the default browser, per Settings → General.

use super::Workbench;
use crate::i18n::t;
use crate::settings::{settings, LinkOpener};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{hex, Chrome};
use crate::ui::{icon_only, TypeScale};
use crate::webview::{normalize_url_with, LoadError, WebView};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, Subscription, Window};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// What the user typed as a URL, or a search with their chosen engine.
pub(super) fn browser_url(input: &str, cx: &gpui::App) -> String {
    normalize_url_with(input, settings(cx).browser.search_engine.query_prefix())
}

pub struct BrowserPanel {
    pub(super) webview: Rc<RefCell<Option<WebView>>>,
    address: Entity<TextInput>,
    /// URL to load once the native view exists (it needs the window).
    pub(super) pending: Option<String>,
    title: String,
    loading: bool,
    /// Progress of the current load (0.0–1.0), shown as the bar under the toolbar.
    progress: f32,
    /// When the last load finished: the full bar stays up briefly so even a quick reload shows.
    finished_at: Option<std::time::Instant>,
    /// The page could not be opened: shown instead of the web view, with a retry.
    error: Option<LoadError>,
    /// Last URL written into the address bar by navigation; anything else there is the user typing.
    synced_url: String,
    _subscription: Subscription,
}

impl BrowserPanel {
    /// The load just finished and the full bar is still shown.
    fn finishing(&self) -> bool {
        self.finished_at.is_some_and(|at| at.elapsed() < Duration::from_millis(350))
    }

    /// Address of the page shown (or about to be).
    pub(super) fn current_url(&self) -> Option<String> {
        self.pending.clone().or_else(|| self.webview.borrow().as_ref().and_then(|view| view.current_url()))
    }
}

impl Workbench {
    /// Opens `url` according to the user's choice (in-app panel or the default browser).
    pub(super) fn open_link(&mut self, url: String, cx: &mut Context<Self>) {
        match settings(cx).link_opener {
            LinkOpener::External => cx.open_url(&url),
            LinkOpener::InApp => self.open_browser(Some(url), cx),
        }
    }

    pub(super) fn toggle_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.browser.take().is_some() {
            crate::webview::focus_gpui_view(window);
            self.focus_active(window, cx);
            return cx.notify();
        }
        self.open_browser(None, cx);
    }

    /// Shows the panel (loading `url` if given). The panel itself is built on the next render.
    pub(super) fn open_browser(&mut self, url: Option<String>, cx: &mut Context<Self>) {
        if !crate::platform::HAS_WEBVIEW {
            // No embedded browser on this platform: the default browser opens the page instead.
            return cx.open_url(&url.unwrap_or_else(|| browser_url(&settings(cx).browser.home, cx)));
        }
        self.page = None;
        match self.browser.as_mut() {
            Some(browser) => {
                if let Some(url) = url {
                    browser.pending = Some(url);
                }
            }
            None => self.browser_request = Some(url.unwrap_or_else(|| browser_url(&settings(cx).browser.home, cx))),
        }
        cx.notify();
    }

    /// Materializes a requested panel and keeps the native view in sync; called from render.
    pub(super) fn prepare_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(url) = self.browser_request.take() {
            let address = cx.new(|cx| TextInput::localized(url.clone(), "browser.address", window, cx));
            let subscription = cx.subscribe(&address, |this, input, event: &TextInputEvent, cx| {
                if matches!(event, TextInputEvent::Confirmed) {
                    let url = browser_url(input.read(cx).text(), cx);
                    if let Some(browser) = this.browser.as_mut() {
                        browser.synced_url = input.read(cx).text().to_string();
                        browser.pending = Some(url);
                    }
                    cx.notify();
                }
            });
            self.browser = Some(BrowserPanel {
                webview: Rc::new(RefCell::new(WebView::new(window, &settings(cx).browser))),
                address,
                pending: Some(url.clone()),
                title: String::new(),
                loading: true,
                progress: 0.,
                finished_at: None,
                error: None,
                synced_url: url.clone(),
                _subscription: subscription,
            });
            // Follow navigation inside the page: address bar, title, progress bar. Polled quickly
            // while a page loads so the bar moves smoothly, slowly otherwise.
            cx.spawn(async move |this, cx| {
                let mut interval = Duration::from_millis(600);
                loop {
                    cx.background_executor().timer(interval).await;
                    let alive = this.update(cx, |this, cx| {
                        let Some(browser) = this.browser.as_mut() else { return false };
                        let (url, title, loading, progress, error) = match browser.webview.borrow().as_ref() {
                            Some(view) => (
                                view.current_url(),
                                view.title().unwrap_or_default(),
                                view.is_loading(),
                                view.estimated_progress(),
                                view.load_error(),
                            ),
                            None => return true,
                        };
                        let mut changed = title != browser.title
                            || loading != browser.loading
                            || (loading && progress != browser.progress)
                            || error != browser.error;
                        // The failed address stays in the address bar (the web view still has the
                        // previous page's URL).
                        if let Some(error) = error.as_ref().filter(|e| !e.url.is_empty() && browser.error.as_ref() != Some(e)) {
                            browser.address.update(cx, |i, cx| i.set_text(error.url.clone(), cx));
                            browser.synced_url = error.url.clone();
                        }
                        let failed = error.is_some();
                        browser.error = error;
                        if browser.loading && !loading {
                            browser.finished_at = Some(std::time::Instant::now());
                        }
                        let finishing = browser.finishing();
                        if !loading && !finishing && browser.finished_at.take().is_some() {
                            changed = true; // the full bar goes away
                        }
                        interval = Duration::from_millis(if loading || finishing { 50 } else { 600 });
                        browser.title = title;
                        browser.loading = loading;
                        browser.progress = if loading { progress } else { 1. };
                        let typing = browser.address.read(cx).text() != browser.synced_url;
                        if let Some(url) = url.filter(|u| *u != browser.synced_url && !typing && !failed) {
                            browser.address.update(cx, |i, cx| i.set_text(url.clone(), cx));
                            browser.synced_url = url;
                            changed = true;
                        }
                        if changed {
                            cx.notify();
                        }
                        true
                    });
                    if !matches!(alive, Ok(true)) {
                        break;
                    }
                }
            })
            .detach();
        }
        // Pages (settings, Git, …) take the whole area: close the browser rather than hide it.
        if self.page.is_some() && self.browser.take().is_some() {
            crate::webview::focus_gpui_view(window);
        }
        // Menus and popups sit on top of it: hide the native view meanwhile.
        let hidden = self.overlay_open();
        if let Some(browser) = self.browser.as_mut() {
            if hidden {
                if let Some(view) = browser.webview.borrow_mut().as_mut() {
                    view.hide();
                }
            }
            if let (Some(url), Some(view)) = (browser.pending.clone(), browser.webview.borrow().as_ref()) {
                view.load(&url);
                browser.pending = None;
                browser.loading = true;
                browser.progress = 0.;
                browser.finished_at = None;
                browser.error = None;
                // The address bar shows where it is going, as a browser does.
                if browser.address.read(cx).text() != url {
                    browser.address.update(cx, |i, cx| i.set_text(url.clone(), cx));
                }
                browser.synced_url = url;
            }
        }
    }

    /// The reload button and "Try again". A page that failed to load (a dev server not up yet)
    /// is not the web view's URL — WebKit's reload would load the previous page, or nothing when
    /// there was none — so the failed address is opened again instead.
    pub(super) fn reload_browser(&mut self, cx: &mut Context<Self>) {
        let Some(browser) = self.browser.as_mut() else { return };
        if let Some(view) = browser.webview.borrow().as_ref() {
            match (browser.error.take(), view.current_url()) {
                (Some(error), _) if !error.url.is_empty() => view.load(&error.url),
                (_, Some(_)) => view.reload(),
                (_, None) => view.load(&browser_url(&browser.synced_url, cx)),
            }
        }
        browser.loading = true;
        browser.progress = 0.;
        browser.finished_at = None;
        cx.notify();
    }

    /// Whether something drawn by GPUI would sit on top of the native web view.
    fn overlay_open(&self) -> bool {
        self.page.is_some()
            || self.launcher_open
            || self.notices_open
            || self.picker.is_some()
            || self.palette.is_some()
            || self.updates.popup
            || self.about_open
            || self.tab_menu.is_some()
            || self.status_menu.is_some()
            || self.branch_menu.is_some()
            || self.resume_menu.is_some()
            || self.close_confirm.is_some()
            || self.agent_panel.is_some()
            || self.prompt_dialog.is_some()
            || self.harness_dialog.is_some()
            || self.onboarding.as_ref().is_some_and(|o| o.is_modal())
            // Native views swallow mouse events; hide it so the resize drag keeps reaching GPUI.
            || self.browser_resizing
    }

    /// Splitter between the terminals and the browser: a real layout column (not overlapping the
    /// native view, which would swallow the mouse).
    pub(super) fn render_browser_splitter(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.browser.as_ref()?;
        Some(
            div()
                .id("browser-resize")
                .group("browser-resize")
                .w(px(5.))
                .flex_shrink_0()
                .h_full()
                .flex()
                .justify_center()
                .cursor(gpui::CursorStyle::ResizeLeftRight)
                // A hairline at rest; the whole grab area lights up on hover or while dragging.
                .child(
                    div()
                        .w(px(1.))
                        .h_full()
                        .bg(hex(Chrome::BORDER))
                        .group_hover("browser-resize", |s| s.w(px(5.)).bg(hex(Chrome::ACCENT)))
                        .when(self.browser_resizing, |d| d.w(px(5.)).bg(hex(Chrome::ACCENT))),
                )
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.browser_resizing = true;
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_browser(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let browser = self.browser.as_ref()?;
        let webview = browser.webview.clone();
        let hidden = self.overlay_open();
        // A failed load shows its message in place of the page (the native view would cover it).
        let covered = hidden || browser.error.is_some();
        if covered {
            if let Some(view) = webview.borrow_mut().as_mut() {
                view.hide();
            }
        }
        let nav = |id: &'static str, glyph: &'static str, action: fn(&WebView), cx: &mut Context<Self>| {
            let webview = browser.webview.clone();
            icon_only(
                id,
                glyph,
                cx.listener(move |_, _: &ClickEvent, _, _| {
                    if let Some(view) = webview.borrow().as_ref() {
                        action(view);
                    }
                }),
            )
        };
        let external = browser.address.clone();
        let toolbar = div()
            .h(px(36.))
            .flex_shrink_0()
            .px_1()
            .flex()
            .items_center()
            .gap_0p5()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::SIDE_BAR))
            .child(nav("browser-back", "arrow-left", WebView::back, cx))
            .child(nav("browser-forward", "arrow-right", WebView::forward, cx))
            .child(icon_only("browser-reload", "rotate-cw", cx.listener(|this, _: &ClickEvent, _, cx| this.reload_browser(cx))))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .mx_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(hex(if browser.loading { Chrome::ACCENT } else { Chrome::BORDER }))
                    .bg(hex(0x1a1a1a))
                    .t_small()
                    .text_color(hex(Chrome::BRIGHT))
                    .child(browser.address.clone()),
            )
            .child(icon_only(
                "browser-external",
                "arrow-up-right",
                cx.listener(move |_, _: &ClickEvent, _, cx| {
                    let url = external.read(cx).text().to_string();
                    cx.open_url(&browser_url(&url, cx));
                }),
            ))
            .child(icon_only(
                "browser-settings",
                "settings",
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.settings_section = super::settings_page::SettingsSection::Browser;
                    this.page = Some(super::Page::Settings);
                    cx.notify();
                }),
            ))
            .child(icon_only("browser-close", "x", cx.listener(|this, _: &ClickEvent, window, cx| this.toggle_browser(window, cx))));
        // The native view is positioned over this element after layout.
        let content = gpui::canvas(
            |_, _, _| {},
            move |bounds, _, _, _| {
                if let Some(view) = webview.borrow_mut().as_mut() {
                    if !covered {
                        view.set_frame(bounds);
                    }
                }
            },
        )
        .flex_1()
        .size_full();
        Some(
            div()
                .relative()
                .w(px(self.docked_widths(cx).0))
                .flex_shrink_0()
                .h_full()
                .flex()
                .flex_col()
                .bg(hex(0xffffff))
                .child(toolbar)
                .child(progress_bar(browser.loading || browser.finishing(), browser.progress))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .child(content)
                        .when_some(browser.error.clone().filter(|_| !hidden), |d, error| d.child(self.render_load_error(error, cx)))
                        .when(hidden, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .bg(hex(Chrome::EDITOR))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .t_small()
                                    .text_color(hex(Chrome::MUTED))
                                    .child(browser.title.clone()),
                            )
                        }),
                )
                .into_any_element(),
        )
    }
}

impl Workbench {
    /// "This page couldn't be opened": the address, WebKit's reason and a retry.
    fn render_load_error(&self, error: LoadError, cx: &mut Context<Self>) -> AnyElement {
        div()
            .absolute()
            .inset_0()
            .bg(hex(Chrome::EDITOR))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .px_6()
            .child(div().t_body().text_color(hex(Chrome::BRIGHT)).child(t(cx, "browser.load_failed")))
            .child(div().t_small().text_color(hex(Chrome::MUTED)).text_center().child(error.url))
            .when(!error.message.is_empty(), |d| d.child(div().t_small().text_color(hex(Chrome::MUTED)).text_center().child(error.message)))
            .child(div().pt_2().child(crate::ui::action_button(
                "browser-retry",
                t(cx, "browser.retry"),
                cx.listener(|this, _: &ClickEvent, _, cx| this.reload_browser(cx)),
            )))
            .into_any_element()
    }
}

/// A thin bar under the toolbar that fills while a page loads, like a browser's. Its own row (not
/// drawn over the page): the native web view would cover it.
fn progress_bar(shown: bool, progress: f32) -> gpui::Div {
    // WebKit starts at 0.1; a sliver shows right away even before its first estimate.
    let fill = progress.clamp(0.08, 1.);
    div()
        .h(px(3.))
        .w_full()
        .flex_shrink_0()
        .bg(hex(Chrome::SIDE_BAR))
        .when(shown, |d| d.child(div().h_full().w(gpui::relative(fill)).rounded_r_sm().bg(hex(Chrome::BLUE))))
}
