//! Settings → Backup: the whole configuration exported to one file and imported from one, by hand.
//! Keeping it in step between computers is Settings → Sync ("Sync settings"). The work itself is
//! `agentty_bridge::backup`.

use super::settings_page::{row_with_hint, section, switch};
use super::Workbench;
use crate::i18n::{t, tf};
use crate::text_input::TextInput;
use crate::theme::{hex, Chrome};
use crate::ui::{action_button, middle_ellipsis, tilde, TypeScale};
use agentty_bridge::backup::{self, Bundle, ImportError, ImportOptions, ImportReport, Scope};
use gpui::{div, prelude::*, px, ClickEvent, Context, Div, Entity, PathPromptOptions, Window};
use std::path::PathBuf;

/// Shorter passwords are refused: the file may travel, and the secrets in it are only as safe as this.
const MIN_PASSWORD: usize = 8;

#[derive(Default)]
pub(super) struct BackupUi {
    include_secrets: bool,
    password: Option<Entity<TextInput>>,
    password_again: Option<Entity<TextInput>>,
    /// A file picked for import, read and waiting for "Import".
    pending: Option<(PathBuf, Bundle)>,
    import_password: Option<Entity<TextInput>>,
    install_plugins: bool,
    busy: bool,
    /// The last result, and whether it is an error.
    message: Option<(String, bool)>,
    /// The message is about an export (shown under Export), else under Import.
    message_on_export: bool,
}

/// Where the configuration before an import is kept.
fn copies_dir() -> PathBuf {
    agentty_bridge::fsutil::data_dir().join("backups")
}

fn masked_input(slot: &mut Option<Entity<TextInput>>, window: &mut Window, cx: &mut Context<Workbench>) -> Entity<TextInput> {
    slot.get_or_insert_with(|| cx.new(|cx| TextInput::new("", "", window, cx).masked())).clone()
}

fn read_text(slot: &Option<Entity<TextInput>>, cx: &gpui::App) -> String {
    slot.as_ref().map(|input| input.read(cx).text().to_string()).unwrap_or_default()
}

fn clear(slot: &Option<Entity<TextInput>>, cx: &mut Context<Workbench>) {
    if let Some(input) = slot {
        input.update(cx, |input, cx| input.set_text(String::new(), cx));
    }
}

impl Workbench {
    fn backup_message(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.backup.message = Some((text.into(), error));
        self.backup.message_on_export = false;
        cx.notify();
    }

    fn export_message(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.backup_message(text, error, cx);
        self.backup.message_on_export = true;
    }

    fn export_configuration(&mut self, cx: &mut Context<Self>) {
        if self.backup.busy {
            return;
        }
        let password = if self.backup.include_secrets {
            let password = read_text(&self.backup.password, cx);
            if password.chars().count() < MIN_PASSWORD {
                return self.export_message(tf(cx, "backup.password_short", &[("n", &MIN_PASSWORD.to_string())]), true, cx);
            }
            if password != read_text(&self.backup.password_again, cx) {
                return self.export_message(t(cx, "backup.password_mismatch"), true, cx);
            }
            Some(password)
        } else {
            None
        };
        let home = agentty_bridge::fsutil::home();
        let folder = [home.join("Desktop"), home.join("Downloads"), home.clone()].into_iter().find(|d| d.is_dir()).unwrap_or(home);
        let path = cx.prompt_for_new_path(&folder, Some(&backup::suggested_file_name()));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else { return };
            let _ = this.update(cx, |this, cx| this.export_to(path, password, cx));
        })
        .detach();
    }

    fn export_to(&mut self, mut path: PathBuf, password: Option<String>, cx: &mut Context<Self>) {
        if path.extension().is_none() {
            path.set_extension(backup::EXTENSION);
        }
        self.backup.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let result = cx
                .background_spawn(async move {
                    let bundle = backup::collect(Scope::Full, password.as_deref(), env!("CARGO_PKG_VERSION"))?;
                    backup::write_file(&target, &bundle)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.backup.busy = false;
                match result {
                    Ok(()) => {
                        clear(&this.backup.password, cx);
                        clear(&this.backup.password_again, cx);
                        let text = tf(cx, "backup.exported", &[("path", &tilde(&path))]);
                        this.export_message(text, false, cx);
                    }
                    Err(err) => this.export_message(format!("{err:#}"), true, cx),
                }
            });
        })
        .detach();
    }

    fn pick_import_file(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| this.read_import_file(path, cx));
        })
        .detach();
    }

    fn read_import_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let read = path.clone();
            let result = cx.background_spawn(async move { backup::read_file(&read) }).await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(bundle) => {
                    this.backup.install_plugins = !bundle.plugins.is_empty();
                    this.backup.pending = Some((path, bundle));
                    this.backup.message = None;
                    clear(&this.backup.import_password, cx);
                    cx.notify();
                }
                Err(err) => this.backup_message(format!("{err:#}"), true, cx),
            });
        })
        .detach();
    }

    fn import_configuration(&mut self, cx: &mut Context<Self>) {
        let Some((_, bundle)) = self.backup.pending.clone() else { return };
        if self.backup.busy {
            return;
        }
        let password = Some(read_text(&self.backup.import_password, cx)).filter(|p| !p.is_empty() && bundle.secrets.is_some());
        let options = ImportOptions {
            password,
            plugins: self.backup.install_plugins,
            keep_copy_in: Some(copies_dir()),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        };
        self.backup.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { backup::import(&bundle, &options) }).await;
            let _ = this.update(cx, |this, cx| {
                this.backup.busy = false;
                match result {
                    Ok(report) => this.finish_import(report, cx),
                    Err(ImportError::WrongPassword) => this.backup_message(t(cx, "backup.wrong_password"), true, cx),
                    Err(err) => this.backup_message(err.to_string(), true, cx),
                }
            });
        })
        .detach();
    }

    fn finish_import(&mut self, report: ImportReport, cx: &mut Context<Self>) {
        if let Some(settings) = report.settings.clone() {
            if let Err(err) = crate::settings::replace_settings(cx, settings) {
                return self.backup_message(format!("{err:#}"), true, cx);
            }
        } else {
            crate::settings::reload_themes(cx);
        }
        self.backup.pending = None;
        clear(&self.backup.import_password, cx);
        // Pages that read sign-in methods and chat credentials once read them again.
        self.accounts_form = None;
        self.chat_notify = Default::default();
        let mut text = tf(
            cx,
            "backup.imported",
            &[("files", &report.files.to_string()), ("themes", &report.themes.to_string()), ("secrets", &report.secrets.to_string())],
        );
        if !report.plugins_installed.is_empty() {
            text.push(' ');
            text.push_str(&tf(cx, "backup.plugins_installed", &[("list", &report.plugins_installed.join(", "))]));
        }
        if !report.plugins_skipped.is_empty() {
            text.push(' ');
            text.push_str(&tf(cx, "backup.plugins_skipped", &[("list", &report.plugins_skipped.join(", "))]));
        }
        if let Some(copy) = &report.copy {
            text.push(' ');
            text.push_str(&tf(cx, "backup.copy_kept", &[("path", &tilde(copy))]));
        }
        self.backup_message(text, false, cx);
    }

    pub(super) fn render_backup_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let field = |input: Entity<TextInput>| {
            div()
                .w(px(220.))
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(0x1a1a1a))
                .t_body()
                .child(input)
        };
        let muted = |text: String| div().t_small().text_color(hex(Chrome::MUTED)).child(text);
        let busy = self.backup.busy;

        // Export.
        let include = self.backup.include_secrets;
        let store = agentty_bridge::secret_store::backend_name();
        let mut export = section(t(cx, "backup.export_title")).child(muted(t(cx, "backup.export_intro").to_string())).child(row_with_hint(
            t(cx, "backup.include_secrets"),
            &tf(cx, "backup.include_secrets_hint", &[("store", store)]),
            switch(
                "backup-include-secrets",
                include,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.backup.include_secrets = !include;
                    cx.notify();
                }),
            ),
        ));
        if include {
            let password = masked_input(&mut self.backup.password, window, cx);
            let again = masked_input(&mut self.backup.password_again, window, cx);
            export = export
                .child(row_with_hint(
                    t(cx, "backup.password"),
                    &tf(cx, "backup.password_hint", &[("n", &MIN_PASSWORD.to_string())]),
                    field(password),
                ))
                .child(row_with_hint(t(cx, "backup.password_again"), "", field(again)));
        }
        let color = |error: bool| hex(if error { Chrome::ERROR } else { Chrome::SUCCESS });
        let message = self.backup.message.clone().map(|(text, error)| div().t_small().text_color(color(error)).child(text));
        let (export_message, import_message) = if self.backup.message_on_export { (message, None) } else { (None, message) };
        export = export.child(
            div().flex().child(
                action_button(
                    "backup-export",
                    t(cx, if busy { "backup.working" } else { "backup.export" }),
                    cx.listener(|this, _: &ClickEvent, _, cx| this.export_configuration(cx)),
                )
                .when(busy, |b| b.opacity(0.5)),
            ),
        );
        let export = export.children(export_message);

        // Import.
        let mut import =
            section(t(cx, "backup.import_title")).child(muted(tf(cx, "backup.import_intro", &[("path", &tilde(&copies_dir()))])));
        match self.backup.pending.clone() {
            None => {
                import = import.child(div().flex().child(action_button(
                    "backup-choose",
                    t(cx, "backup.choose_file"),
                    cx.listener(|this, _: &ClickEvent, _, cx| this.pick_import_file(cx)),
                )));
            }
            Some((path, bundle)) => {
                let date = bundle.created_at.get(..10).unwrap_or(&bundle.created_at).to_string();
                let mut items = Vec::new();
                if bundle.settings.is_some() {
                    items.push(t(cx, "backup.item.settings").to_string());
                }
                if !bundle.files.is_empty() {
                    items.push(tf(cx, "backup.item.files", &[("n", &bundle.files.len().to_string())]));
                }
                if !bundle.themes.is_empty() {
                    items.push(tf(cx, "backup.item.themes", &[("n", &bundle.themes.len().to_string())]));
                }
                if !bundle.plugins.is_empty() {
                    items.push(tf(cx, "backup.item.plugins", &[("n", &bundle.plugins.len().to_string())]));
                }
                if bundle.secrets.is_some() {
                    items.push(t(cx, "backup.item.secrets").to_string());
                }
                let summary = div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(hex(Chrome::BORDER))
                    .child(div().t_body().text_color(hex(Chrome::BRIGHT)).truncate().child(middle_ellipsis(&tilde(&path), 90)))
                    .child(muted(tf(
                        cx,
                        "backup.file_from",
                        &[("device", &bundle.device_name), ("version", &bundle.app_version), ("date", &date)],
                    )))
                    .child(muted(tf(cx, "backup.contains", &[("items", &items.join(" · "))])));
                import = import.child(summary);
                if bundle.secrets.is_some() {
                    let password = masked_input(&mut self.backup.import_password, window, cx);
                    import = import.child(row_with_hint(t(cx, "backup.password"), t(cx, "backup.import_password_hint"), field(password)));
                }
                if !bundle.plugins.is_empty() {
                    let on = self.backup.install_plugins;
                    import = import.child(row_with_hint(
                        t(cx, "backup.install_plugins"),
                        t(cx, "backup.install_plugins_hint"),
                        switch(
                            "backup-install-plugins",
                            on,
                            cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.backup.install_plugins = !on;
                                cx.notify();
                            }),
                        ),
                    ));
                }
                import = import.child(
                    div()
                        .flex()
                        .gap_1()
                        .child(
                            action_button(
                                "backup-import",
                                t(cx, if busy { "backup.working" } else { "backup.import" }),
                                cx.listener(|this, _: &ClickEvent, _, cx| this.import_configuration(cx)),
                            )
                            .when(busy, |b| b.opacity(0.5)),
                        )
                        .child(action_button(
                            "backup-import-cancel",
                            t(cx, "confirm.cancel"),
                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.backup.pending = None;
                                cx.notify();
                            }),
                        )),
                );
            }
        }
        let import = import.children(import_message);

        // Between computers.
        let synced = self.sync_settings_on();
        let between = section(t(cx, "backup.sync_title")).child(muted(t(cx, "backup.sync_body").to_string())).child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(action_button(
                    "backup-open-sync",
                    t(cx, "backup.open_sync"),
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.settings_section = super::settings_page::SettingsSection::Sync;
                        cx.notify();
                    }),
                ))
                .child(muted(t(cx, if synced { "backup.sync_on" } else { "backup.sync_off" }).to_string())),
        );

        div().flex().flex_col().child(export).child(import).child(between)
    }

    /// `debug backup export <path> [password] | pick <path> | import [password] | plugins on|off | state`.
    pub(super) fn debug_backup(&mut self, argument: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (command, rest) = argument.split_once(' ').unwrap_or((argument, ""));
        let mut words = rest.split_whitespace().map(str::to_string);
        match command {
            "export" => {
                if let Some(path) = words.next() {
                    self.export_to(PathBuf::from(path), words.next(), cx);
                }
            }
            "pick" => self.read_import_file(PathBuf::from(rest.trim()), cx),
            "import" => {
                if let Some(password) = words.next() {
                    let input = masked_input(&mut self.backup.import_password, window, cx);
                    input.update(cx, |input, cx| input.set_text(password, cx));
                }
                self.import_configuration(cx);
            }
            "plugins" => self.backup.install_plugins = rest.trim() == "on",
            _ => {}
        }
        eprintln!(
            "backup: busy={} pending={} message={:?}",
            self.backup.busy,
            self.backup.pending.as_ref().map(|(p, _)| p.display().to_string()).unwrap_or_default(),
            self.backup.message
        );
        cx.notify();
    }
}
