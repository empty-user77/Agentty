//! The call being typed: after `(` or `,` the server's signature shows above the line, the
//! parameter being typed in the accent color (VS Code's parameter hints). It follows the typing
//! inside the call and closes at `)`, on Esc, or when the cursor leaves.

use super::buffer::Pos;
use super::lsp;
use super::{CodeEditor, Content};
use crate::theme::{hex, Chrome};
use gpui::{div, prelude::*, px, AnyElement, Context};
use std::path::PathBuf;
use std::time::Duration;

/// How long the server gets to answer.
const WAIT: Duration = Duration::from_millis(1200);

pub(super) struct SignatureHint {
    path: PathBuf,
    /// Where the cursor was when it was asked (it shows above that line).
    at: Pos,
    signature: Option<lsp::Signature>,
    asking: u64,
}

impl CodeEditor {
    /// After `typed` went in at the cursor: `(` and `,` ask for the signature, `)` closes it, and
    /// anything else typed while it shows asks again (the parameter may have changed).
    pub(super) fn signature_typed(&mut self, typed: &str, cx: &mut Context<Self>) {
        let trigger = match typed {
            "(" | "," => typed.chars().next(),
            ")" => {
                self.signature = None;
                return cx.notify();
            }
            _ if self.signature.is_some() => None,
            _ => return,
        };
        self.request_signature(trigger, cx);
    }

    fn request_signature(&mut self, trigger: Option<char>, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(self.active).filter(|d| matches!(d.content, Content::Text) && d.editable()) else { return };
        if lsp::id(doc.language).is_none() || lsp::status(&doc.project, doc.language, &doc.path) != "ready" {
            return;
        }
        self.signature_seq += 1;
        let asking = self.signature_seq;
        let cursor = doc.buffer.cursor();
        let (project, language, path, text) = (doc.project.clone(), doc.language, doc.path.clone(), lsp::Snapshot::of(&doc.buffer));
        match self.signature.as_mut().filter(|s| s.path == path && s.at.line == cursor.line) {
            Some(open) => {
                open.asking = asking;
                open.at = cursor;
            }
            None => self.signature = Some(SignatureHint { path: path.clone(), at: cursor, signature: None, asking }),
        }
        cx.spawn(async move |this, cx| {
            let found =
                cx.background_spawn(async move { lsp::signature_help(&project, language, &path, &text, cursor, trigger, WAIT) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.signature.as_ref().is_some_and(|s| s.asking == asking) {
                    match found {
                        Some(found) => {
                            if let Some(open) = this.signature.as_mut() {
                                open.signature = Some(found);
                            }
                        }
                        // Out of the call: nothing to show.
                        None => this.signature = None,
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn render_signature(&self, cx: &gpui::App) -> Option<AnyElement> {
        if self.picker.is_some() || self.confirm.is_some() || self.preview.covered || self.rename.is_some() {
            return None;
        }
        let hint = self.signature.as_ref()?;
        let signature = hint.signature.as_ref()?;
        self.docs.get(self.active).filter(|d| d.path == hint.path)?;
        let layout = self.layout.as_ref()?;
        let top = layout.point_at(Pos::new(hint.at.line, hint.at.col))?;
        // Above the line, so the completion list below it stays free.
        let at = gpui::point(top.x, top.y - px(4.));
        let label = &signature.label;
        let (before, active, after) = match signature.active.filter(|(s, e)| s <= e && *e <= label.len()) {
            Some((s, e)) if label.is_char_boundary(s) && label.is_char_boundary(e) => (&label[..s], &label[s..e], &label[e..]),
            _ => (label.as_str(), "", ""),
        };
        let panel = div()
            .id("editor-signature")
            .max_w(px(640.))
            .px_2()
            .py_1()
            .rounded_md()
            .bg(hex(Chrome::OVERLAY))
            .border_1()
            .border_color(hex(Chrome::OVERLAY_BORDER))
            .shadow_lg()
            .font_family(crate::settings::terminal_font(cx))
            .text_size(px(12.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .text_color(hex(Chrome::FOREGROUND))
                    .child(before.to_string())
                    .child(div().text_color(hex(Chrome::ATTENTION)).font_weight(gpui::FontWeight::BOLD).child(active.to_string()))
                    .child(after.to_string()),
            )
            .when_some(signature.doc.clone(), |d, doc| d.child(div().truncate().text_color(hex(Chrome::MUTED)).child(doc)));
        Some(
            gpui::deferred(
                gpui::anchored().anchor(gpui::Corner::BottomLeft).position(at).snap_to_window_with_margin(self.popup_margin()).child(panel),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }

    pub(super) fn debug_signature(&self) -> serde_json::Value {
        match self.signature.as_ref().and_then(|s| s.signature.as_ref()) {
            Some(s) => serde_json::json!({
                "label": s.label,
                "active": s.active.and_then(|(a, b)| s.label.get(a..b)),
                "doc": s.doc,
            }),
            None => serde_json::Value::Null,
        }
    }
}
