//! Docker panel docked at the right edge, for projects that run their databases and caches in
//! containers: the compose services of the project the active pane works in (or, without compose,
//! its containers) with their state and ports, and start / stop / restart / `up -d` / `down`.
//!
//! It stays out of the way: a small chip in the status bar appears only for a project with a
//! compose file, a Dockerfile or containers of its own, and opens the panel. Docker is asked once
//! when the project changes (and when the window comes back to the front); only while the panel is
//! open does it look again every few seconds — always off the UI thread (`agentty_bridge::docker`).

use super::Workbench;
use crate::i18n::{t, tf};
use crate::launch::LaunchSpec;
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, icon_only_sized, tilde, IconSize, Tooltip, TypeScale};
use agentty_bridge::docker::{self, Action, Compose, Engine, Overview, Service, State};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, SharedString, Window};
use std::path::PathBuf;
use std::time::Duration;

const REFRESH_EVERY: Duration = Duration::from_secs(4);
const INSTALL_URL: &str = "https://docs.docker.com/get-started/get-docker/";

#[derive(Default)]
pub(super) struct DockerState {
    /// Active pane and its folder when last looked, to notice a tab switch or a `cd`.
    seen: Option<(gpui::EntityId, PathBuf)>,
    /// The project (working tree) Docker was asked about, and what it said.
    root: Option<PathBuf>,
    overview: Option<Overview>,
    loading: bool,
    generation: u64,
    /// Ask again at the next render (the window came to the front, an action finished).
    stale: bool,
    pub(super) open: bool,
    /// Ends the refresh loop of an earlier opening.
    open_generation: u64,
    /// The command running right now; the buttons wait until it is done.
    busy: Option<String>,
    error: Option<String>,
    /// `down` waits for a second, explicit confirmation.
    confirm_down: bool,
}

impl DockerState {
    /// For the debug driver's `probe`.
    pub(super) fn debug_state(&self) -> serde_json::Value {
        let overview = self.overview.as_ref();
        serde_json::json!({
            "open": self.open,
            "root": self.root,
            "relevant": overview.is_some_and(Overview::relevant),
            "engine": overview.map(|o| format!("{:?}", o.engine)),
            "compose": overview.and_then(|o| o.compose.as_ref()).map(|c| serde_json::json!({ "dir": c.dir, "project": c.project })),
            "services": overview.map(|o| o.services.iter().map(|s| serde_json::json!({ "name": s.name, "image": s.image, "state": format!("{:?}", s.state), "ports": s.ports })).collect::<Vec<_>>()),
            "busy": self.busy,
            "error": self.error,
            "confirm_down": self.confirm_down,
        })
    }
}

fn state_label(state: State) -> &'static str {
    match state {
        State::Running => "docker.state.running",
        State::Starting => "docker.state.starting",
        State::Unhealthy => "docker.state.unhealthy",
        State::Paused => "docker.state.paused",
        State::Stopped => "docker.state.stopped",
        State::NotCreated => "docker.state.not_created",
    }
}

fn state_color(state: State) -> u32 {
    match state {
        State::Running => Chrome::SUCCESS,
        State::Starting => Chrome::BLUE,
        State::Unhealthy => Chrome::ERROR,
        State::Paused => Chrome::WARNING,
        State::Stopped | State::NotCreated => Chrome::MUTED,
    }
}

/// An argv as one line for the pane's shell. Names in it passed `docker::valid_name`; paths (`-f`)
/// are quoted for the shell the pane runs.
fn command_line(argv: &[String]) -> String {
    let safe = |arg: &str| !arg.is_empty() && arg.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c));
    argv.iter()
        .map(|arg| {
            if safe(arg) {
                arg.clone()
            } else if cfg!(windows) {
                crate::launch::powershell_quote(arg)
            } else {
                crate::launch::shell_quote(arg)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl Workbench {
    pub(super) fn toggle_docker_panel(&mut self, cx: &mut Context<Self>) {
        if self.docker.open {
            self.docker.open = false;
            self.docker.confirm_down = false;
            return cx.notify();
        }
        self.docker.open = true;
        self.docker.open_generation += 1;
        let generation = self.docker.open_generation;
        self.page = None;
        // Looks again while it is open; ends with this opening of the panel.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(REFRESH_EVERY).await;
            match this.update(cx, |this, cx| {
                let open = this.docker.open && this.docker.open_generation == generation;
                if open && this.page.is_none() {
                    this.refresh_docker(cx);
                }
                open
            }) {
                Ok(true) => {}
                _ => break,
            }
        })
        .detach();
        self.refresh_docker(cx);
        cx.notify();
    }

    /// Follows the active pane's project, and asks Docker again when told to. Called on render.
    pub(super) fn prepare_docker(&mut self, cx: &mut Context<Self>) {
        let seen = self.active_pane().map(|pane| (pane.entity_id(), pane.read(cx).display_cwd()));
        if seen != self.docker.seen {
            self.docker.seen = seen;
            let root = self.active_tree(cx);
            if root != self.docker.root {
                self.docker.root = root;
                self.docker.overview = None;
                self.docker.error = None;
                self.docker.confirm_down = false;
                self.docker.stale = true;
            }
        }
        if self.docker.stale {
            self.refresh_docker(cx);
        }
    }

    /// The window came back to the front: containers may have been started in another terminal.
    pub(super) fn docker_window_activated(&mut self) {
        if self.docker.root.is_some() {
            self.docker.stale = true;
        }
    }

    fn refresh_docker(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.docker;
        if state.loading {
            return;
        }
        let Some(root) = state.root.clone() else {
            state.stale = false;
            return;
        };
        state.stale = false;
        state.loading = true;
        state.generation += 1;
        let generation = state.generation;
        cx.spawn(async move |this, cx| {
            let overview = cx.background_spawn(async move { docker::overview(&root) }).await;
            let _ = this.update(cx, |this, cx| {
                let state = &mut this.docker;
                if state.generation != generation {
                    return;
                }
                state.loading = false;
                // The project changed while Docker was being asked: ask about the new one.
                if state.root.as_ref() != Some(&overview.root) {
                    state.stale = true;
                    return cx.notify();
                }
                if state.overview.as_ref() != Some(&overview) {
                    state.overview = Some(overview);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Runs `action` on the whole compose project (`service: None`) or on one service / container.
    fn docker_action(&mut self, action: Action, service: Option<Service>, cx: &mut Context<Self>) {
        let Some(overview) = self.docker.overview.clone() else { return };
        if self.docker.busy.is_some() {
            return;
        }
        if action == Action::Down && !self.docker.confirm_down {
            self.docker.confirm_down = true;
            return cx.notify();
        }
        self.docker.confirm_down = false;
        let compose = overview.compose.clone();
        let args = match (&compose, &service) {
            (Some(compose), service) => {
                // A service without a container has nothing to start: `up -d` creates it.
                let action = if action == Action::Start && service.as_ref().is_some_and(|s| s.state == State::NotCreated) {
                    Action::Up
                } else {
                    action
                };
                docker::compose_args(compose, action, service.as_ref().map(|s| s.name.as_str()))
            }
            (None, Some(container)) => match &container.id {
                Some(id) => docker::container_args(id, action),
                None => return,
            },
            (None, None) => return,
        };
        let args = match args {
            Ok(args) => args,
            Err(err) => {
                self.docker.error = Some(err.to_string());
                return cx.notify();
            }
        };
        let dir = compose.as_ref().map_or(overview.root.clone(), |c| c.dir.clone());
        // Without `-p` / `-f …`: long paths would hide what matters, the action and the service.
        let prefix = compose.as_ref().map_or(0, |c| c.args().len());
        let words: Vec<String> = args.iter().skip(prefix).map(|a| a.to_string_lossy().into_owned()).collect();
        let shown = format!("docker {}{}", if compose.is_some() { "compose " } else { "" }, words.join(" "));
        self.docker.busy = Some(shown);
        self.docker.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { docker::execute(&args, &dir) }).await;
            let _ = this.update(cx, |this, cx| {
                this.docker.busy = None;
                this.docker.error = result.err().map(|err| err.to_string());
                this.docker.stale = true;
                cx.notify();
            });
        })
        .detach();
    }

    /// The debug driver's `docker` command: `` (toggle the panel), `refresh`, `up`, `down` (twice to
    /// confirm, like the button), `start:<service>`, `stop:<service>`, `restart:<service>`, `logs:<service>`.
    pub(super) fn debug_docker(&mut self, argument: &str, window: &mut Window, cx: &mut Context<Self>) {
        let service = |this: &Self, name: &str| this.docker.overview.as_ref()?.services.iter().find(|s| s.name == name).cloned();
        match argument.split_once(':') {
            None if argument == "refresh" => self.refresh_docker(cx),
            None if argument == "up" => self.docker_action(Action::Up, None, cx),
            None if argument == "down" => self.docker_action(Action::Down, None, cx),
            None => self.toggle_docker_panel(cx),
            Some(("logs", name)) => self.open_docker_logs(name.to_string(), window, cx),
            Some((verb, name)) => {
                let action = match verb {
                    "start" => Action::Start,
                    "stop" => Action::Stop,
                    "restart" => Action::Restart,
                    _ => return,
                };
                if let Some(service) = service(self, name) {
                    self.docker_action(action, Some(service), cx);
                }
            }
        }
    }

    /// Follows a service's (or container's) logs in a new terminal tab.
    fn open_docker_logs(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(overview) = self.docker.overview.as_ref() else { return };
        let compose: Option<&Compose> = overview.compose.as_ref();
        let dir = compose.map_or(overview.root.clone(), |c| c.dir.clone());
        match docker::logs_argv(compose, &name) {
            Ok(argv) => {
                let spec = LaunchSpec::shell_command(command_line(&argv), format!("logs · {name}"), dir);
                self.open_tab(spec, window, cx);
            }
            Err(err) => {
                self.docker.error = Some(err.to_string());
                cx.notify();
            }
        }
    }

    /// "🐳 3 running" in the status bar, for a project that uses Docker. Opens the panel.
    pub(super) fn render_docker_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let overview = self.docker.overview.as_ref().filter(|o| o.relevant())?;
        let unhealthy = overview.services.iter().any(|s| s.state == State::Unhealthy);
        let running = overview.running();
        let (label, color) = match &overview.engine {
            Engine::NoCli => (t(cx, "docker.chip_no_cli").to_string(), Chrome::MUTED),
            Engine::NotRunning(_) => (t(cx, "docker.chip_off").to_string(), Chrome::WARNING),
            Engine::Ready if unhealthy => (tf(cx, "docker.chip_running", &[("n", &running.to_string())]), Chrome::ERROR),
            Engine::Ready if running > 0 => (tf(cx, "docker.chip_running", &[("n", &running.to_string())]), Chrome::BLUE),
            Engine::Ready => (tf(cx, "docker.chip_running", &[("n", "0")]), Chrome::MUTED),
        };
        let open = self.docker.open;
        Some(
            div()
                .id("status-docker")
                .tooltip(Tooltip::text(t(cx, "docker.tooltip"), None))
                .h_full()
                .px_1p5()
                .flex()
                .items_center()
                .gap_1()
                .cursor_pointer()
                .when(open, |d| d.bg(hex(Chrome::SELECTED)))
                .hover(|s| s.bg(hex(Chrome::HOVER)))
                .child(icon("container", 13., hex(color)))
                .child(div().text_color(hex(if running > 0 || open { Chrome::FOREGROUND } else { Chrome::MUTED })).child(label))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_docker_panel(cx)))
                .into_any_element(),
        )
    }

    pub(super) fn render_docker_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.docker.open {
            return None;
        }
        let width = self.docker_panel_width(cx);
        let state = &self.docker;
        let overview = state.overview.as_ref();
        let project = state.root.as_ref().and_then(|r| r.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let header = div()
            .h(px(36.))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(hex(Chrome::BORDER))
            .bg(hex(Chrome::SIDE_BAR))
            .child(icon("container", IconSize::BUTTON, hex(Chrome::BLUE)))
            .child(div().t_body().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(t(cx, "docker.title")))
            .child(
                div()
                    .id("docker-project")
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .t_small()
                    .text_color(hex(Chrome::MUTED))
                    .when_some(state.root.clone(), |d, root| d.tooltip(Tooltip::text(tilde(&root), None)))
                    .child(project),
            )
            .child(
                crate::ui::icon_only("docker-refresh", "refresh-cw", cx.listener(|this, _: &ClickEvent, _, cx| this.refresh_docker(cx)))
                    .tooltip(Tooltip::text(t(cx, "usage.refresh"), None)),
            )
            .child(crate::ui::icon_only("docker-close", "x", cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_docker_panel(cx))));

        let note = |text: String| div().px_3().py_2().t_small().text_color(hex(Chrome::MUTED)).child(text);
        let body: AnyElement = match overview.map(|o| &o.engine) {
            None if state.root.is_none() => note(t(cx, "docker.no_project").to_string()).into_any_element(),
            None => crate::ui::loading_row(t(cx, "usage.loading")).into_any_element(),
            Some(Engine::NoCli) => div()
                .flex()
                .flex_col()
                .child(note(t(cx, "docker.no_cli").to_string()))
                .child(div().px_3().child(crate::ui::action_button(
                    "docker-install",
                    t(cx, "docker.install"),
                    |_: &ClickEvent, _, cx: &mut gpui::App| cx.open_url(INSTALL_URL),
                )))
                .into_any_element(),
            Some(Engine::NotRunning(detail)) => div()
                .flex()
                .flex_col()
                .child(note(t(cx, "docker.not_running").to_string()))
                .child(div().px_3().t_caption().text_color(hex(Chrome::MUTED)).child(detail.clone()))
                .into_any_element(),
            Some(Engine::Ready) => self.render_docker_services(overview.expect("engine came from it"), cx),
        };

        Some(
            div()
                .id("docker-panel")
                .w(px(width))
                .flex_shrink_0()
                .h_full()
                .flex()
                .flex_col()
                .bg(hex(Chrome::PANEL))
                .child(header)
                .child(body)
                .child(div().flex_1())
                .child(
                    div()
                        .flex_shrink_0()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(hex(Chrome::BORDER))
                        .t_caption()
                        .text_color(hex(Chrome::MUTED))
                        .child(t(cx, "docker.privacy_note")),
                )
                .into_any_element(),
        )
    }

    fn render_docker_services(&self, overview: &Overview, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.docker;
        let busy = state.busy.is_some();
        let mut column = div().id("docker-services").flex_1().min_h_0().overflow_y_scroll().flex().flex_col();

        // Compose: the project's folder and the buttons for all of its services.
        if let Some(compose) = &overview.compose {
            let folder = compose.dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| tilde(&compose.dir));
            let file = overview
                .files
                .compose
                .as_ref()
                .filter(|_| compose.dir == overview.root)
                .and_then(|f| f.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or(folder);
            let button =
                |id: &'static str, glyph: &'static str, label: &'static str, tip: &'static str, action: Action, cx: &mut Context<Self>| {
                    div()
                        .id(id)
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .t_small()
                        .bg(hex(0x2d2d30))
                        .text_color(hex(Chrome::FOREGROUND))
                        .when(!busy, |d| {
                            d.cursor_pointer()
                                .hover(|s| s.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::BRIGHT)))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.docker_action(action, None, cx)))
                        })
                        .when(busy, |d| d.opacity(0.5))
                        .tooltip(Tooltip::text(t(cx, tip), None))
                        .child(icon(glyph, 11., hex(Chrome::FOREGROUND)))
                        .child(t(cx, label))
                };
            column = column.child(
                div()
                    .flex_shrink_0()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("docker-compose-file")
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .t_caption()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hex(Chrome::MUTED))
                            .tooltip(Tooltip::text(tilde(&compose.dir), None))
                            .child(t(cx, "docker.compose").to_uppercase())
                            .child(div().min_w_0().truncate().font_weight(FontWeight::NORMAL).child(file)),
                    )
                    .child(button("docker-up", "play", "docker.up", "docker.up_tooltip", Action::Up, cx))
                    .child(button("docker-down", "power", "docker.down", "docker.down_tooltip", Action::Down, cx)),
            );
            if state.confirm_down {
                column = column.child(
                    div()
                        .flex_shrink_0()
                        .mx_2()
                        .my_1()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(hex_alpha(Chrome::ERROR, 0.6))
                        .bg(hex_alpha(Chrome::ERROR, 0.12))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().t_small().text_color(hex(Chrome::BRIGHT)).child(t(cx, "docker.down_confirm")))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    div()
                                        .id("docker-down-confirm")
                                        .px_2()
                                        .py_0p5()
                                        .rounded_sm()
                                        .t_small()
                                        .cursor_pointer()
                                        .bg(hex_alpha(Chrome::ERROR, 0.7))
                                        .text_color(hex(Chrome::BRIGHT))
                                        .hover(|s| s.bg(hex(Chrome::ERROR)))
                                        .child(t(cx, "docker.down"))
                                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.docker_action(Action::Down, None, cx))),
                                )
                                .child(crate::ui::action_button(
                                    "docker-down-cancel",
                                    t(cx, "docker.cancel"),
                                    cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.docker.confirm_down = false;
                                        cx.notify();
                                    }),
                                )),
                        ),
                );
            }
        } else if !overview.services.is_empty() {
            column = column.child(
                div()
                    .flex_shrink_0()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .t_caption()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(hex(Chrome::MUTED))
                    .child(t(cx, "docker.containers").to_uppercase()),
            );
        }
        if let Some(command) = &state.busy {
            column = column.child(crate::ui::loading_row(tf(cx, "docker.busy", &[("command", command)])));
        }
        for error in [state.error.as_ref(), overview.error.as_ref()].into_iter().flatten() {
            column = column.child(div().flex_shrink_0().px_3().py_1().t_small().text_color(hex(Chrome::ERROR)).child(error.clone()));
        }
        if overview.services.is_empty() {
            let key = if overview.compose.is_some() {
                "docker.no_services"
            } else if overview.files.dockerfile {
                "docker.dockerfile_only"
            } else {
                "docker.no_containers"
            };
            return column.child(div().px_3().py_2().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, key))).into_any_element();
        }
        for (index, service) in overview.services.iter().enumerate() {
            column = column.child(self.render_docker_service(index, service, busy, cx));
        }
        column.into_any_element()
    }

    fn render_docker_service(&self, index: usize, service: &Service, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let color = state_color(service.state);
        let has_container = service.id.is_some();
        let button = |id: &'static str, glyph: &'static str, tip: &'static str, cx: &mut Context<Self>| {
            icon_only_sized((id, index), glyph, 22., 12., |_, _, _| {})
                .tooltip(Tooltip::text(t(cx, tip), None))
                .when(busy, |d| d.opacity(0.4))
        };
        let toggle = if service.state.is_up() { Action::Stop } else { Action::Start };
        let (toggle_glyph, toggle_tip) = if toggle == Action::Stop { ("square", "docker.stop") } else { ("play", "docker.start") };
        let (for_toggle, for_restart) = (service.clone(), service.clone());
        let logs_name = service.name.clone();
        let mut actions =
            div().flex().flex_shrink_0().items_center().child(button("docker-toggle", toggle_glyph, toggle_tip, cx).when(!busy, |d| {
                d.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.docker_action(toggle, Some(for_toggle.clone()), cx)))
            }));
        if has_container {
            actions =
                actions
                    .child(button("docker-restart", "rotate-cw", "docker.restart", cx).when(!busy, |d| {
                        d.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.docker_action(Action::Restart, Some(for_restart.clone()), cx)
                        }))
                    }))
                    .child(button("docker-logs", "scroll-text", "docker.logs", cx).on_click(
                        cx.listener(move |this, _: &ClickEvent, window, cx| this.open_docker_logs(logs_name.clone(), window, cx)),
                    ));
        }
        let mut meta = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .t_caption()
            .text_color(hex(Chrome::MUTED))
            .child(div().text_color(hex(color)).child(t(cx, state_label(service.state))));
        if !service.image.is_empty() {
            meta = meta.child(div().max_w(px(150.)).truncate().child(format!("· {}", service.image)));
        }
        for port in &service.ports {
            meta =
                meta.child(div().px_1().rounded_sm().bg(hex_alpha(Chrome::BLUE, 0.15)).text_color(hex(Chrome::BLUE)).child(port.clone()));
        }
        div()
            .id(("docker-service", index))
            .flex_shrink_0()
            .mx_1()
            .px_2()
            .py_1p5()
            .rounded_md()
            .flex()
            .items_center()
            .gap_2()
            .hover(|s| s.bg(hex(Chrome::HOVER)))
            .when(!service.status.is_empty(), |d| d.tooltip(Tooltip::text(service.status.clone(), None)))
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(8.))
                    .rounded_full()
                    .when(service.state == State::NotCreated, |d| d.border_1().border_color(hex(color)))
                    .when(service.state != State::NotCreated, |d| d.bg(hex(color))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .t_small()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(hex(Chrome::BRIGHT))
                            .child(SharedString::from(service.name.clone())),
                    )
                    .child(meta),
            )
            .child(actions)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_commands_quote_only_what_needs_it() {
        let argv: Vec<String> =
            ["docker", "compose", "-f", "/work/my shop/dev.yml", "logs", "-f", "--tail", "200", "db"].map(String::from).to_vec();
        let line = command_line(&argv);
        assert!(line.starts_with("docker compose -f "));
        assert!(line.ends_with(" logs -f --tail 200 db"));
        // Single quotes in both POSIX shells and PowerShell.
        assert!(line.contains("'/work/my shop/dev.yml'"));
    }

    #[test]
    fn every_state_has_a_label_and_color() {
        for state in [State::Running, State::Starting, State::Unhealthy, State::Paused, State::Stopped, State::NotCreated] {
            assert!(crate::i18n::tr(crate::settings::Language::En, state_label(state)) != state_label(state));
            let _ = state_color(state);
        }
    }
}
