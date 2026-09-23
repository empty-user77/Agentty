//! Auto-update: checks the release channel at launch and hourly, announces new versions with a
//! popup and a sidebar badge, and installs them. macOS: verified download → signature and team
//! check → bundle swap → relaunch. Windows: verified download of the setup program, which Agentty
//! starts silently before quitting; the setup program waits for it, installs and starts the new
//! version. Linux (and development builds) only announce the version and open its release page:
//! packages are installed with the package manager.

use super::Workbench;
use crate::i18n::{t, tf};
use crate::theme::{hex, hex_alpha, Chrome};
use crate::ui::{icon, IconSize, TypeScale};
use agentty_bridge::update::{self, Release};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, Window};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Temp folders updates are downloaded into (`<temp>/agentty-update-…`).
const DOWNLOAD_PREFIX: &str = "agentty-update";

/// How this copy of Agentty takes an update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallKind {
    /// Runs from `Agentty.app`: swap the bundle.
    MacBundle,
    /// Installed by the Windows setup program (or the zip's install.ps1 into the same folder).
    WindowsSetup,
    /// Linux packages and development builds: open the release page.
    Manual,
}

pub fn install_kind() -> InstallKind {
    static KIND: OnceLock<InstallKind> = OnceLock::new();
    *KIND.get_or_init(|| {
        if current_bundle().is_some() {
            InstallKind::MacBundle
        } else if cfg!(windows) && installed_on_windows() {
            InstallKind::WindowsSetup
        } else {
            InstallKind::Manual
        }
    })
}

/// Whether `agentty.exe` runs from an installation (not a build folder): the setup program's
/// uninstaller sits next to it, or it is in the default per-user folder install.ps1 also uses.
fn installed_on_windows() -> bool {
    let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) else { return false };
    let default = std::env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join("Programs").join("Agentty"));
    is_windows_install_dir(&dir, default.as_deref())
}

fn is_windows_install_dir(dir: &Path, default: Option<&Path>) -> bool {
    let same = |a: &Path, b: &Path| {
        a.to_string_lossy().trim_end_matches(['\\', '/']).eq_ignore_ascii_case(b.to_string_lossy().trim_end_matches(['\\', '/']))
    };
    dir.join("unins000.exe").is_file() || default.is_some_and(|default| same(dir, default))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Installing(Release),
    /// The release channel could not be reached (proxies, rate limits, …).
    CheckFailed(String),
    Failed(String),
}

/// Install progress shared between the background installer and the popup.
#[derive(Default)]
pub struct InstallProgress {
    stage: AtomicU8,
    downloaded: AtomicU64,
    /// 0 while the size is unknown.
    total: AtomicU64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallStage {
    Downloading,
    Verifying,
    Restarting,
}

impl InstallProgress {
    fn reset(&self) {
        self.set_stage(InstallStage::Downloading);
        self.downloaded.store(0, Ordering::Relaxed);
        self.total.store(0, Ordering::Relaxed);
    }

    fn set_stage(&self, stage: InstallStage) {
        self.stage.store(stage as u8, Ordering::Relaxed);
    }

    fn set_bytes(&self, downloaded: u64, total: Option<u64>) {
        self.downloaded.store(downloaded, Ordering::Relaxed);
        self.total.store(total.unwrap_or(0), Ordering::Relaxed);
    }

    pub fn stage(&self) -> InstallStage {
        match self.stage.load(Ordering::Relaxed) {
            1 => InstallStage::Verifying,
            2 => InstallStage::Restarting,
            _ => InstallStage::Downloading,
        }
    }

    /// Bytes downloaded and the total size, when known.
    pub fn bytes(&self) -> (u64, Option<u64>) {
        let total = self.total.load(Ordering::Relaxed);
        (self.downloaded.load(Ordering::Relaxed), (total > 0).then_some(total))
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0))
}

#[derive(Default)]
pub struct Updates {
    pub state: UpdateState,
    /// Popup currently shown (the launch announcement or a manual check result).
    pub popup: bool,
    /// Version already announced by popup this run, so hourly checks don't nag.
    announced: Option<String>,
    progress: Arc<InstallProgress>,
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

/// A downloaded, verified update, ready to hand over once Agentty quits.
enum Prepared {
    /// The running bundle and the new one staged next to it.
    Bundle { current: PathBuf, staged: PathBuf },
    /// The Windows setup program, checked against the published checksum.
    Setup(PathBuf),
}

/// Downloads and verifies the update into a new private temp folder: stages the new bundle next to
/// the current one on macOS, returns the setup program on Windows.
fn prepare_install(release: &Release, progress: &InstallProgress) -> anyhow::Result<Prepared> {
    let kind = install_kind();
    anyhow::ensure!(kind != InstallKind::Manual, "updates install only into an installed copy of Agentty");
    let work = update::private_download_dir(DOWNLOAD_PREFIX)?;
    // Earlier setup programs can't delete themselves while they run.
    update::remove_old_download_dirs(DOWNLOAD_PREFIX, &work);
    progress.reset();
    let file = update::download(release, &work, CURRENT_VERSION, &mut |done, total| progress.set_bytes(done, total))?;
    progress.set_stage(InstallStage::Verifying);
    if kind == InstallKind::WindowsSetup {
        return Ok(Prepared::Setup(file));
    }
    let staged = stage_from_dmg(&file, &work);
    let _ = std::fs::remove_file(&file);
    staged.map(|(current, staged)| Prepared::Bundle { current, staged })
}

/// Hands the update over; Agentty quits right after.
fn launch_prepared(prepared: &Prepared) -> anyhow::Result<()> {
    match prepared {
        Prepared::Bundle { current, staged } => spawn_relauncher(current, staged),
        Prepared::Setup(setup) => spawn_setup(setup),
    }
}

/// Starts the Windows setup program without questions: it waits for this process to exit (by PID,
/// so other Agentty processes such as MCP servers keep running), installs over this copy and
/// starts the new version (see packaging/windows/agentty.iss).
fn spawn_setup(setup: &Path) -> anyhow::Result<()> {
    Command::new(setup)
        .args(setup_arguments(std::process::id()))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

fn setup_arguments(pid: u32) -> Vec<String> {
    ["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/NOCANCEL", "/RELAUNCH"]
        .into_iter()
        .map(String::from)
        .chain([format!("/WAITPID={pid}")])
        .collect()
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

/// How long "Later" keeps the update popup away.
const LATER: Duration = Duration::from_secs(24 * 60 * 60);

use crate::settings::unix_now;

/// Whether "Later" still holds for `version`: it was pressed for that version and the day is not
/// over. A newer version is announced right away.
fn put_off(later_version: &str, later_until: u64, version: &str, now: u64) -> bool {
    later_version == version && now < later_until
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
        let task = cx.background_spawn(async { update::check(CURRENT_VERSION, std::env::consts::OS, std::env::consts::ARCH) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.updates.state = match result {
                    Ok(Some(release)) => {
                        let later = {
                            let s = crate::settings::settings(cx);
                            (s.update_later_version.clone(), s.update_later_until)
                        };
                        let waiting = put_off(&later.0, later.1, &release.version, unix_now());
                        if !waiting && this.updates.announced.as_deref() != Some(release.version.as_str()) {
                            this.updates.announced = Some(release.version.clone());
                            this.updates.popup = true;
                        }
                        UpdateState::Available(release)
                    }
                    Ok(None) => UpdateState::UpToDate,
                    // Quiet background failures keep whatever we knew before.
                    Err(err) if manual => UpdateState::CheckFailed(format!("{err:#}")),
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
            let work = update::private_download_dir(DOWNLOAD_PREFIX)?;
            let (current, staged) = stage_from_dmg(&dmg, &work)?;
            spawn_relauncher(&current, &staged)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => cx.defer(crate::request_quit),
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
        // Installing ends with a relaunch: unsaved files in the editor come first.
        if self.show_unsaved_files(cx) {
            return;
        }
        if install_kind() == InstallKind::Manual || release.installer_url.is_none() {
            // Linux packages, development builds and releases without an installer for this system:
            // show the download page instead.
            cx.open_url(&release.page_url);
            return;
        }
        self.updates.state = UpdateState::Installing(release.clone());
        self.updates.popup = true;
        self.updates.progress.reset();
        cx.notify();
        let progress = self.updates.progress.clone();
        let task = cx.background_spawn(async move { prepare_install(&release, &progress) });
        // Repaint the progress bar while the installer runs.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let installing = this
                .update(cx, |this, cx| {
                    cx.notify();
                    matches!(this.updates.state, UpdateState::Installing(_))
                })
                .unwrap_or(false);
            if !installing {
                break;
            }
        })
        .detach();
        let progress = self.updates.progress.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await.and_then(|prepared| launch_prepared(&prepared));
            if result.is_ok() {
                // Let the "restarting" state show before the window disappears.
                progress.set_stage(InstallStage::Restarting);
                let _ = this.update(cx, |_, cx| cx.notify());
                cx.background_executor().timer(Duration::from_millis(800)).await;
            }
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => cx.defer(crate::request_quit),
                Err(err) => {
                    this.updates.state = UpdateState::Failed(format!("{err:#}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The status bar's version, which becomes the update's own entry while one waits or installs:
    /// a click opens the update popup (again, after "Later").
    pub(super) fn render_version_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let (label, busy) = match &self.updates.state {
            UpdateState::Available(release) => (tf(cx, "update.available_short", &[("version", &release.version)]), false),
            UpdateState::Installing(_) => (t(cx, "update.installing").to_string(), true),
            _ => return div().text_color(hex(Chrome::SUCCESS)).child(format!("● Agentty v{CURRENT_VERSION}")).into_any_element(),
        };
        div()
            .id("update-status")
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .text_color(hex(Chrome::BLUE))
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .hover(|s| s.text_color(hex(Chrome::BRIGHT)))
            .child(if busy {
                crate::ui::spinner(IconSize::INLINE, hex(Chrome::BLUE)).into_any_element()
            } else {
                icon("package", IconSize::INLINE, hex(Chrome::BLUE)).into_any_element()
            })
            .child(label)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.updates.popup = true;
                cx.notify();
            }))
            .into_any_element()
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
        // "Later" puts the popup off for a day, across restarts; the status bar keeps the update.
        let later = cx.listener(|this, _: &ClickEvent, _, cx| {
            this.updates.popup = false;
            // Announced again once the day is over.
            this.updates.announced = None;
            if let UpdateState::Available(release) = &this.updates.state {
                let version = release.version.clone();
                crate::settings::update_settings(cx, move |s| {
                    s.update_later_version = version;
                    s.update_later_until = unix_now() + LATER.as_secs();
                });
            }
            cx.notify();
        });
        let (title, body, actions): (String, String, gpui::AnyElement) = match &self.updates.state {
            UpdateState::Available(release) => {
                let page = release.page_url.clone();
                // Without an installer for this copy the primary button opens the release page.
                let manual = install_kind() == InstallKind::Manual || release.installer_url.is_none();
                let body_key = match (manual, cfg!(target_os = "linux")) {
                    (true, true) => "update.available_body_linux",
                    (true, false) => "update.available_body_manual",
                    (false, _) => "update.available_body",
                };
                (
                    tf(cx, "update.available_title", &[("version", &release.version)]),
                    tf(cx, body_key, &[("current", CURRENT_VERSION), ("version", &release.version)]),
                    div()
                        .flex()
                        .gap_2()
                        .child(button("update-notes", t(cx, "update.notes").into(), false).on_click(move |_, _, cx| cx.open_url(&page)))
                        .child(button("update-later", t(cx, "update.later").into(), false).on_click(later))
                        .child(
                            button("update-install", t(cx, if manual { "update.download" } else { "update.install" }).into(), true)
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.install_update(cx))),
                        )
                        .into_any_element(),
                )
            }
            UpdateState::Installing(release) => (
                tf(cx, "update.available_title", &[("version", &release.version)]),
                t(cx, if install_kind() == InstallKind::WindowsSetup { "update.installing_body_setup" } else { "update.installing_body" })
                    .into(),
                self.render_install_progress(cx),
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
            // Offer the release page instead of a dead end; it opens in the default browser.
            UpdateState::CheckFailed(_) | UpdateState::Failed(_) => {
                let check = matches!(self.updates.state, UpdateState::CheckFailed(_));
                (
                    t(cx, if check { "update.check_failed" } else { "update.failed" }).into(),
                    t(cx, if check { "update.check_failed_body" } else { "update.failed_body" }).into(),
                    div()
                        .flex()
                        .gap_2()
                        .child(button("update-close", t(cx, "update.close").into(), false).on_click(close))
                        .child(
                            button("update-releases", t(cx, "update.open_releases").into(), true)
                                .on_click(|_, _, cx| cx.open_url(agentty_bridge::update::RELEASES_PAGE)),
                        )
                        .into_any_element(),
                )
            }
        };
        // The technical reason, small, for when someone asks what went wrong.
        let detail = match &self.updates.state {
            UpdateState::CheckFailed(error) | UpdateState::Failed(error) => Some(error.clone()),
            _ => None,
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
                            .child(div().t_large().font_weight(crate::theme::EMPHASIS).text_color(hex(Chrome::BRIGHT)).child(title)),
                    )
                    .when(!body.is_empty(), |d| d.child(div().t_body().text_color(hex(Chrome::FOREGROUND)).child(body)))
                    .when_some(detail, |d, detail| d.child(div().t_caption().text_color(hex(Chrome::MUTED)).line_clamp(3).child(detail)))
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
                    .child(match self.updates.state {
                        UpdateState::Installing(_) => actions,
                        _ => div().flex().justify_end().child(actions).into_any_element(),
                    }),
            )
    }

    /// Stage label, progress bar and "12.3 / 45.6 MB · 27%" while an update installs.
    fn render_install_progress(&self, cx: &mut Context<Self>) -> AnyElement {
        let progress = &self.updates.progress;
        let stage = progress.stage();
        let (done, total) = progress.bytes();
        let fraction = match stage {
            InstallStage::Downloading => total.map(|total| (done as f32 / total as f32).clamp(0.0, 1.0)),
            InstallStage::Verifying | InstallStage::Restarting => Some(1.0),
        };
        let setup = install_kind() == InstallKind::WindowsSetup;
        let label = match stage {
            InstallStage::Downloading => t(cx, "update.stage_downloading"),
            // Windows checks the published checksum; the setup program isn't signed.
            InstallStage::Verifying if setup => t(cx, "update.stage_verifying_download"),
            InstallStage::Verifying => t(cx, "update.stage_verifying"),
            InstallStage::Restarting if setup => t(cx, "update.stage_starting_setup"),
            InstallStage::Restarting => t(cx, "update.stage_restarting"),
        };
        let detail = match (stage, total) {
            (InstallStage::Downloading, Some(total)) => {
                format!("{} / {} MB · {}%", megabytes(done), megabytes(total), (fraction.unwrap_or(0.0) * 100.0).floor() as u32)
            }
            (InstallStage::Downloading, None) => format!("{} MB", megabytes(done)),
            _ => String::new(),
        };
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(crate::ui::spinner(IconSize::BUTTON, hex(Chrome::MUTED)))
                    .child(div().flex_1().t_small().text_color(hex(Chrome::FOREGROUND)).child(label))
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(detail)),
            )
            .child(div().w_full().h(px(6.)).rounded_full().overflow_hidden().bg(hex(0x2d2d30)).when_some(fraction, |d, fraction| {
                d.child(div().h_full().rounded_full().bg(hex(Chrome::ACCENT)).w(gpui::relative(fraction)))
            }))
            .into_any_element()
    }
}

pub const AUTHOR: &str = "Ray Lee";
pub const AUTHOR_EMAIL: &str = "yongyongdev@gmail.com";
pub const AUTHOR_URL: &str = "https://github.com/empty-user77";
pub const WEBSITE: &str = "https://www.agentty.run";
pub const PRIVACY_URL: &str = "https://www.agentty.run/privacy-policy";
pub const TERMS_URL: &str = "https://www.agentty.run/terms-of-service";
pub const EULA_URL: &str = "https://www.agentty.run/eula";
pub const X_URL: &str = "https://x.com/raylee_world";

impl Workbench {
    /// "About Agentty" from the app menu: icon, version, author and a GitHub link.
    pub(super) fn render_about_dialog(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let close = cx.listener(|this, _: &ClickEvent, _, cx| {
            this.about_open = false;
            cx.notify();
        });
        div().id("about-overlay").absolute().inset_0().flex().items_center().justify_center().bg(hex_alpha(0x000000, 0.45)).occlude().child(
            div()
                .w(px(400.))
                .p_6()
                .flex()
                .flex_col()
                .gap_4()
                .rounded_xl()
                .bg(hex(Chrome::OVERLAY))
                .border_1()
                .border_color(hex(Chrome::OVERLAY_BORDER))
                .shadow_lg()
                .child(gpui::img("brand/logo.png").size(px(80.)))
                .child(
                    div()
                        .t_heading()
                        .font_weight(crate::theme::EMPHASIS)
                        .text_color(hex(Chrome::BRIGHT))
                        .child(format!("Agentty v{CURRENT_VERSION}")),
                )
                .child(div().t_body().text_color(hex(Chrome::MUTED)).child(t(cx, "tagline")))
                .child(
                    div()
                        .id("about-website")
                        .t_body()
                        .text_color(hex(Chrome::BLUE))
                        .cursor_pointer()
                        .child("agentty.run")
                        .on_click(|_, _, cx| cx.open_url(WEBSITE)),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .t_body()
                        .whitespace_nowrap()
                        .child(div().text_color(hex(Chrome::MUTED)).child(t(cx, "about.author")))
                        .child(div().text_color(hex(Chrome::FOREGROUND)).child(AUTHOR))
                        .child(div().text_color(hex(Chrome::MUTED)).child(AUTHOR_EMAIL)),
                )
                .child(
                    div()
                        .pt_1()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .id("about-ok")
                                .flex_1()
                                .py_2()
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
                                .py_2()
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
    fn later_holds_a_day_for_that_version_only() {
        let now = 1_000_000;
        let until = now + LATER.as_secs();
        assert!(put_off("0.2.0", until, "0.2.0", now));
        assert!(put_off("0.2.0", until, "0.2.0", until - 1));
        // The day is over: announced again.
        assert!(!put_off("0.2.0", until, "0.2.0", until));
        // A newer version is announced right away.
        assert!(!put_off("0.2.0", until, "0.2.1", now));
        // Never pressed.
        assert!(!put_off("", 0, "0.2.0", now));
    }

    #[test]
    fn development_binary_is_not_a_bundle() {
        // Tests run from target/…/deps, never inside Contents/MacOS.
        assert!(current_bundle().is_none());
        assert_eq!(install_kind(), InstallKind::Manual);
    }

    #[test]
    fn windows_install_folder_is_recognized() {
        let dir = std::env::temp_dir().join(format!("agentty-install-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let default = Path::new("C:\\Users\\someone\\AppData\\Local\\Programs\\Agentty");
        assert!(!is_windows_install_dir(&dir, Some(default)));
        // The folder install.ps1 and the setup program use by default, however it is spelled.
        assert!(is_windows_install_dir(Path::new("c:\\users\\someone\\appdata\\local\\programs\\agentty\\"), Some(default)));
        // Anywhere else only with the setup program's uninstaller next to agentty.exe.
        std::fs::write(dir.join("unins000.exe"), b"").unwrap();
        assert!(is_windows_install_dir(&dir, None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn setup_runs_silently_and_waits_for_this_process() {
        let args = setup_arguments(4242);
        for expected in ["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/RELAUNCH", "/WAITPID=4242"] {
            assert!(args.iter().any(|a| a == expected), "{expected} missing from {args:?}");
        }
    }
}
