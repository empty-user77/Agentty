//! Auto-update: checks the release channel at launch and hourly, announces new versions with a
//! popup and a sidebar badge, and installs them (verified download → signature and team check →
//! bundle swap → relaunch).

use super::Workbench;
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, TypeScale};
use agentty_bridge::update::{self, Release};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, Window};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Debug, Default, PartialEq)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Installing(Release),
    Failed(String),
}

#[derive(Default)]
pub struct Updates {
    pub state: UpdateState,
    /// Popup currently shown (the launch announcement or a manual check result).
    pub popup: bool,
    /// Version already announced by popup this run, so hourly checks don't nag.
    announced: Option<String>,
}

/// The `.app` bundle this process runs from, if any.
pub fn current_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundle = exe.parent()?.parent()?.parent()?.to_path_buf();
    (bundle.extension().is_some_and(|e| e == "app") && exe.parent()?.ends_with("Contents/MacOS")).then_some(bundle)
}

fn team_id(app: &Path) -> Option<String> {
    let output = Command::new("codesign").args(["-dv", "--verbose=2"]).arg(app).output().ok()?;
    let text = String::from_utf8_lossy(&output.stderr).to_string();
    text.lines().find_map(|l| l.strip_prefix("TeamIdentifier=")).map(str::to_string).filter(|t| t != "not set")
}

fn run(command: &mut Command) -> anyhow::Result<String> {
    let output = command.output()?;
    anyhow::ensure!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr).trim());
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Downloads, verifies and stages the new bundle next to the current one; returns the staged path.
fn prepare_install(release: &Release) -> anyhow::Result<(PathBuf, PathBuf)> {
    anyhow::ensure!(current_bundle().is_some(), "updates install only into the Agentty app bundle");
    let work = std::env::temp_dir().join(format!("agentty-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let dmg = update::download(release, &work, CURRENT_VERSION)?;
    let staged = stage_from_dmg(&dmg, &work);
    let _ = std::fs::remove_file(&dmg);
    staged
}

/// Mounts `dmg`, checks the app's signature, developer team and notarization, and copies it next
/// to the running bundle.
fn stage_from_dmg(dmg: &Path, work: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let current = current_bundle().ok_or_else(|| anyhow::anyhow!("updates install only into the Agentty app bundle"))?;
    let current_team = team_id(&current).ok_or_else(|| anyhow::anyhow!("this copy of Agentty is not signed"))?;

    let mount = work.join("mount");
    std::fs::create_dir_all(&mount)?;
    run(Command::new("hdiutil").args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint"]).arg(&mount).arg(dmg))?;
    let result = (|| {
        let new_app = std::fs::read_dir(&mount)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or_else(|| anyhow::anyhow!("the update contains no app"))?;
        // Same developer, intact signature, notarized: otherwise it is not ours.
        run(Command::new("codesign").args(["--verify", "--deep", "--strict"]).arg(&new_app))?;
        anyhow::ensure!(team_id(&new_app).as_deref() == Some(current_team.as_str()), "the update is signed by a different developer");
        run(Command::new("spctl").args(["--assess", "--type", "execute"]).arg(&new_app))?;
        let staged = current.with_file_name(".Agentty-update.app");
        let _ = std::fs::remove_dir_all(&staged);
        run(Command::new("ditto").arg(&new_app).arg(&staged))?;
        Ok(staged)
    })();
    let _ = Command::new("hdiutil").args(["detach", "-quiet"]).arg(&mount).status();
    Ok((current, result?))
}

/// After Agentty exits: swap the bundles and open the new version.
fn spawn_relauncher(current: &Path, staged: &Path) -> anyhow::Result<()> {
    let quote = crate::launch::shell_quote;
    let backup = current.with_file_name(".Agentty-previous.app");
    let script = format!(
        "while kill -0 {pid} 2>/dev/null; do sleep 0.2; done; rm -rf {backup}; mv {current} {backup} && mv {staged} {current} && rm -rf {backup}; open {current}",
        pid = std::process::id(),
        backup = quote(&backup.display().to_string()),
        current = quote(&current.display().to_string()),
        staged = quote(&staged.display().to_string()),
    );
    Command::new("/bin/sh")
        .args(["-c", &script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

impl Workbench {
    pub(super) fn start_update_checks(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            if this.update(cx, |this, cx| this.check_for_updates(false, cx)).is_err() {
                break;
            }
            cx.background_executor().timer(CHECK_INTERVAL).await;
        })
        .detach();
    }

    /// `manual`: from the menu — always report the result in the popup.
    pub fn check_for_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        if matches!(self.updates.state, UpdateState::Checking | UpdateState::Installing(_)) {
            return;
        }
        let previous = std::mem::replace(&mut self.updates.state, UpdateState::Checking);
        if manual {
            self.updates.popup = true;
        }
        cx.notify();
        let task = cx.background_spawn(async { update::check(CURRENT_VERSION, std::env::consts::ARCH) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.updates.state = match result {
                    Ok(Some(release)) => {
                        if this.updates.announced.as_deref() != Some(release.version.as_str()) {
                            this.updates.announced = Some(release.version.clone());
                            this.updates.popup = true;
                        }
                        UpdateState::Available(release)
                    }
                    Ok(None) => UpdateState::UpToDate,
                    // Quiet background failures keep whatever we knew before.
                    Err(err) if manual => UpdateState::Failed(format!("{err:#}")),
                    Err(_) => match previous {
                        UpdateState::Available(release) => UpdateState::Available(release),
                        _ => UpdateState::Idle,
                    },
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Debug: install from a local DMG through the same verification and swap as a real update.
    pub fn install_local_dmg(&mut self, dmg: PathBuf, cx: &mut Context<Self>) {
        let task = cx.background_spawn(async move {
            let work = std::env::temp_dir().join(format!("agentty-update-local-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&work);
            std::fs::create_dir_all(&work)?;
            let (current, staged) = stage_from_dmg(&dmg, &work)?;
            spawn_relauncher(&current, &staged)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => cx.quit(),
                Err(err) => {
                    eprintln!("update: {err:#}");
                    this.updates.state = UpdateState::Failed(format!("{err:#}"));
                    this.updates.popup = true;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn install_update(&mut self, cx: &mut Context<Self>) {
        let UpdateState::Available(release) = self.updates.state.clone() else {
            return self.check_for_updates(true, cx);
        };
        if current_bundle().is_none() {
            // Development builds can't replace themselves; show the download page instead.
            cx.open_url(&release.page_url);
            return;
        }
        self.updates.state = UpdateState::Installing(release.clone());
        self.updates.popup = true;
        cx.notify();
        let task = cx.background_spawn(async move { prepare_install(&release) });
        cx.spawn(async move |this, cx| {
            let result = task.await.and_then(|(current, staged)| spawn_relauncher(&current, &staged));
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => cx.quit(),
                Err(err) => {
                    this.updates.state = UpdateState::Failed(format!("{err:#}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// "Update available: x.y.z" at the bottom of the sidebar.
    /// Floating pill in the bottom-left corner while an update waits: "Update available: 0.2.0".
    /// A click opens the update popup.
    pub(super) fn render_update_badge(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (label, busy) = match &self.updates.state {
            UpdateState::Available(release) => (tf(cx, "update.available_short", &[("version", &release.version)]), false),
            UpdateState::Installing(_) => (t(cx, "update.installing").to_string(), true),
            _ => return None,
        };
        let pill = div()
            .id("update-badge")
            .h(px(28.))
            .flex()
            .items_center()
            .gap_1p5()
            .pl_2p5()
            .pr_3()
            .rounded_full()
            .bg(hex(Chrome::ACCENT))
            .border_1()
            .border_color(hex_alpha(0xffffff, 0.18))
            .shadow_lg()
            .text_color(hex(Chrome::BRIGHT))
            .t_small()
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .hover(|s| s.bg(hex(0x1a8ae6)))
            .child(if busy {
                crate::ui::spinner(IconSize::INLINE, hex(Chrome::BRIGHT)).into_any_element()
            } else {
                icon("package", IconSize::INLINE, hex(Chrome::BRIGHT)).into_any_element()
            })
            .child(label)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.updates.popup = true;
                cx.notify();
            }));
        Some(
            div()
                .absolute()
                .left(px(super::chrome::ACTIVITY_BAR_WIDTH + 12.))
                .bottom(px(super::chrome::STATUS_BAR_HEIGHT + 12.))
                .child(crate::ui::fade_in("update-badge-fade", pill))
                .into_any_element(),
        )
    }

    pub(super) fn render_update_popup(&self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let button = |id: &'static str, label: String, primary: bool| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded_md()
                .t_body()
                .cursor_pointer()
                .when(primary, |d| d.bg(hex(Chrome::ACCENT)).text_color(hex(Chrome::BRIGHT)).hover(|s| s.bg(hex(0x1a8ae6))))
                .when(!primary, |d| d.bg(hex(0x2d2d30)).text_color(hex(Chrome::FOREGROUND)).hover(|s| s.bg(hex(Chrome::HOVER))))
                .child(label)
        };
        let close = cx.listener(|this, _: &ClickEvent, _, cx| {
            this.updates.popup = false;
            cx.notify();
        });
        let (title, body, actions): (String, String, gpui::AnyElement) = match &self.updates.state {
            UpdateState::Available(release) => {
                let page = release.page_url.clone();
                (
                    tf(cx, "update.available_title", &[("version", &release.version)]),
                    tf(cx, "update.available_body", &[("current", CURRENT_VERSION), ("version", &release.version)]),
                    div()
                        .flex()
                        .gap_2()
                        .child(button("update-notes", t(cx, "update.notes").into(), false).on_click(move |_, _, cx| cx.open_url(&page)))
                        .child(button("update-later", t(cx, "update.later").into(), false).on_click(close))
                        .child(
                            button("update-install", t(cx, "update.install").into(), true)
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.install_update(cx))),
                        )
                        .into_any_element(),
                )
            }
            UpdateState::Installing(release) => (
                tf(cx, "update.available_title", &[("version", &release.version)]),
                t(cx, "update.installing_body").into(),
                div().into_any_element(),
            ),
            UpdateState::Checking => (
                t(cx, "update.checking").into(),
                String::new(),
                crate::ui::spinner(IconSize::BUTTON, hex(Chrome::MUTED)).into_any_element(),
            ),
            UpdateState::UpToDate | UpdateState::Idle => (
                t(cx, "update.up_to_date").into(),
                tf(cx, "update.up_to_date_body", &[("version", CURRENT_VERSION)]),
                button("update-ok", "OK".into(), true).on_click(close).into_any_element(),
            ),
            UpdateState::Failed(error) => {
                (t(cx, "update.failed").into(), error.clone(), button("update-ok", "OK".into(), true).on_click(close).into_any_element())
            }
        };
        let notes = match &self.updates.state {
            UpdateState::Available(release) if !release.notes.trim().is_empty() => {
                Some(release.notes.chars().take(600).collect::<String>())
            }
            _ => None,
        };

        div()
            .id("update-overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hex_alpha(0x000000, 0.45))
            .occlude()
            .child(
                div()
                    .w(px(440.))
                    .p_5()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .rounded_xl()
                    .bg(hex(Chrome::OVERLAY))
                    .border_1()
                    .border_color(hex(Chrome::OVERLAY_BORDER))
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(gpui::img("brand/logo.png").size(px(40.)).flex_shrink_0())
                            .child(div().t_large().font_weight(FontWeight::SEMIBOLD).text_color(hex(Chrome::BRIGHT)).child(title)),
                    )
                    .when(!body.is_empty(), |d| d.child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(body)))
                    .when_some(notes, |d, notes| {
                        d.child(
                            div()
                                .id("update-notes-text")
                                .max_h(px(160.))
                                .overflow_y_scroll()
                                .p_3()
                                .rounded_md()
                                .bg(hex(0x1a1a1a))
                                .t_small()
                                .text_color(hex(Chrome::MUTED))
                                .child(notes),
                        )
                    })
                    .child(div().flex().justify_end().child(actions)),
            )
    }
}

pub const AUTHOR: &str = "이용범 (yongyongdev@gmail.com)";
pub const AUTHOR_URL: &str = "https://github.com/empty-user77";
pub const WEBSITE: &str = "https://agentty.run";

impl Workbench {
    /// "About Agentty" from the app menu: icon, version, author and a GitHub link.
    pub(super) fn render_about_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let close = cx.listener(|this, _: &ClickEvent, _, cx| {
            this.about_open = false;
            cx.notify();
        });
        div().id("about-overlay").absolute().inset_0().flex().items_center().justify_center().bg(hex_alpha(0x000000, 0.45)).occlude().child(
            div()
                .w(px(300.))
                .p_5()
                .flex()
                .flex_col()
                .gap_3()
                .rounded_xl()
                .bg(hex(Chrome::OVERLAY))
                .border_1()
                .border_color(hex(Chrome::OVERLAY_BORDER))
                .shadow_lg()
                .child(gpui::img("brand/logo.png").size(px(64.)))
                .child(
                    div()
                        .t_title()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(hex(Chrome::BRIGHT))
                        .child(format!("Agentty v{CURRENT_VERSION}")),
                )
                .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "tagline")))
                .child(
                    div()
                        .id("about-website")
                        .t_body()
                        .text_color(hex(Chrome::BLUE))
                        .cursor_pointer()
                        .child("agentty.run")
                        .on_click(|_, _, cx| cx.open_url(WEBSITE)),
                )
                .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(format!("{}: {AUTHOR}", t(cx, "about.author"))))
                .child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(AUTHOR_URL))
                .child(
                    div()
                        .pt_1()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .id("about-ok")
                                .flex_1()
                                .py_1p5()
                                .flex()
                                .justify_center()
                                .rounded_md()
                                .bg(hex(0x2d2d30))
                                .t_body()
                                .text_color(hex(Chrome::FOREGROUND))
                                .cursor_pointer()
                                .hover(|s| s.bg(hex(Chrome::HOVER)))
                                .child(t(cx, "about.ok"))
                                .on_click(close),
                        )
                        .child(
                            div()
                                .id("about-github")
                                .flex_1()
                                .py_1p5()
                                .flex()
                                .justify_center()
                                .rounded_md()
                                .bg(hex(Chrome::ACCENT))
                                .t_body()
                                .text_color(hex(Chrome::BRIGHT))
                                .cursor_pointer()
                                .hover(|s| s.bg(hex(0x1a8ae6)))
                                .child(t(cx, "about.github"))
                                .on_click(|_, _, cx| cx.open_url(AUTHOR_URL)),
                        ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_binary_is_not_a_bundle() {
        // Tests run from target/…/deps, never inside Contents/MacOS.
        assert!(current_bundle().is_none());
    }
}
