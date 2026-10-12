//! What the in-app browser puts into every page and reads back out of it, the same on every
//! platform that has one (WebKit on macOS, WebView2 on Windows).

/// Records `console.*` calls, uncaught errors and rejections (last 500) in `window.__agenttyConsole`.
pub const CONSOLE_CAPTURE: &str = r#"(() => { if (window.__agenttyConsole) return; const logs = []; window.__agenttyConsole = logs;
const push = (level, text) => { logs.push({ level, time: Date.now(), text }); if (logs.length > 500) logs.shift(); };
const show = (x) => { try { return typeof x === 'string' ? x : JSON.stringify(x); } catch (e) { return String(x); } };
for (const level of ['log', 'info', 'warn', 'error', 'debug']) { const original = console[level];
  console[level] = function (...args) { try { push(level, args.map(show).join(' ')); } catch (e) {} return original.apply(this, args); }; }
window.addEventListener('error', (e) => { if (e.filename) push('error', `${e.message} @ ${e.filename}:${e.lineno}`); });
window.addEventListener('unhandledrejection', (e) => push('error', `Unhandled rejection: ${show(e.reason)}`)); })();"#;

/// Records `fetch` and `XMLHttpRequest` calls (request, response, status, timing, body) in
/// `window.__agenttyNet` for the network panel at the bottom of the browser. Bodies are kept
/// truncated and only for text-like responses, so a page that streams megabytes stays cheap.
pub const NETWORK_CAPTURE: &str = r#"(() => { if (window.__agenttyNet) return;
const MAX = 300, BODY = 12000, FIELD = 2000;
const cut = (text) => { text = String(text == null ? '' : text); return text.length > FIELD ? text.slice(0, FIELD) + '\u2026' : text; };
const state = { seq: 0, entries: [] }; window.__agenttyNet = state;
const add = (entry) => { entry.id = ++state.seq; state.entries.push(entry);
  if (state.entries.length > MAX) state.entries.shift(); return entry; };
const trim = (text) => typeof text === 'string' ? (text.length > BODY ? text.slice(0, BODY) + '…' : text) : '';
const textLike = (type) => /json|text|xml|javascript|html|form-urlencoded/i.test(type || '');
const headerList = (source) => { const out = []; try {
    if (!source) return out;
    if (typeof source.forEach === 'function' && typeof source.get === 'function') { source.forEach((v, k) => out.push([cut(k), cut(v)])); return out; }
    if (Array.isArray(source)) { for (const pair of source) out.push([cut(pair[0]), cut(pair[1])]); return out; }
    for (const key of Object.keys(source)) out.push([cut(key), cut(source[key])]);
  } catch (e) {} return out; };
const absolute = (url) => { try { return cut(new URL(url, location.href).href); } catch (e) { return cut(url); } };
const originalFetch = window.fetch;
if (originalFetch) window.fetch = function (input, init) {
  const request = (typeof Request !== 'undefined' && input instanceof Request) ? input : null;
  const url = absolute(request ? request.url : input);
  const method = cut(String((init && init.method) || (request && request.method) || 'GET').toUpperCase());
  const started = performance.now();
  const body = init && typeof init.body === 'string' ? init.body : '';
  const entry = add({ kind: 'fetch', method, url, status: 0, start: Date.now(), reqBody: trim(body),
    reqHeaders: headerList((init && init.headers) || (request && request.headers)) });
  return originalFetch.apply(this, arguments).then((response) => {
    entry.status = response.status; entry.statusText = cut(response.statusText || '');
    entry.ok = response.ok; entry.duration = Math.round(performance.now() - started);
    entry.resHeaders = headerList(response.headers);
    try { entry.type = cut(response.headers.get('content-type') || ''); } catch (e) { entry.type = ''; }
    let length = 0; try { length = Number(response.headers.get('content-length')) || 0; } catch (e) {}
    if (length) entry.size = length;
    if (textLike(entry.type)) { try { response.clone().text().then((text) => {
      entry.body = trim(text); if (!entry.size) entry.size = text.length; }).catch(() => {}); } catch (e) {} }
    return response;
  }).catch((error) => { entry.error = cut((error && error.message) || error);
    entry.duration = Math.round(performance.now() - started); throw error; });
};
const XHR = window.XMLHttpRequest;
if (XHR && XHR.prototype) { const open = XHR.prototype.open, send = XHR.prototype.send;
  XHR.prototype.open = function (method, url) { try { this.__agentty = { method: cut(String(method).toUpperCase()), url: absolute(url) }; } catch (e) {}
    return open.apply(this, arguments); };
  XHR.prototype.send = function (body) { const info = this.__agentty;
    if (info) { const started = performance.now();
      const entry = add({ kind: 'xhr', method: info.method, url: info.url, status: 0, start: Date.now(),
        reqBody: trim(typeof body === 'string' ? body : ''), reqHeaders: [] });
      this.addEventListener('loadend', () => {
        entry.status = this.status; entry.ok = this.status >= 200 && this.status < 400;
        entry.duration = Math.round(performance.now() - started);
        try { entry.type = cut(this.getResponseHeader('content-type') || ''); } catch (e) { entry.type = ''; }
        try { entry.resHeaders = (this.getAllResponseHeaders() || '').trim().split(/\r?\n/).filter(Boolean).slice(0, 100)
          .map((line) => { const at = line.indexOf(':'); return at < 0 ? [cut(line), ''] : [cut(line.slice(0, at)), cut(line.slice(at + 1).trim())]; }); } catch (e) {}
        if (!this.status) entry.error = 'network error';
        try { if (this.responseType === '' || this.responseType === 'text') {
          entry.body = trim(this.responseText); entry.size = this.responseText.length; } } catch (e) {}
      });
    }
    return send.apply(this, arguments); }; }
})();"#;

/// How much the page has pulled over the network so far, read from the Resource Timing API (the
/// real transfer, compression included) — the summary shown in the browser's status bar.
pub const NET_TOTALS: &str = r#"let transferred = 0, decoded = 0, count = 0;
const take = (entry) => { transferred += entry.transferSize || 0; decoded += entry.decodedBodySize || 0; count += 1; };
try { for (const entry of performance.getEntriesByType('navigation')) take(entry); } catch (e) {}
try { for (const entry of performance.getEntriesByType('resource')) take(entry); } catch (e) {}
const state = window.__agenttyNet;
return JSON.stringify({ transferred, decoded, count, calls: state ? state.entries.length : 0 });"#;

/// The API calls the page made, newest last and without bodies (those are fetched per request).
/// `window.__agenttyNet` lives in the page and the page may write anything into it, so everything
/// read back out is cut to a length the panel can show before it crosses into Agentty.
pub const NET_ENTRIES: &str = r#"const state = window.__agenttyNet; if (!state) return JSON.stringify([]);
const cut = (v) => String(v == null ? '' : v).slice(0, 2000);
const num = (v) => { const n = Number(v); return Number.isFinite(n) ? Math.max(0, Math.min(n, 1e12)) : 0; };
return JSON.stringify(state.entries.slice(-200).map((e) => ({ id: num(e.id), kind: cut(e.kind), method: cut(e.method), url: cut(e.url),
  status: num(e.status), ok: !!e.ok, duration: num(e.duration), size: num(e.size), type: cut(e.type), error: cut(e.error) })));"#;

/// Everything about one recorded call: headers and the (truncated) request and response bodies.
pub const NET_DETAIL: &str = r#"const state = window.__agenttyNet; if (!state) throw new Error('nothing recorded');
const entry = state.entries.find((e) => String(e.id) === String(id));
if (!entry) throw new Error('this request is no longer recorded');
const cut = (v, max) => String(v == null ? '' : v).slice(0, max);
const num = (v) => { const n = Number(v); return Number.isFinite(n) ? Math.max(0, Math.min(n, 1e12)) : 0; };
const headers = (list) => (Array.isArray(list) ? list : []).slice(0, 100).map((p) => [cut(p && p[0], 2000), cut(p && p[1], 2000)]);
return JSON.stringify({ id: num(entry.id), method: cut(entry.method, 2000), url: cut(entry.url, 2000), status: num(entry.status),
  statusText: cut(entry.statusText, 2000), duration: num(entry.duration), size: num(entry.size), type: cut(entry.type, 2000),
  error: cut(entry.error, 2000), reqBody: cut(entry.reqBody, 16000), body: cut(entry.body, 16000),
  reqHeaders: headers(entry.reqHeaders), resHeaders: headers(entry.resHeaders) });"#;

/// Whether a page may go to `url` by itself (a link, a redirect, a script, a form): the web and
/// what a page builds in memory. `file:`, `javascript:` and apps' own schemes (`zoommtg:`,
/// `agentty:`, …) are refused outright rather than left to what WebKit happens to do with them.
pub fn navigation_allowed(url: &str) -> bool {
    let scheme = url.split(':').next().unwrap_or_default().to_ascii_lowercase();
    matches!(scheme.as_str(), "http" | "https" | "about" | "data" | "blob")
}
