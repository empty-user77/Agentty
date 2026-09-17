//! Commands from `agentty browser …` (see `browser_cli`): lets agents in Agentty terminals open,
//! read and operate the in-app browser.

use super::Workbench;
use crate::agent_signal::{browser_reply, BrowserRequest};
use gpui::{Context, Window};
use std::time::Duration;

const TEXT: &str = "const el = selector ? document.querySelector(selector) : document.body;
if (!el) throw new Error('no element matches ' + selector);
return JSON.stringify(el.innerText);";

const HTML: &str = "const el = selector ? document.querySelector(selector) : document.documentElement;
if (!el) throw new Error('no element matches ' + selector);
const html = el.outerHTML; return JSON.stringify(html.length > 200000 ? html.slice(0, 200000) + '…' : html);";

const ELEMENTS: &str = "const esc = (s) => CSS.escape(s);
const path = (node) => { if (node.id) return '#' + esc(node.id); const parts = []; let el = node;
  while (el && el.nodeType === 1 && parts.length < 6) { let part = el.tagName.toLowerCase();
    const name = el.getAttribute('name'); if (name) { parts.unshift(part + '[name=\"' + name + '\"]'); break; }
    const siblings = el.parentElement ? [...el.parentElement.children].filter((c) => c.tagName === el.tagName) : [];
    if (siblings.length > 1) part += ':nth-of-type(' + (siblings.indexOf(el) + 1) + ')';
    parts.unshift(part); if (el.parentElement && el.parentElement.id) { parts.unshift('#' + esc(el.parentElement.id)); break; }
    el = el.parentElement; }
  return parts.join(' > '); };
const out = [];
for (const el of document.querySelectorAll('a[href],button,input,textarea,select,[role=button],[onclick],[contenteditable=true]')) {
  const r = el.getBoundingClientRect(); if (r.width === 0 && r.height === 0) continue;
  out.push({ selector: path(el), tag: el.tagName.toLowerCase(), type: el.type || undefined,
    text: (el.innerText || el.value || el.placeholder || el.getAttribute('aria-label') || '').trim().slice(0, 80), href: el.href || undefined });
  if (out.length >= 200) break; }
return JSON.stringify(out);";

const CLICK: &str = "const el = document.querySelector(selector);
if (!el) throw new Error('no element matches ' + selector);
el.scrollIntoView({ block: 'center' }); if (el.focus) el.focus(); el.click(); return JSON.stringify(true);";

const TYPE: &str = "const el = document.querySelector(selector);
if (!el) throw new Error('no element matches ' + selector);
el.focus();
if (el.isContentEditable) { el.textContent = text; } else {
  const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : el instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, 'value'); if (setter && setter.set) setter.set.call(el, text); else el.value = text; }
el.dispatchEvent(new Event('input', { bubbles: true })); el.dispatchEvent(new Event('change', { bubbles: true }));
return JSON.stringify(true);";

const PRESS: &str = "const el = document.activeElement || document.body;
for (const type of ['keydown', 'keypress', 'keyup']) el.dispatchEvent(new KeyboardEvent(type, { key, code: key, bubbles: true, cancelable: true }));
if (key === 'Enter' && el.form) { if (el.form.requestSubmit) el.form.requestSubmit(); else el.form.submit(); }
return JSON.stringify(true);";

const WAIT: &str = "const deadline = Date.now() + Number(timeout);
while (Date.now() < deadline) { if (document.querySelector(selector)) return JSON.stringify(true); await new Promise((r) => setTimeout(r, 100)); }
throw new Error('timed out waiting for ' + selector);";

const CONSOLE: &str =
    "const logs = window.__agenttyConsole || []; const out = logs.slice(); if (clear === '1') logs.length = 0; return JSON.stringify(out);";

impl Workbench {
    pub fn browser_command(&mut self, request: BrowserRequest, window: &mut Window, cx: &mut Context<Self>) {
        crate::metrics::track(cx, "feature_used", serde_json::json!({ "feature": "browser_api" }));
        self.run_browser_command(request, 0, window, cx);
    }

    fn run_browser_command(&mut self, request: BrowserRequest, attempt: u32, window: &mut Window, cx: &mut Context<Self>) {
        let reply = request.reply.clone();
        let send = move |result: Result<String, String>| {
            let _ = reply.send(browser_reply(result));
        };
        let arg = |i: usize| request.args.get(i).cloned();
        let json = |value: serde_json::Value| Ok(value.to_string());
        match request.command.as_str() {
            "open" | "navigate" => {
                if request.command == "navigate" && arg(0).is_none() {
                    return send(Err("usage: agentty browser navigate <url>".into()));
                }
                let url = arg(0).map(|a| super::browser::browser_url(&a, cx));
                self.open_browser(url.clone(), cx);
                // Create the web view now rather than on the next frame (which may not come soon,
                // e.g. while the window is covered).
                self.prepare_browser(window, cx);
                window.refresh();
                return send(json(serde_json::json!({ "opened": true, "url": url })));
            }
            "close" => {
                self.browser = None;
                crate::webview::focus_gpui_view(window);
                cx.notify();
                return send(json(serde_json::json!(true)));
            }
            _ => {}
        }
        // Everything else needs the web view, which appears on the next frame after `open`.
        let ready =
            self.browser.as_ref().is_some_and(|b| b.webview.borrow().is_some() && b.pending.is_none()) && self.browser_request.is_none();
        if !ready {
            if self.browser.is_none() && self.browser_request.is_none() {
                return send(Err("the browser is not open — run `agentty browser open [url]` first".into()));
            }
            if attempt >= 25 {
                return send(Err("the browser did not start (is the Agentty window visible?)".into()));
            }
            self.prepare_browser(window, cx);
            window.refresh();
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                let _ = this.update_in(cx, |this, window, cx| this.run_browser_command(request, attempt + 1, window, cx));
            })
            .detach();
            return;
        }
        let Some(browser) = self.browser.as_ref() else { return };
        let webview = browser.webview.clone();
        let borrowed = webview.borrow();
        let Some(view) = borrowed.as_ref() else { return };
        let js = |body: &str, args: &[(&str, &str)]| view.call_async(body, args, Box::new(send.clone()));
        let selector = arg(0).unwrap_or_default();
        match request.command.as_str() {
            "url" => send(json(serde_json::json!(view.current_url()))),
            "title" => send(json(serde_json::json!(view.title()))),
            "status" => send(json(serde_json::json!({ "url": view.current_url(), "title": view.title(), "loading": view.is_loading() }))),
            "back" => {
                view.back();
                send(json(serde_json::json!(true)))
            }
            "forward" => {
                view.forward();
                send(json(serde_json::json!(true)))
            }
            "reload" => {
                view.reload();
                send(json(serde_json::json!(true)))
            }
            "wait-load" => {
                let timeout = arg(0).and_then(|t| t.parse::<u64>().ok()).unwrap_or(15_000);
                let webview = webview.clone();
                drop(borrowed);
                cx.spawn(async move |_, cx| {
                    let started = std::time::Instant::now();
                    loop {
                        cx.background_executor().timer(Duration::from_millis(150)).await;
                        let loading = webview.borrow().as_ref().map(|v| v.is_loading());
                        match loading {
                            Some(false) => return send(Ok(serde_json::json!(true).to_string())),
                            None => return send(Err("the browser was closed".into())),
                            Some(true) if started.elapsed().as_millis() as u64 > timeout => {
                                return send(Err("timed out waiting for the page to load".into()))
                            }
                            Some(true) => {}
                        }
                    }
                })
                .detach();
            }
            "text" => js(TEXT, &[("selector", &selector)]),
            "html" => js(HTML, &[("selector", &selector)]),
            "elements" => js(ELEMENTS, &[]),
            "click" if !selector.is_empty() => js(CLICK, &[("selector", &selector)]),
            "type" if request.args.len() >= 2 => js(TYPE, &[("selector", &selector), ("text", &request.args[1..].join(" "))]),
            "press" if !selector.is_empty() => js(PRESS, &[("key", &selector)]),
            "wait" if !selector.is_empty() => {
                let timeout = arg(1).unwrap_or_else(|| "10000".into());
                js(WAIT, &[("selector", &selector), ("timeout", &timeout)])
            }
            "console" => js(CONSOLE, &[("clear", if selector == "clear" { "1" } else { "0" })]),
            "eval" if !request.args.is_empty() => {
                let code = request.args.join(" ");
                // An expression first; statements (with `return`) when that doesn't parse.
                let expression = format!("return JSON.stringify(await (async () => (\n{code}\n))() ?? null);");
                let statements = format!("return JSON.stringify(await (async () => {{\n{code}\n}})() ?? null);");
                let webview = webview.clone();
                view.call_async(
                    &expression,
                    &[],
                    Box::new(move |result| match result {
                        Err(error) if error.contains("SyntaxError") || error.to_lowercase().contains("syntax") => {
                            if let Some(view) = webview.borrow().as_ref() {
                                view.call_async(&statements, &[], Box::new(send));
                            }
                        }
                        other => send(other),
                    }),
                )
            }
            "screenshot" => match arg(0) {
                Some(path) => view.snapshot_png(std::path::PathBuf::from(path), Box::new(send)),
                None => send(Err("usage: agentty browser screenshot <path.png>".into())),
            },
            other => send(Err(format!("unknown or incomplete command `{other}` — see `agentty browser help`"))),
        }
    }
}
