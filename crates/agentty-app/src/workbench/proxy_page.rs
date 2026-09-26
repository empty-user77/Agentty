//! Monitoring → Proxy: what the tabs opened while capture is on talk to (`capture.rs`). A table of
//! connections that can be narrowed by endpoint, method, tab or any text.

use super::Workbench;
use crate::capture::{self, Record};
use crate::i18n::{t, tf};
use crate::text_input::TextInput;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{action_button, icon, IconSize, TypeScale};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, Entity, SharedString, Subscription};
use std::time::Duration;

const ROW_HEIGHT: f32 = 24.;

pub(super) struct ProxyPage {
    pub filter: Entity<TextInput>,
    pub _subscription: Subscription,
    records: Vec<Record>,
    revision: u64,
    /// Endpoint picked from the chips.
    endpoint: Option<String>,
    /// Connection whose details are shown under the table.
    picked: Option<u64>,
    watching: bool,
    error: Option<String>,
    scroll: gpui::UniformListScrollHandle,
    /// Records passing the chips and the filter, newest first. Worked out when what it depends on
    /// changes rather than on every frame: with headers recorded a record carries two of them, and
    /// the list holds thousands.
    visible: Vec<Record>,
    /// What `visible` was worked out from: the capture revision, the filter text, the chip.
    visible_key: Option<(u64, String, Option<String>)>,
    /// Whether the machine's own proxy settings point here, read off disk by the watcher.
    system_on: bool,
    /// A `networksetup` exchange is running; the button waits rather than starting a second one.
    system_busy: bool,
}

impl ProxyPage {
    pub fn new(filter: Entity<TextInput>, subscription: Subscription) -> Self {
        Self {
            filter,
            _subscription: subscription,
            records: Vec::new(),
            revision: 0,
            endpoint: None,
            picked: None,
            watching: false,
            error: None,
            scroll: gpui::UniformListScrollHandle::new(),
            visible: Vec::new(),
            visible_key: None,
            system_on: false,
            system_busy: false,
        }
    }
}

fn bytes(value: u64) -> String {
    match value {
        v if v >= 1 << 30 => format!("{:.1} GB", v as f64 / (1u64 << 30) as f64),
        v if v >= 1 << 20 => format!("{:.1} MB", v as f64 / (1u64 << 20) as f64),
        v if v >= 1 << 10 => format!("{:.1} KB", v as f64 / 1024.),
        v => format!("{v} B"),
    }
}

fn duration(ms: u64) -> String {
    match ms {
        ms if ms >= 60_000 => format!("{}m {:02}s", ms / 60_000, ms % 60_000 / 1000),
        ms if ms >= 1000 => format!("{:.1}s", ms as f64 / 1000.),
        ms => format!("{ms}ms"),
    }
}

fn clock(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|at| at.with_timezone(&chrono::Local).format("%H:%M:%S").to_string())
        .unwrap_or_default()
}

/// Whether `record` passes the text filter: every word must be found in the endpoint, the path, the
/// method, the status or the tab's name.
fn matches(record: &Record, tab: &str, query: &str) -> bool {
    let haystack = format!(
        "{} {} {} {} {}",
        record.endpoint(),
        record.path,
        record.method,
        record.status.map(|s| s.to_string()).unwrap_or_default(),
        tab
    )
    .to_lowercase();
    query.split_whitespace().all(|word| haystack.contains(&word.to_lowercase()))
}

impl Workbench {
    /// Keeps the table current while the page is open; ends when it closes.
    fn watch_capture(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.proxy.watching, true) {
            return;
        }
        cx.spawn(async move |this, cx| loop {
            let open = this.update(cx, |this, cx| {
                if this.page != Some(super::Page::Proxy) {
                    this.proxy.watching = false;
                    return false;
                }
                let revision = capture::revision();
                if revision != this.proxy.revision {
                    this.proxy.revision = revision;
                    this.proxy.records = capture::records();
                    cx.notify();
                }
                // Whether the machine points here is a file on disk; the bar asks the watcher for
                // it rather than reading it on every frame.
                let system_on = crate::platform::system_proxy::saved_previous().is_some();
                if system_on != this.proxy.system_on {
                    this.proxy.system_on = system_on;
                    cx.notify();
                }
                true
            });
            if !matches!(open, Ok(true)) {
                break;
            }
            cx.background_executor().timer(Duration::from_millis(500)).await;
        })
        .detach();
    }

    fn pane_title(&self, pane_id: Option<u64>, cx: &gpui::App) -> String {
        let Some(id) = pane_id else { return String::new() };
        self.all_panes().iter().find(|p| p.read(cx).pane_id == id).map(|p| p.read(cx).display_title()).unwrap_or_else(|| format!("#{id}"))
    }

    /// Brings `proxy.visible` up to date: the records that pass the endpoint chip and the text
    /// filter, newest first. It is rebuilt only when the capture, the filter or the chip changed —
    /// a record can carry two recorded heads, so filtering the whole list on every frame moved
    /// megabytes for nothing.
    fn refresh_visible(&mut self, cx: &gpui::App) {
        let query = self.proxy.filter.read(cx).text().trim().to_string();
        let key = (self.proxy.revision, query, self.proxy.endpoint.clone());
        if self.proxy.visible_key.as_ref() == Some(&key) {
            return;
        }
        let (_, query, endpoint) = &key;
        let titles: Vec<(Option<u64>, String)> = self.proxy.records.iter().map(|r| (r.pane, self.pane_title(r.pane, cx))).collect();
        self.proxy.visible = self
            .proxy
            .records
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, r)| endpoint.as_ref().is_none_or(|e| &r.endpoint() == e))
            .filter(|(i, r)| query.is_empty() || matches(r, &titles[*i].1, query))
            .map(|(_, r)| r.clone())
            .collect();
        self.proxy.visible_key = Some(key);
    }

    pub(super) fn render_proxy_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.watch_capture(cx);
        self.refresh_visible(cx);
        let recording = capture::is_recording();
        let port = capture::port();
        let shown = self.proxy.visible.len();
        let (sent, received): (u64, u64) = self.proxy.visible.iter().fold((0, 0), |acc, r| (acc.0 + r.sent, acc.1 + r.received));
        let total = self.proxy.records.len();

        let toggle = div()
            .id("proxy-toggle")
            .flex_shrink_0()
            .h(px(28.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .rounded_md()
            .cursor_pointer()
            .t_body()
            .text_color(hex(Chrome::BRIGHT))
            .bg(if recording { hex_alpha(Chrome::ERROR, 0.35) } else { hex(Chrome::ACCENT) })
            .hover(|s| s.opacity(0.85))
            .child(div().size(px(8.)).rounded_full().bg(hex(if recording { Chrome::ERROR } else { Chrome::BRIGHT })))
            .child(t(cx, if recording { "proxy.stop" } else { "proxy.start" }))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if capture::is_recording() {
                    capture::stop();
                } else {
                    crate::metrics::track(cx, "feature_used", serde_json::json!({ "feature": "proxy_capture" }));
                    this.proxy.error = capture::start().err().map(|err| err.to_string());
                }
                cx.notify();
            }));

        // Off by default, and it never touches HTTPS: only the heads of plain requests are kept.
        let recording_heads = capture::records_heads();
        let heads = div()
            .id("proxy-heads")
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .h(px(28.))
            .px_3()
            .flex()
            .items_center()
            .gap_1p5()
            .rounded_md()
            .cursor_pointer()
            .t_small()
            .border_1()
            .border_color(hex(if recording_heads { Chrome::ACCENT } else { Chrome::BORDER }))
            .text_color(hex(if recording_heads { Chrome::BRIGHT } else { Chrome::MUTED }))
            .tooltip(crate::ui::Tooltip::text(t(cx, "proxy.record_heads_hint"), None))
            .hover(|s| s.border_color(hex(Chrome::ACCENT)))
            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                capture::set_record_heads(!capture::records_heads());
                cx.notify();
            }))
            .child(icon(if recording_heads { "eye" } else { "list" }, IconSize::INLINE, hex(Chrome::MUTED)))
            .child(t(cx, "proxy.record_heads"));

        // The whole machine, not just Agentty's tabs. It changes the system's own proxy settings,
        // so it says what it does, needs the administrator password, and puts them back when off.
        // Read by the watcher, not here: this runs on every frame and that reads a file.
        let system_on = self.proxy.system_on;
        let system_busy = self.proxy.system_busy;
        let system = crate::platform::system_proxy::supported().then(|| {
            div()
                .id("proxy-system")
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .h(px(28.))
                .px_3()
                .flex()
                .items_center()
                .gap_1p5()
                .rounded_md()
                .cursor_pointer()
                .t_small()
                .border_1()
                .border_color(hex(if system_on { Chrome::WARNING } else { Chrome::BORDER }))
                .text_color(hex(if system_on { Chrome::BRIGHT } else { Chrome::MUTED }))
                .tooltip(crate::ui::Tooltip::text(t(cx, "proxy.system_hint"), None))
                .hover(|s| s.border_color(hex(Chrome::WARNING)))
                .when(system_busy, |d| d.opacity(0.6))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_system_capture(cx)))
                .child(icon("globe", IconSize::INLINE, hex(if system_on { Chrome::WARNING } else { Chrome::MUTED })))
                .child(if system_busy {
                    t(cx, "proxy.system_working").to_string()
                } else {
                    t(cx, if system_on { "proxy.system_off" } else { "proxy.system_on" }).to_string()
                })
        });

        let state = match (recording, port) {
            (true, Some(port)) => tf(cx, "proxy.state_on", &[("port", &port.to_string())]),
            (false, Some(_)) => t(cx, "proxy.state_paused").to_string(),
            _ => t(cx, "proxy.state_off").to_string(),
        };
        // When the row doesn't fit (a narrow window, a long translation) the state gives way first, then the
        // buttons' labels are cut short; the start/stop button stays whole. Nothing changes while it fits.
        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().t_heading().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(t(cx, "page.proxy")))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .t_small()
                    .text_color(hex(if recording { Chrome::SUCCESS } else { Chrome::MUTED }))
                    .child(state),
            )
            .child(
                action_button(
                    "proxy-new-tab",
                    t(cx, "proxy.new_tab"),
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        // A tab gets the proxy when it starts: make sure capture is on before it does.
                        if !capture::is_recording() {
                            this.proxy.error = capture::start().err().map(|err| err.to_string());
                        }
                        this.request_launch(crate::launch::PaneKind::Shell, super::LaunchTarget::NewTab, window, cx);
                    }),
                )
                .flex_shrink()
                .min_w_0()
                .truncate(),
            )
            // Panes that were already open keep the environment they started with; typing the
            // variables into their shell is the one way to route them without restarting them.
            .child(
                action_button(
                    "proxy-apply-open",
                    t(cx, "proxy.apply_open"),
                    cx.listener(|this, _: &ClickEvent, _, cx| this.apply_capture_to_open_panes(cx)),
                )
                .flex_shrink()
                .min_w_0()
                .truncate(),
            )
            .child(
                action_button(
                    "proxy-clear",
                    t(cx, "proxy.clear"),
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        capture::clear();
                        this.proxy.endpoint = None;
                        cx.notify();
                    }),
                )
                .flex_shrink()
                .min_w_0()
                .truncate(),
            )
            .child(heads)
            .children(system)
            .child(toggle);

        let hint = div()
            .flex()
            .items_start()
            .gap_2()
            .p_2()
            .rounded_md()
            .bg(hex(Chrome::PANEL))
            .t_small()
            .text_color(hex(Chrome::MUTED))
            .child(icon("info", IconSize::INLINE, hex(Chrome::MUTED)))
            .child(div().flex_1().min_w_0().child(t(cx, "proxy.hint")));

        // Endpoints by how often they were reached; a click narrows the table to one.
        let mut counts: Vec<(String, usize)> = Vec::new();
        for record in &self.proxy.records {
            let endpoint = record.endpoint();
            match counts.iter_mut().find(|(e, _)| *e == endpoint) {
                Some((_, n)) => *n += 1,
                None => counts.push((endpoint, 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        // One row (GPUI sizes wrapped rows badly inside a column); the busiest endpoints come first.
        let mut chips = div().h(px(26.)).flex_shrink_0().flex().items_center().gap_1().overflow_hidden();
        for (index, (endpoint, count)) in counts.into_iter().take(14).enumerate() {
            let active = self.proxy.endpoint.as_ref() == Some(&endpoint);
            let pick = endpoint.clone();
            chips = chips.child(
                div()
                    .id(("proxy-endpoint", index))
                    .flex_shrink_0()
                    .h(px(24.))
                    .px_2()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .cursor_pointer()
                    .t_small()
                    .border_1()
                    .border_color(hex(if active { Chrome::ACCENT } else { Chrome::BORDER }))
                    .when(active, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.3)).text_color(hex(Chrome::BRIGHT)))
                    .hover(|s| s.border_color(hex(Chrome::ACCENT)))
                    .child(endpoint)
                    .child(div().t_caption().text_color(hex(Chrome::MUTED)).child(count.to_string()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.proxy.endpoint = if this.proxy.endpoint.as_ref() == Some(&pick) { None } else { Some(pick.clone()) };
                        cx.notify();
                    })),
            );
        }

        let filter = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .h(px(28.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .border_1()
                    .border_color(hex(Chrome::BORDER))
                    .bg(hex(Chrome::PANEL))
                    .child(icon("search", IconSize::INLINE, hex(Chrome::MUTED)))
                    .child(div().flex_1().min_w_0().child(self.proxy.filter.clone())),
            )
            .child(div().flex_shrink_0().t_small().text_color(hex(Chrome::MUTED)).child(tf(
                cx,
                "proxy.count",
                &[("shown", &shown.to_string()), ("total", &total.to_string())],
            )));

        let columns = div()
            .h(px(ROW_HEIGHT))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .t_caption()
            .font_weight(crate::theme::EMPHASIS)
            .text_color(hex(Chrome::MUTED))
            .child(div().w(px(64.)).flex_shrink_0().child(t(cx, "proxy.col_time")))
            .child(div().w(px(130.)).min_w(px(48.)).truncate().child(t(cx, "proxy.col_tab")))
            .child(div().w(px(70.)).flex_shrink_0().truncate().child(t(cx, "proxy.col_method")))
            .child(div().flex_1().min_w(px(72.)).truncate().child(t(cx, "proxy.col_endpoint")))
            .child(div().w(px(52.)).flex_shrink_0().child(t(cx, "proxy.col_status")))
            .child(div().w(px(76.)).flex_shrink_0().text_right().child(format!("↑ {}", bytes(sent))))
            .child(div().w(px(76.)).flex_shrink_0().text_right().child(format!("↓ {}", bytes(received))))
            .child(div().w(px(70.)).flex_shrink_0().text_right().child(t(cx, "proxy.col_time_taken")));

        let table: AnyElement = if shown == 0 {
            let text = if total > 0 {
                t(cx, "proxy.no_match")
            } else if recording {
                t(cx, "proxy.waiting")
            } else {
                t(cx, "proxy.empty")
            };
            div().flex_1().p_6().t_body().text_color(hex(Chrome::MUTED)).child(text).into_any_element()
        } else {
            let handle = self.proxy.scroll.clone();
            let base = handle.0.borrow().base_handle.clone();
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    gpui::uniform_list(
                        "proxy-rows",
                        shown,
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            this.refresh_visible(cx);
                            // Only the rows actually on screen are copied out, so the borrow ends
                            // before they are rendered.
                            let rows: Vec<(usize, Record)> =
                                range.filter_map(|i| this.proxy.visible.get(i).map(|r| (i, r.clone()))).collect();
                            rows.into_iter().map(|(i, record)| this.render_proxy_row(i, &record, cx)).collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(handle)
                    .size_full(),
                )
                .group(crate::ui::SCROLL_GROUP)
                .child(crate::ui::scrollbar(base))
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_6()
            .child(header)
            .child(hint)
            .when_some(self.proxy.error.clone(), |d, error| d.child(div().t_small().text_color(hex(Chrome::ERROR)).child(error)))
            .child(chips)
            .child(filter)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .rounded_md()
                    .border_1()
                    .border_color(hex(Chrome::BORDER))
                    .child(columns)
                    .child(table)
                    .children(self.render_proxy_detail(cx)),
            )
    }

    /// Sends the machine's own traffic through the capture proxy, or puts the settings back.
    ///
    /// `networksetup` is a subprocess per network service and takes a moment each; the whole
    /// exchange runs in the background, or the window would stand still for seconds on one click.
    pub(super) fn toggle_system_capture(&mut self, cx: &mut Context<Self>) {
        use crate::platform::system_proxy;
        if self.proxy.system_busy {
            return;
        }
        let previous = system_proxy::saved_previous();
        if previous.is_none() {
            if !capture::is_recording() {
                self.proxy.error = capture::start().err().map(|err| err.to_string());
            }
            if capture::port().is_none() {
                return cx.notify();
            }
        }
        let port = capture::port();
        self.proxy.system_busy = true;
        self.proxy.error = None;
        // Stop serving untokened connections before the settings go back, so nothing else on the
        // machine is listed once it is no longer pointed here.
        if previous.is_some() {
            capture::set_allow_system(false);
        }
        let work = cx.background_spawn(async move {
            match previous {
                Some(previous) => system_proxy::restore(&previous).map(|()| false),
                None => system_proxy::enable(port.expect("checked above")).map(|_| true),
            }
        });
        cx.spawn(async move |this, cx| {
            let outcome = work.await;
            let _ = this.update(cx, |this, cx| {
                this.proxy.system_busy = false;
                match outcome {
                    Ok(enabled) => {
                        // Only now: until the machine is actually pointed here, the token is the
                        // only way in.
                        capture::set_allow_system(enabled);
                        this.proxy.system_on = enabled;
                        let message = t(cx, if enabled { "proxy.system_applied" } else { "proxy.system_restored" });
                        this.show_toast(message, cx);
                    }
                    Err(err) => this.proxy.error = Some(format!("{err}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Types the proxy variables into every shell pane that is open, so terminals started before
    /// capture was turned on are routed too. Agent panes are left alone: text typed into an agent
    /// is a prompt, not a shell command.
    pub(super) fn apply_capture_to_open_panes(&mut self, cx: &mut Context<Self>) {
        if !capture::is_recording() {
            self.proxy.error = capture::start().err().map(|err| err.to_string());
        }
        let mut applied = 0;
        let mut skipped = 0;
        for pane in self.all_panes() {
            let (pane_id, is_agent) = {
                let view = pane.read(cx);
                (view.pane_id, view.is_agent() || view.live_tool.is_some())
            };
            if is_agent {
                skipped += 1;
                continue;
            }
            let exports: String = capture::pane_environment(pane_id)
                .into_iter()
                .map(|(name, value)| format!("export {name}={}", crate::launch::shell_quote(&value)))
                .collect::<Vec<_>>()
                .join("; ");
            if exports.is_empty() {
                continue;
            }
            pane.update(cx, |view, cx| view.submit_prompt(exports, cx));
            applied += 1;
        }
        let message = tf(cx, "proxy.applied", &[("n", &applied.to_string()), ("skipped", &skipped.to_string())]);
        self.show_toast(message, cx);
        cx.notify();
    }

    /// Everything known about the picked connection. An HTTPS tunnel has only its outside: the
    /// bytes inside it are never read, so there is no request or response text to show.
    fn render_proxy_detail(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picked = self.proxy.picked?;
        let record = self.proxy.records.iter().find(|r| r.id == picked)?;
        let row = |label: String, value: String| {
            div()
                .flex()
                .gap_2()
                .px_3()
                .py_0p5()
                .t_small()
                .child(div().w(px(120.)).flex_shrink_0().text_color(hex(Chrome::MUTED)).child(label))
                .child(div().flex_1().min_w_0().font_family("JetBrains Mono").text_color(hex(Chrome::BRIGHT)).child(value))
        };
        let block = |title: String, text: String| {
            div().flex().flex_col().px_3().py_1().child(div().t_caption().text_color(hex(Chrome::MUTED)).child(title)).child(
                div()
                    .mt_0p5()
                    .p_2()
                    .rounded_md()
                    .bg(hex(Chrome::PANEL))
                    .t_small()
                    .font_family("JetBrains Mono")
                    .text_color(hex(Chrome::FOREGROUND))
                    .child(text),
            )
        };
        let mut detail = div()
            .id("proxy-detail")
            .flex_shrink_0()
            .max_h(px(280.))
            .overflow_y_scroll()
            .border_t_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::EDITOR))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .child(div().flex_1().t_body().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(format!(
                        "{} {}{}",
                        record.method,
                        record.endpoint(),
                        record.path
                    )))
                    .child(crate::ui::icon_only(
                        "proxy-detail-close",
                        "x",
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.proxy.picked = None;
                            cx.notify();
                        }),
                    )),
            )
            .child(row(t(cx, "proxy.col_time").to_string(), clock(record.started_ms)))
            .child(row(t(cx, "proxy.col_tab").to_string(), self.pane_title(record.pane, cx)))
            .child(row(t(cx, "proxy.col_host").to_string(), format!("{}:{}", record.host, record.port)))
            .child(row(t(cx, "proxy.col_status").to_string(), record.status.map(|s| s.to_string()).unwrap_or_else(|| "—".into())))
            .child(row(t(cx, "proxy.col_sent").to_string(), bytes(record.sent)))
            .child(row(t(cx, "proxy.col_received").to_string(), bytes(record.received)))
            .child(row(
                t(cx, "proxy.col_time_taken").to_string(),
                record.duration_ms.map(duration).unwrap_or_else(|| t(cx, "proxy.open").to_string()),
            ));
        if let Some(error) = &record.error {
            detail = detail.child(row(t(cx, "proxy.failed").to_string(), error.clone()));
        }
        match (&record.request_head, &record.response_head) {
            (None, None) if record.method == "CONNECT" => {
                detail = detail.child(div().px_3().py_2().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "proxy.tunnel_only")));
            }
            (request, response) => {
                if let Some(text) = request {
                    detail = detail.child(block(t(cx, "proxy.request_head").to_string(), text.clone()));
                }
                if let Some(text) = response {
                    detail = detail.child(block(t(cx, "proxy.response_head").to_string(), text.clone()));
                }
                if request.is_none() && response.is_none() {
                    detail = detail.child(div().px_3().py_2().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "proxy.no_heads")));
                }
            }
        }
        Some(detail.into_any_element())
    }

    fn render_proxy_row(&self, index: usize, record: &Record, cx: &mut Context<Self>) -> AnyElement {
        let open = record.duration_ms.is_none();
        let status_color = match (record.status, &record.error) {
            (_, Some(_)) => Chrome::ERROR,
            (Some(code), _) if code >= 400 => Chrome::ERROR,
            (Some(code), _) if code >= 300 => Chrome::WARNING,
            (Some(_), _) => Chrome::SUCCESS,
            (None, _) => Chrome::MUTED,
        };
        let status: SharedString = match (&record.error, record.status, open) {
            (Some(_), _, _) => t(cx, "proxy.failed").into(),
            (_, Some(code), _) => code.to_string().into(),
            (_, None, true) => t(cx, "proxy.open").into(),
            (_, None, false) => "—".into(),
        };
        let method_color = if record.method == "CONNECT" { Chrome::PURPLE } else { Chrome::BLUE };
        let endpoint = record.endpoint();
        let pick = endpoint.clone();
        let id = record.id;
        let picked = self.proxy.picked == Some(id);
        div()
            .id(("proxy-row", index))
            .w_full()
            .h(px(ROW_HEIGHT))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .t_small()
            .cursor_pointer()
            .font_family("JetBrains Mono")
            .when(index % 2 == 1, |d| d.bg(hex_alpha(0xffffff, 0.025)))
            .when(picked, |d| d.bg(hex_alpha(Chrome::ACCENT, 0.35)))
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.proxy.picked = if this.proxy.picked == Some(id) { None } else { Some(id) };
                cx.notify();
            }))
            .when_some(record.error.clone(), |d, error| d.tooltip(crate::ui::Tooltip::text(error, None)))
            .child(div().w(px(64.)).flex_shrink_0().text_color(hex(Chrome::MUTED)).child(clock(record.started_ms)))
            .child(div().w(px(130.)).min_w(px(48.)).truncate().text_color(hex(Chrome::FOREGROUND)).child(self.pane_title(record.pane, cx)))
            .child(div().w(px(70.)).flex_shrink_0().text_color(hex(method_color)).child(record.method.clone()))
            .child(
                div()
                    .id(("proxy-row-endpoint", index))
                    .flex_1()
                    .min_w(px(72.))
                    .flex()
                    .gap_1()
                    .cursor_pointer()
                    // Filters by this endpoint; picking the row for its details is the rest of it.
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.proxy.endpoint = Some(pick.clone());
                        cx.notify();
                    }))
                    .child(div().flex_shrink_0().max_w(gpui::relative(0.7)).truncate().text_color(hex(Chrome::BRIGHT)).child(endpoint))
                    .child(div().min_w_0().truncate().text_color(hex(Chrome::MUTED)).child(record.path.clone())),
            )
            .child(div().w(px(52.)).flex_shrink_0().text_color(hex(status_color)).child(status))
            .child(div().w(px(76.)).flex_shrink_0().text_right().text_color(hex(Chrome::FOREGROUND)).child(bytes(record.sent)))
            .child(div().w(px(76.)).flex_shrink_0().text_right().text_color(hex(Chrome::FOREGROUND)).child(bytes(record.received)))
            .child(
                div()
                    .w(px(70.))
                    .flex_shrink_0()
                    .text_right()
                    .text_color(hex(if open { Chrome::ATTENTION } else { Chrome::MUTED }))
                    .child(record.duration_ms.map(duration).unwrap_or_else(|| "…".into())),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(method: &str, host: &str, port: u16, path: &str, status: Option<u16>) -> Record {
        Record {
            id: 1,
            started_ms: 0,
            pane: Some(1),
            method: method.into(),
            host: host.into(),
            port,
            path: path.into(),
            status,
            sent: 0,
            received: 0,
            duration_ms: Some(1),
            error: None,
            request_head: None,
            response_head: None,
        }
    }

    #[test]
    fn filters_by_endpoint_method_status_and_tab() {
        let tunnel = record("CONNECT", "api.example.com", 443, "", None);
        let plain = record("GET", "registry.example.org", 8080, "/pkg/left-pad", Some(404));
        assert_eq!(tunnel.endpoint(), "api.example.com");
        assert_eq!(plain.endpoint(), "registry.example.org:8080");
        assert!(matches(&tunnel, "Claude Code", "api.example"));
        assert!(matches(&tunnel, "Claude Code", "connect claude"));
        assert!(!matches(&tunnel, "Claude Code", "registry"));
        assert!(matches(&plain, "Terminal", "404 left-pad"));
        assert!(matches(&plain, "Terminal", "GET :8080"));
        assert!(!matches(&plain, "Terminal", "post"));
    }

    #[test]
    fn formats_sizes_and_times() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(2048), "2.0 KB");
        assert_eq!(bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(duration(250), "250ms");
        assert_eq!(duration(1500), "1.5s");
        assert_eq!(duration(125_000), "2m 05s");
    }
}
