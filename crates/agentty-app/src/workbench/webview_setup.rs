//! Windows: "the in-app browser needs a component". The browser runs on Microsoft Edge WebView2,
//! which Windows 10 and 11 normally have; where it is missing, opening the browser shows this
//! dialog, which downloads Microsoft's installer, checks its signature and runs it — then the
//! browser opens as asked.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::TypeScale;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context};

/// The dialog's state.
pub(super) struct WebviewSetup {
    /// What was being opened (it opens once the component is there).
    url: Option<String>,
    installing: bool,
    error: Option<String>,
}

impl Workbench {
    /// Shows the dialog instead of the browser when WebView2 is missing; `true` when it did.
    pub(super) fn ask_for_webview(&mut self, url: Option<String>, cx: &mut Context<Self>) -> bool {
        if crate::webview::available() {
            return false;
        }
        match self.webview_setup.as_mut() {
            Some(setup) => setup.url = url.or(setup.url.take()),
            None => self.webview_setup = Some(WebviewSetup { url, installing: false, error: None }),
        }
        cx.notify();
        true
    }

    fn install_webview(&mut self, cx: &mut Context<Self>) {
        let Some(setup) = self.webview_setup.as_mut().filter(|s| !s.installing) else { return };
        setup.installing = true;
        setup.error = None;
        cx.notify();
        let work = cx.background_executor().spawn(async { crate::webview::install_runtime() });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) if crate::webview::available() => {
                        let url = this.webview_setup.take().and_then(|s| s.url);
                        this.open_browser(url, cx);
                    }
                    Ok(()) => {}
                    Err(error) => {
                        if let Some(setup) = this.webview_setup.as_mut() {
                            setup.installing = false;
                            setup.error = Some(error);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_webview_setup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let setup = self.webview_setup.as_ref()?;
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
        let installing = setup.installing;
        Some(
            div()
                .id("webview-setup-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hex_alpha(0x000000, 0.45))
                .occlude()
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    if !this.webview_setup.as_ref().is_some_and(|s| s.installing) {
                        this.webview_setup = None;
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .id("webview-setup")
                        .w(px(460.))
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
                        .child(
                            div()
                                .t_title()
                                .font_weight(crate::theme::EMPHASIS)
                                .text_color(hex(Chrome::BRIGHT))
                                .child(t(cx, "webview_setup.title").to_string()),
                        )
                        .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(t(cx, "webview_setup.body").to_string()))
                        .children(setup.error.as_ref().map(|error| {
                            div().t_small().text_color(hex(Chrome::ERROR)).child(tf(cx, "webview_setup.failed", &[("error", error)]))
                        }))
                        .child(
                            div()
                                .pt_1()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .when(!installing, |d| {
                                    d.child(button("webview-setup-close", t(cx, "confirm.close").to_string(), false).on_click(cx.listener(
                                        |this, _: &ClickEvent, _, cx| {
                                            this.webview_setup = None;
                                            cx.notify();
                                        },
                                    )))
                                    .child(
                                        button("webview-setup-default", t(cx, "webview_setup.default_browser").to_string(), false)
                                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                                let url = this.webview_setup.take().and_then(|s| s.url);
                                                let home = crate::settings::settings(cx).browser.home.clone();
                                                cx.open_url(&url.unwrap_or_else(|| super::browser::browser_url(&home, cx)));
                                                cx.notify();
                                            })),
                                    )
                                })
                                .child(
                                    button(
                                        "webview-setup-install",
                                        t(cx, if installing { "webview_setup.installing" } else { "webview_setup.install" }).to_string(),
                                        true,
                                    )
                                    .when(!installing, |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.install_webview(cx)))),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
