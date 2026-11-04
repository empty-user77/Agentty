//! When Tailscale stands in the way of remote access: what exactly is wrong, the one thing that
//! fixes it, and ways to get help that don't ask the user to know anything about Tailscale.
//!
//! The page keeps looking while it is open, so what the user does in Tailscale shows up here by
//! itself. Looking never wakes a VPN the user turned off (`tailscale::run` asks macOS first).

use super::Workbench;
use crate::i18n::t;
use crate::remote::tailscale::{self, Status, Trouble};
use crate::remote::{self, Phase};
use crate::theme::{hex, Chrome};
use crate::ui::{action_button, TypeScale};
use gpui::{div, prelude::*, ClickEvent, Context, Div, Window};
use std::path::PathBuf;
use std::time::Duration;

const TAILSCALE_DOWNLOAD: &str = "https://tailscale.com/download";
const TAILSCALE_MACHINES: &str = "https://login.tailscale.com/admin/machines";
/// System Settings → General → Login Items & Extensions.
const MAC_EXTENSIONS: &str = "x-apple.systempreferences:com.apple.LoginItems-Settings.extension";
/// System Settings → Users & Groups.
const MAC_USERS: &str = "x-apple.systempreferences:com.apple.Users-Groups-Settings.extension";
/// On Linux, what lets this account use `tailscale serve` and `cert` without sudo.
const LINUX_OPERATOR: &str = "sudo tailscale set --operator=$USER";
/// How often the open page looks while something is wrong.
const LOOK: Duration = Duration::from_secs(3);
/// Looks that pass between two looks while all is well, to notice it going away.
const QUIET_LOOKS: u32 = 5;

/// The way out of a trouble.
#[derive(Debug, Clone, PartialEq)]
enum Fix {
    Download,
    OpenApp,
    Open(String),
    /// A page of System Settings.
    Settings(&'static str),
    /// Linux has no app to open: the command to run.
    Copy(&'static str),
    /// Nothing to do but wait a moment.
    None,
}

/// (what is wrong, how to fix it, the button that does it) on `os`.
fn guidance(status: &Status, os: &str) -> (&'static str, &'static str, Fix) {
    let linux = os == "linux";
    let app = |fix: &'static str| if linux { Fix::Copy(fix) } else { Fix::OpenApp };
    if !status.installed {
        return ("remote.problem_not_installed", "remote.setup.tailscale_install", Fix::Download);
    }
    match &status.trouble {
        Some(Trouble::VpnOff) => ("remote.ts.vpn_off", "remote.ts.vpn_off_how", Fix::OpenApp),
        Some(Trouble::NeedsLogin(url)) => (
            "remote.ts.login",
            if linux { "remote.ts.login_how_linux" } else { "remote.ts.login_how" },
            url.clone().map_or_else(|| app("sudo tailscale up"), Fix::Open),
        ),
        Some(Trouble::NeedsApproval) => ("remote.ts.approve", "remote.ts.approve_how", Fix::Open(TAILSCALE_MACHINES.to_string())),
        Some(Trouble::Starting) => ("remote.ts.starting", "remote.ts.starting_how", Fix::None),
        Some(Trouble::Stopped) => {
            ("remote.ts.stopped", if linux { "remote.ts.stopped_how_linux" } else { "remote.ts.stopped_how" }, app("sudo tailscale up"))
        }
        Some(Trouble::NoService(_)) => (
            "remote.ts.no_service",
            match os {
                "macos" => "remote.ts.no_service_how_mac",
                "windows" => "remote.ts.no_service_how_windows",
                _ => "remote.ts.no_service_how_linux",
            },
            app("sudo systemctl enable --now tailscaled && sudo tailscale up"),
        ),
        Some(Trouble::NoIdentity) => ("remote.ts.no_identity", "remote.ts.no_identity_how", app("sudo tailscale up --force-reauth")),
        Some(Trouble::NeedsExtension) => ("remote.ts.extension", "remote.ts.extension_how", Fix::Settings(MAC_EXTENSIONS)),
        Some(Trouble::OtherVpn(_)) => ("remote.ts.other_vpn", "remote.ts.other_vpn_how", Fix::OpenApp),
        Some(Trouble::NotAdmin) => ("remote.ts.not_admin", "remote.ts.not_admin_how", Fix::Settings(MAC_USERS)),
        Some(Trouble::Denied(_)) => ("remote.ts.denied", refusal_how(os), app(LINUX_OPERATOR)),
        Some(Trouble::Tool(_)) | None => ("remote.ts.tool", "remote.ts.tool_how", app("sudo systemctl restart tailscaled")),
    }
}

/// How far it got: installed, switched on, signed in, connected (`None`: can't tell yet).
fn checks(status: &Status) -> [Option<bool>; 4] {
    if !status.installed {
        return [Some(false), None, None, None];
    }
    let yes = Some(true);
    let no = Some(false);
    match &status.trouble {
        None => [yes, yes, yes, yes],
        Some(Trouble::VpnOff | Trouble::Stopped | Trouble::NoService(_) | Trouble::NeedsExtension | Trouble::OtherVpn(_)) => {
            [yes, no, None, no]
        }
        Some(Trouble::Tool(_) | Trouble::NotAdmin | Trouble::Denied(_)) => [yes, None, None, no],
        Some(Trouble::NeedsLogin(_)) => [yes, yes, no, no],
        Some(Trouble::NeedsApproval) => [yes, yes, yes, no],
        Some(Trouble::Starting) => [yes, yes, None, no],
        Some(Trouble::NoIdentity) => [yes, yes, no, yes],
    }
}

/// What an agent is told when the user hands the problem over. Looking comes first and anything
/// that changes the computer is theirs to allow.
fn agent_prompt(report: &str, language: &str) -> String {
    format!(
        "Agentty's remote access needs Tailscale, and on this computer Agentty can't use it. Find out why and help me fix it.\n\n\
         What Agentty found:\n{report}\n\n\
         Agentty needs Tailscale connected and signed in, with MagicDNS and HTTPS certificates on for the tailnet. It asks the \
         `tailscale` tool: `/Applications/Tailscale.app/Contents/MacOS/Tailscale` or `tailscale` on the PATH on macOS, \
         `C:\\Program Files\\Tailscale\\tailscale.exe` on Windows.\n\n\
         Start by only looking: `tailscale version`, `tailscale status`, and on macOS `scutil --nc list`, `systemextensionsctl list` \
         and `id -Gn`. Permissions are a likely cause: an extension macOS hasn't been allowed to run, a Mac account that isn't an \
         administrator, another VPN holding the tunnel, a Linux account that isn't Tailscale's operator, or another Windows user \
         holding the service. Then tell me what is wrong \
         in plain words and the fewest steps that fix it. Assume I don't know networking. Ask me before anything that changes this \
         computer or my tailnet, such as installing, signing in or out, or starting a service. Never turn on Tailscale Funnel. Don't \
         show my account name, tailnet name or any key in your answers.\n\n\
         Talk to me in {language}."
    )
}

impl Workbench {
    /// Keeps looking at Tailscale while the page is open, so nothing has to be pressed once it is
    /// put right. Started once, for as long as the window lasts.
    pub(super) fn watch_tailscale_from_page(&mut self, cx: &mut Context<Self>) {
        let mut quiet = 0;
        self.remote_page.tailscale_watch = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(LOOK).await;
            let looked = this.update(cx, |this, cx| {
                if this.page != Some(super::Page::Remote) || matches!(cx.global::<remote::Remote>().phase, Phase::On { .. }) {
                    return;
                }
                let fine = cx.global::<remote::Remote>().tailscale.as_ref().is_some_and(|s| s.trouble.is_none() && s.installed && s.https);
                quiet = if fine { (quiet + 1) % QUIET_LOOKS } else { 0 };
                if quiet == 0 {
                    remote::refresh_tailscale(cx);
                }
            });
            if looked.is_err() {
                return;
            }
        }));
    }

    /// Opens a tab with an agent, told what was found and asked to sort it out.
    fn ask_agent_about_tailscale(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(status) = cx.global::<remote::Remote>().tailscale.clone() else { return };
        let Some(pick) = self.pick_agent(None, false, cx) else {
            return self.show_toast(t(cx, "agent.none_installed"), cx);
        };
        let language = agentty_bridge::idea::language_name(crate::settings::settings(cx).language.code());
        let prompt = agent_prompt(&tailscale::report(&status, env!("CARGO_PKG_VERSION")), language);
        // Where the user already works, which the agent already trusts.
        let cwd = self
            .active_pane()
            .map(|pane| {
                let view = pane.read(cx);
                view.live_cwd.clone().unwrap_or_else(|| view.spec.cwd.clone())
            })
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        self.page = None;
        self.open_agent_tab(pick, prompt, t(cx, "remote.ts.ask_agent").to_string(), cwd, window, cx);
    }

    /// What is wrong with Tailscale and everything that helps. `recheck` adds a button for it, where
    /// the setup steps' own isn't on screen.
    pub(super) fn render_tailscale_help(&mut self, status: &Status, recheck: bool, cx: &mut Context<Self>) -> Div {
        let (what, how, fix) = guidance(status, std::env::consts::OS);
        let mono = crate::settings::terminal_font(cx);
        let said = match &status.trouble {
            Some(Trouble::NoService(said) | Trouble::Tool(said) | Trouble::Denied(said) | Trouble::OtherVpn(said)) if !said.is_empty() => {
                Some(said.clone())
            }
            _ => None,
        };
        let labels = ["remote.ts.check_installed", "remote.ts.check_on", "remote.ts.check_login", "remote.ts.check_connected"];
        let mut progress = div().flex().flex_wrap().gap_x_3().gap_y_1();
        for (label, done) in labels.into_iter().zip(checks(status)) {
            let (mark, color) = match done {
                Some(true) => ("✓", Chrome::SUCCESS),
                Some(false) => ("✗", Chrome::ERROR),
                None => ("·", Chrome::MUTED),
            };
            progress = progress.child(
                div()
                    .flex()
                    .gap_1()
                    .t_small()
                    .text_color(hex(Chrome::MUTED))
                    .child(div().font_family(mono.clone()).text_color(hex(color)).child(mark))
                    .child(t(cx, label).to_string()),
            );
        }
        let primary = match fix {
            Fix::Download => {
                Some(action_button("remote-ts-fix", t(cx, "remote.install_tailscale"), |_, _, cx| cx.open_url(TAILSCALE_DOWNLOAD)))
            }
            Fix::OpenApp => Some(action_button("remote-ts-fix", t(cx, "remote.ts.open_app"), |_, _, cx| {
                tailscale::open_app();
                remote::refresh_tailscale(cx);
            })),
            Fix::Open(url) => {
                let label = if url == TAILSCALE_MACHINES { "remote.ts.open_machines" } else { "remote.ts.sign_in" };
                Some(action_button("remote-ts-fix", t(cx, label), move |_, _, cx| cx.open_url(&url)))
            }
            Fix::Settings(page) => {
                let label = if page == MAC_USERS { "remote.ts.open_users" } else { "remote.ts.open_extensions" };
                Some(action_button("remote-ts-fix", t(cx, label), move |_, _, cx| cx.open_url(page)))
            }
            Fix::Copy(command) => Some(action_button("remote-ts-fix", t(cx, "remote.ts.copy_command"), move |_, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(command.to_string()))
            })),
            Fix::None => None,
        };
        let report = tailscale::report(status, env!("CARGO_PKG_VERSION"));
        let guide = crate::guide_url(crate::settings::settings(cx).language.code(), "remote-access");
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().t_body().text_color(hex(Chrome::BRIGHT)).child(t(cx, what).to_string()))
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, how).to_string()))
            .when_some(said, |d, said| d.child(div().t_small().font_family(mono.clone()).text_color(hex(Chrome::MUTED)).child(said)))
            .child(progress)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(primary)
                    .when(recheck, |d| {
                        d.child(action_button("remote-ts-recheck", t(cx, "remote.setup.recheck"), |_, _, cx| remote::refresh_tailscale(cx)))
                    })
                    .child(action_button(
                        "remote-ts-agent",
                        t(cx, "remote.ts.ask_agent"),
                        cx.listener(|this, _: &ClickEvent, window, cx| this.ask_agent_about_tailscale(window, cx)),
                    ))
                    .child(action_button(
                        "remote-ts-report",
                        t(cx, "remote.ts.copy_report"),
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(report.clone()));
                            this.show_toast(t(cx, "chat.copied"), cx);
                        }),
                    ))
                    .child(action_button("remote-ts-guide", t(cx, "remote.ts.guide"), move |_, _, cx| cx.open_url(&guide))),
            )
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "remote.ts.auto_check").to_string()))
    }
}

/// The words for a refusal on this system.
fn refusal_how(os: &str) -> &'static str {
    match os {
        "macos" => "remote.ts.denied_how_mac",
        "windows" => "remote.ts.denied_how_windows",
        _ => "remote.ts.denied_how_linux",
    }
}

/// `text` about a failed certificate, and what to do when the tool's words say it was refused.
pub(super) fn with_refusal_help(text: String, said: &str, cx: &gpui::App) -> String {
    if tailscale::is_denied(said) {
        format!("{text} {}", t(cx, refusal_how(std::env::consts::OS)))
    } else {
        text
    }
}

/// What to do about a `serve` that was refused, when that is what happened: it reaches here as the
/// tool's own words, long after the status looked fine.
pub(super) fn refusal_help(said: &str, cx: &gpui::App) -> Option<Div> {
    if !tailscale::is_denied(said) {
        return None;
    }
    let os = std::env::consts::OS;
    let how = refusal_how(os);
    Some(div().flex().flex_col().gap_2().child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, how).to_string())).when(
        os == "linux",
        |d| {
            d.child(div().flex().child(action_button("remote-ts-operator", t(cx, "remote.ts.copy_command"), |_, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(LINUX_OPERATOR.to_string()))
            })))
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(trouble: Trouble) -> Status {
        Status { installed: true, trouble: Some(trouble), ..Default::default() }
    }

    #[test]
    fn every_trouble_gets_its_own_words_and_a_way_out() {
        let all = [
            Trouble::VpnOff,
            Trouble::NeedsLogin(None),
            Trouble::NeedsApproval,
            Trouble::Stopped,
            Trouble::NoService("x".into()),
            Trouble::Tool("x".into()),
            Trouble::NoIdentity,
            Trouble::NeedsExtension,
            Trouble::OtherVpn("Work".into()),
            Trouble::NotAdmin,
            Trouble::Denied("x".into()),
        ];
        let mut said = std::collections::HashSet::new();
        for trouble in all {
            for os in ["macos", "windows", "linux"] {
                let (what, how, fix) = guidance(&with(trouble.clone()), os);
                assert!(fix != Fix::None, "{trouble:?} on {os} has nothing to press");
                assert!(crate::i18n::lookup(what).is_some() && crate::i18n::lookup(how).is_some());
            }
            assert!(said.insert(guidance(&with(trouble), "macos").0), "the same words for two troubles");
        }
    }

    #[test]
    fn the_fix_fits_the_system() {
        assert_eq!(guidance(&Status::default(), "macos").2, Fix::Download);
        assert_eq!(guidance(&with(Trouble::Stopped), "macos").2, Fix::OpenApp);
        assert_eq!(guidance(&with(Trouble::Stopped), "windows").2, Fix::OpenApp);
        assert_eq!(guidance(&with(Trouble::Stopped), "linux").2, Fix::Copy("sudo tailscale up"));
        let url = "https://login.tailscale.com/a/abc".to_string();
        assert_eq!(guidance(&with(Trouble::NeedsLogin(Some(url.clone()))), "linux").2, Fix::Open(url));
    }

    #[test]
    fn a_refusal_on_linux_gets_the_operator_command() {
        assert_eq!(guidance(&with(Trouble::Denied("x".into())), "linux").2, Fix::Copy(LINUX_OPERATOR));
        assert_eq!(guidance(&with(Trouble::Denied("x".into())), "macos").2, Fix::OpenApp);
        assert_eq!(guidance(&with(Trouble::NeedsExtension), "macos").2, Fix::Settings(MAC_EXTENSIONS));
        assert_eq!(guidance(&with(Trouble::NotAdmin), "macos").2, Fix::Settings(MAC_USERS));
    }

    #[test]
    fn progress_shows_how_far_it_got() {
        assert_eq!(checks(&Status::default()), [Some(false), None, None, None]);
        assert_eq!(checks(&with(Trouble::VpnOff)), [Some(true), Some(false), None, Some(false)]);
        assert_eq!(checks(&with(Trouble::NeedsLogin(None))), [Some(true), Some(true), Some(false), Some(false)]);
        assert_eq!(checks(&Status { installed: true, ..Default::default() }), [Some(true); 4]);
    }

    #[test]
    fn the_agent_looks_first_and_asks_before_changing_anything() {
        let prompt = agent_prompt("State: Stopped", "Korean");
        assert!(prompt.contains("State: Stopped") && prompt.contains("Ask me before") && prompt.contains("Never turn on Tailscale Funnel"));
        assert!(prompt.ends_with("Talk to me in Korean."));
    }
}
