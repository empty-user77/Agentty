//! The offer above a CSV file, spreadsheet or Word document opened here while another app opens
//! that type from Finder: make Agentty the one, or not be asked again for that type.

use super::{CodeEditor, Tone};
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, TypeScale};
use gpui::{div, prelude::*, px, ClickEvent, Context};

impl CodeEditor {
    /// Works out once per open file whether to make the offer (Launch Services is asked then).
    pub(super) fn check_default_offer(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(self.active) else { return };
        if doc.default_offer.is_some() {
            return;
        }
        let ask = crate::default_app::should_ask(&doc.path, cx);
        if let Some(doc) = self.docs.get_mut(self.active) {
            doc.default_offer = Some(ask);
        }
    }

    pub(super) fn render_default_offer(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let doc = self.docs.get(self.active).filter(|d| d.default_offer == Some(true))?;
        let kind = doc.path.extension().map(|e| e.to_string_lossy().to_ascii_uppercase()).unwrap_or_default();
        let path = doc.path.clone();
        let button = |id: &'static str, label: String, primary: bool| {
            div()
                .id(id)
                .flex_shrink_0()
                .h(px(22.))
                .px_2p5()
                .flex()
                .items_center()
                .rounded_md()
                .t_small()
                .cursor_pointer()
                .when(primary, |d| d.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::BRIGHT)).hover(|s| s.opacity(0.9)))
                .when(!primary, |d| d.text_color(hex(Chrome::MUTED)).hover(|s| s.bg(hex(Chrome::HOVER)).text_color(hex(Chrome::BRIGHT))))
                .child(label)
        };
        let (make, later) = (path.clone(), path);
        Some(
            div()
                .h(px(34.))
                .flex_shrink_0()
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex_alpha(Chrome::ACCENT, 0.08))
                .child(icon("app-window", 13., hex(Chrome::MUTED)))
                .child(div().flex_1().min_w_0().truncate().t_small().child(tf(cx, "default_app.ask", &[("kind", &kind)])))
                .child(button("default-app-make", t(cx, "default_app.make_default").to_string(), true).on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| {
                        let result = crate::default_app::make_default(&make);
                        for doc in this.docs.iter_mut().filter(|d| d.path.extension() == make.extension()) {
                            doc.default_offer = Some(false);
                        }
                        // macOS asks the user to confirm the change itself: nothing to announce here.
                        match result {
                            Ok(()) => {}
                            Err(err) => {
                                let text = tf(cx, "default_app.failed", &[("error", &err.to_string())]);
                                if let Some(doc) = this.docs.get_mut(this.active) {
                                    doc.set_notice(text, Tone::Error);
                                }
                            }
                        }
                        cx.notify();
                    },
                )))
                .child(button("default-app-later", t(cx, "default_app.not_now").to_string(), false).on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| {
                        crate::default_app::dismiss(&later, cx);
                        for doc in this.docs.iter_mut().filter(|d| d.path.extension() == later.extension()) {
                            doc.default_offer = Some(false);
                        }
                        cx.notify();
                    },
                ))),
        )
    }
}
