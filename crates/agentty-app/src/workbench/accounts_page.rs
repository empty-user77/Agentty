//! Settings → Accounts: how new Claude Code and Codex tabs sign in. The default is each CLI's own
//! login; the other methods (API key, gateway token, OAuth token, Bedrock, Vertex AI, Codex
//! `auth.json`) are for machines where that login isn't possible. Secrets go straight to the OS
//! credential store (`agentty_bridge::agent_auth`); the fields are cleared once saved.

use super::settings_page::{row_with_hint, section};
use super::Workbench;
use crate::i18n::{t, tf};
use crate::text_input::TextInput;
use crate::theme::{hex, Chrome};
use crate::ui::{action_button, chip, TypeScale};
use agentty_bridge::agent_auth::{self, AuthAgent, AuthProfile, ClaudeAuth, CodexAuth, CodexAuthSummary};
use gpui::{div, prelude::*, ClickEvent, Context, Div, Entity, PathPromptOptions, SharedString, Window};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeMethod {
    Cli,
    ApiKey,
    AuthToken,
    OAuthToken,
    Bedrock,
    Vertex,
}

impl ClaudeMethod {
    const ALL: [ClaudeMethod; 6] = [Self::Cli, Self::ApiKey, Self::AuthToken, Self::OAuthToken, Self::Bedrock, Self::Vertex];

    fn of(auth: &ClaudeAuth) -> Self {
        match auth {
            ClaudeAuth::Cli => Self::Cli,
            ClaudeAuth::ApiKey { .. } => Self::ApiKey,
            ClaudeAuth::AuthToken { .. } => Self::AuthToken,
            ClaudeAuth::OAuthToken => Self::OAuthToken,
            ClaudeAuth::Bedrock { .. } => Self::Bedrock,
            ClaudeAuth::Vertex { .. } => Self::Vertex,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Cli => "accounts.method_cli",
            Self::ApiKey => "accounts.method_api_key",
            Self::AuthToken => "accounts.method_auth_token",
            Self::OAuthToken => "accounts.method_oauth",
            Self::Bedrock => "accounts.method_bedrock",
            Self::Vertex => "accounts.method_vertex",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Cli => "accounts.claude_cli_hint",
            Self::ApiKey => "accounts.claude_api_key_hint",
            Self::AuthToken => "accounts.claude_auth_token_hint",
            Self::OAuthToken => "accounts.claude_oauth_hint",
            Self::Bedrock => "accounts.claude_bedrock_hint",
            Self::Vertex => "accounts.claude_vertex_hint",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexMethod {
    Cli,
    ApiKey,
    AuthJson,
}

impl CodexMethod {
    const ALL: [CodexMethod; 3] = [Self::Cli, Self::ApiKey, Self::AuthJson];

    fn of(auth: &CodexAuth) -> Self {
        match auth {
            CodexAuth::Cli => Self::Cli,
            CodexAuth::ApiKey { .. } => Self::ApiKey,
            CodexAuth::AuthJson => Self::AuthJson,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Cli => "accounts.method_cli",
            Self::ApiKey => "accounts.method_api_key",
            Self::AuthJson => "accounts.method_auth_json",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::Cli => "accounts.codex_cli_hint",
            Self::ApiKey => "accounts.codex_api_key_hint",
            Self::AuthJson => "accounts.codex_auth_json_hint",
        }
    }
}

/// Result line under a form: success or error text.
type Status = Option<(bool, String)>;

pub struct AccountsForm {
    claude_method: ClaudeMethod,
    claude_secret: Entity<TextInput>,
    claude_base_url: Entity<TextInput>,
    claude_region: Entity<TextInput>,
    claude_profile: Entity<TextInput>,
    claude_project: Entity<TextInput>,
    claude_status: Status,
    codex_method: CodexMethod,
    codex_secret: Entity<TextInput>,
    codex_base_url: Entity<TextInput>,
    codex_json: Entity<TextInput>,
    codex_status: Status,
    /// Masked form of the saved secrets, refreshed after every change (never the secret itself).
    saved: SavedSecrets,
}

#[derive(Default)]
struct SavedSecrets {
    claude: Option<String>,
    codex: Option<String>,
    codex_json: Option<CodexAuthSummary>,
}

impl SavedSecrets {
    fn read(profile: &AuthProfile) -> Self {
        let masked = |account: Option<&'static str>| account.and_then(agent_auth::load_secret).map(|s| agent_auth::masked(&s));
        SavedSecrets {
            claude: masked(profile.claude.secret_account()),
            codex: masked(profile.codex.secret_account()),
            codex_json: agent_auth::imported_codex_auth(),
        }
    }
}

impl Workbench {
    fn accounts_form(&mut self, window: &mut Window, cx: &mut Context<Self>) -> &mut AccountsForm {
        if self.accounts_form.is_none() {
            let profile = AuthProfile::load();
            let mut input = |text: String, key: &'static str, masked: bool| {
                cx.new(|cx| {
                    let input = TextInput::localized(text, key, window, cx);
                    if masked {
                        input.masked()
                    } else {
                        input
                    }
                })
            };
            let (base_url, region, aws_profile, project) = match &profile.claude {
                ClaudeAuth::ApiKey { base_url } | ClaudeAuth::AuthToken { base_url } => {
                    (base_url.clone(), String::new(), String::new(), String::new())
                }
                ClaudeAuth::Bedrock { region, profile } => (String::new(), region.clone(), profile.clone(), String::new()),
                ClaudeAuth::Vertex { project_id, region } => (String::new(), region.clone(), String::new(), project_id.clone()),
                ClaudeAuth::Cli | ClaudeAuth::OAuthToken => Default::default(),
            };
            let codex_base_url = match &profile.codex {
                CodexAuth::ApiKey { base_url } => base_url.clone(),
                _ => String::new(),
            };
            self.accounts_form = Some(AccountsForm {
                claude_method: ClaudeMethod::of(&profile.claude),
                claude_secret: input(String::new(), "accounts.secret_placeholder", true),
                claude_base_url: input(base_url, "accounts.base_url_placeholder", false),
                claude_region: input(region, "accounts.region_placeholder", false),
                claude_profile: input(aws_profile, "accounts.aws_profile_placeholder", false),
                claude_project: input(project, "accounts.project_placeholder", false),
                claude_status: None,
                codex_method: CodexMethod::of(&profile.codex),
                codex_secret: input(String::new(), "accounts.secret_placeholder", true),
                codex_base_url: input(codex_base_url, "accounts.base_url_placeholder", false),
                codex_json: input(String::new(), "accounts.auth_json_placeholder", true),
                codex_status: None,
                saved: SavedSecrets::read(&profile),
            });
        }
        self.accounts_form.as_mut().expect("accounts form was just created")
    }

    /// The Claude method as the form describes it (not yet saved).
    fn claude_auth_from_form(form: &AccountsForm, cx: &gpui::App) -> ClaudeAuth {
        let text = |input: &Entity<TextInput>| input.read(cx).text().trim().to_string();
        match form.claude_method {
            ClaudeMethod::Cli => ClaudeAuth::Cli,
            ClaudeMethod::ApiKey => ClaudeAuth::ApiKey { base_url: text(&form.claude_base_url) },
            ClaudeMethod::AuthToken => ClaudeAuth::AuthToken { base_url: text(&form.claude_base_url) },
            ClaudeMethod::OAuthToken => ClaudeAuth::OAuthToken,
            ClaudeMethod::Bedrock => ClaudeAuth::Bedrock { region: text(&form.claude_region), profile: text(&form.claude_profile) },
            ClaudeMethod::Vertex => ClaudeAuth::Vertex { project_id: text(&form.claude_project), region: text(&form.claude_region) },
        }
    }

    fn codex_auth_from_form(form: &AccountsForm, cx: &gpui::App) -> CodexAuth {
        match form.codex_method {
            CodexMethod::Cli => CodexAuth::Cli,
            CodexMethod::ApiKey => CodexAuth::ApiKey { base_url: form.codex_base_url.read(cx).text().trim().to_string() },
            CodexMethod::AuthJson => CodexAuth::AuthJson,
        }
    }

    fn save_claude_auth(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.accounts_form.as_ref() else { return };
        let auth = Self::claude_auth_from_form(form, cx);
        let typed = form.claude_secret.read(cx).text().trim().to_string();
        let result = (|| -> anyhow::Result<()> {
            auth.validate()?;
            if let Some(account) = auth.secret_account() {
                if !typed.is_empty() {
                    let secret = if auth == ClaudeAuth::OAuthToken { agent_auth::parse_claude_token(&typed)? } else { typed.clone() };
                    agent_auth::store_secret(account, &secret)?;
                } else if auth.secret_required() && agent_auth::load_secret(account).is_none() {
                    anyhow::bail!("{}", t(cx, "accounts.secret_missing"));
                }
            }
            let mut profile = AuthProfile::load();
            profile.claude = auth;
            profile.save()
        })();
        self.finish_save(AuthAgent::Claude, result, cx);
    }

    fn save_codex_auth(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.accounts_form.as_ref() else { return };
        let auth = Self::codex_auth_from_form(form, cx);
        let typed = form.codex_secret.read(cx).text().trim().to_string();
        let json = form.codex_json.read(cx).text().trim().to_string();
        let result = (|| -> anyhow::Result<()> {
            auth.validate()?;
            match &auth {
                CodexAuth::ApiKey { .. } if !typed.is_empty() => agent_auth::store_secret(agent_auth::account::CODEX_API_KEY, &typed)?,
                CodexAuth::ApiKey { .. } if agent_auth::load_secret(agent_auth::account::CODEX_API_KEY).is_none() => {
                    anyhow::bail!("{}", t(cx, "accounts.secret_missing"))
                }
                CodexAuth::AuthJson if !json.is_empty() => {
                    agent_auth::import_codex_auth_json(&json)?;
                }
                CodexAuth::AuthJson if agent_auth::imported_codex_auth().is_none() => {
                    anyhow::bail!("{}", t(cx, "accounts.auth_json_missing"))
                }
                _ => {}
            }
            let mut profile = AuthProfile::load();
            profile.codex = auth;
            profile.save()
        })();
        self.finish_save(AuthAgent::Codex, result, cx);
    }

    /// Clears the typed secrets (they are in the credential store now) and reports the result.
    fn finish_save(&mut self, agent: AuthAgent, result: anyhow::Result<()>, cx: &mut Context<Self>) {
        let status = match &result {
            Ok(()) => (true, t(cx, "accounts.saved").to_string()),
            Err(err) => (false, format!("{err:#}")),
        };
        let Some(form) = self.accounts_form.as_mut() else { return };
        if result.is_ok() {
            let fields = match agent {
                AuthAgent::Claude => vec![form.claude_secret.clone()],
                AuthAgent::Codex => vec![form.codex_secret.clone(), form.codex_json.clone()],
            };
            for field in fields {
                field.update(cx, |input, cx| input.set_text("", cx));
            }
            form.saved = SavedSecrets::read(&AuthProfile::load());
        }
        match agent {
            AuthAgent::Claude => form.claude_status = Some(status),
            AuthAgent::Codex => form.codex_status = Some(status),
        }
        cx.notify();
    }

    fn reset_auth(&mut self, agent: AuthAgent, cx: &mut Context<Self>) {
        let result = agent_auth::reset(agent);
        if let Some(form) = self.accounts_form.as_mut() {
            match agent {
                AuthAgent::Claude => form.claude_method = ClaudeMethod::Cli,
                AuthAgent::Codex => form.codex_method = CodexMethod::Cli,
            }
        }
        let status = match &result {
            Ok(()) => (true, t(cx, "accounts.removed").to_string()),
            Err(err) => (false, format!("{err:#}")),
        };
        if let Some(form) = self.accounts_form.as_mut() {
            form.saved = SavedSecrets::read(&AuthProfile::load());
            match agent {
                AuthAgent::Claude => form.claude_status = Some(status),
                AuthAgent::Codex => form.codex_status = Some(status),
            }
        }
        cx.notify();
    }

    /// Checks the typed (or else the saved) credential against the provider's API, off the UI thread.
    fn verify_auth(&mut self, agent: AuthAgent, cx: &mut Context<Self>) {
        let Some(form) = self.accounts_form.as_mut() else { return };
        let (claude, codex) = (Self::claude_auth_from_form(form, cx), Self::codex_auth_from_form(form, cx));
        let typed = match agent {
            AuthAgent::Claude => form.claude_secret.read(cx).text().trim().to_string(),
            AuthAgent::Codex => form.codex_secret.read(cx).text().trim().to_string(),
        };
        let checking = Some((true, t(cx, "accounts.checking").to_string()));
        match agent {
            AuthAgent::Claude => form.claude_status = checking,
            AuthAgent::Codex => form.codex_status = checking,
        }
        cx.notify();
        let task = cx.background_spawn(async move {
            match agent {
                AuthAgent::Claude => {
                    let secret = if typed.is_empty() {
                        claude.secret_account().and_then(agent_auth::load_secret).unwrap_or_default()
                    } else if claude == ClaudeAuth::OAuthToken {
                        agent_auth::parse_claude_token(&typed)?
                    } else {
                        typed
                    };
                    anyhow::ensure!(!secret.is_empty(), "no key to check");
                    agent_auth::verify_claude(&claude, &secret)
                }
                AuthAgent::Codex => {
                    let secret =
                        if typed.is_empty() { codex.secret_account().and_then(agent_auth::load_secret).unwrap_or_default() } else { typed };
                    anyhow::ensure!(!secret.is_empty(), "no key to check");
                    agent_auth::verify_codex(&codex, &secret)
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let status = match result {
                    Ok(models) => (true, tf(cx, "accounts.verified", &[("n", &models.to_string())])),
                    Err(err) => (false, format!("{err:#}")),
                };
                if let Some(form) = this.accounts_form.as_mut() {
                    match agent {
                        AuthAgent::Claude => form.claude_status = Some(status),
                        AuthAgent::Codex => form.codex_status = Some(status),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads an `auth.json` picked in the file dialog into the paste field (saved with Save).
    fn pick_codex_auth_json(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.first() else { return };
            let text = std::fs::read_to_string(path);
            let _ = this.update(cx, |this, cx| {
                let Some(form) = this.accounts_form.as_mut() else { return };
                match text {
                    Ok(text) => {
                        form.codex_json.update(cx, |input, cx| input.set_text(text.trim().to_string(), cx));
                        form.codex_status = Some((true, t(cx, "accounts.auth_json_loaded").to_string()));
                    }
                    Err(err) => form.codex_status = Some((false, err.to_string())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_accounts(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.accounts_form(window, cx);
        let form = self.accounts_form.as_ref().expect("accounts form exists");
        let store = agentty_bridge::secret_store::backend_name();

        let field = |input: &Entity<TextInput>| {
            let focus = input.clone();
            div()
                .w(gpui::px(340.))
                .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| window.focus(&gpui::Focusable::focus_handle(&focus, cx)))
                .flex()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(hex(Chrome::BORDER))
                .bg(hex(0x1a1a1a))
                .t_body()
                .font_family(crate::settings::BUNDLED_FONT)
                .text_color(hex(Chrome::BRIGHT))
                .child(input.clone())
        };
        let status_line = |status: &Status| {
            status
                .as_ref()
                .map(|(ok, text)| div().t_small().text_color(hex(if *ok { Chrome::MUTED } else { Chrome::ERROR })).child(text.clone()))
        };
        let saved_text = |masked: &Option<String>, cx: &gpui::App| match masked {
            Some(masked) => tf(cx, "accounts.saved_in", &[("store", store), ("key", masked)]),
            None => t(cx, "accounts.nothing_saved").to_string(),
        };
        let (claude_saved, codex_saved) = (saved_text(&form.saved.claude, cx), saved_text(&form.saved.codex, cx));
        let saved_line = |text: String| div().t_small().text_color(hex(Chrome::MUTED)).child(text);

        // ---- Claude Code
        let mut claude_methods = div().flex().flex_wrap().gap_1();
        for method in ClaudeMethod::ALL {
            claude_methods = claude_methods.child(chip(
                SharedString::from(format!("claude-auth-{method:?}")),
                t(cx, method.label()),
                form.claude_method == method,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(form) = this.accounts_form.as_mut() {
                        form.claude_method = method;
                        form.claude_status = None;
                    }
                    cx.notify();
                }),
            ));
        }
        let method = form.claude_method;
        let mut claude = section("Claude Code")
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "accounts.claude_intro")))
            .child(claude_methods)
            .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(t(cx, method.hint())));
        let secret_label = match method {
            ClaudeMethod::ApiKey => Some("accounts.api_key"),
            ClaudeMethod::AuthToken => Some("accounts.auth_token"),
            ClaudeMethod::OAuthToken => Some("accounts.oauth_token"),
            ClaudeMethod::Bedrock => Some("accounts.bedrock_key"),
            ClaudeMethod::Cli | ClaudeMethod::Vertex => None,
        };
        if let Some(label) = secret_label {
            claude = claude.child(row_with_hint(t(cx, label), t(cx, "accounts.secret_hint"), field(&form.claude_secret)));
        }
        if matches!(method, ClaudeMethod::ApiKey | ClaudeMethod::AuthToken) {
            claude = claude.child(row_with_hint(t(cx, "accounts.base_url"), t(cx, "accounts.base_url_hint"), field(&form.claude_base_url)));
        }
        if method == ClaudeMethod::Vertex {
            claude = claude.child(row_with_hint(t(cx, "accounts.project"), "ANTHROPIC_VERTEX_PROJECT_ID", field(&form.claude_project)));
        }
        if matches!(method, ClaudeMethod::Bedrock | ClaudeMethod::Vertex) {
            let hint = if method == ClaudeMethod::Bedrock { "AWS_REGION" } else { "CLOUD_ML_REGION" };
            claude = claude.child(row_with_hint(t(cx, "accounts.region"), hint, field(&form.claude_region)));
        }
        if method == ClaudeMethod::Bedrock {
            claude = claude.child(row_with_hint(t(cx, "accounts.aws_profile"), "AWS_PROFILE", field(&form.claude_profile)));
        }
        if secret_label.is_some() {
            claude = claude.child(saved_line(claude_saved));
        }
        let can_verify = matches!(method, ClaudeMethod::ApiKey | ClaudeMethod::AuthToken | ClaudeMethod::OAuthToken);
        claude = claude
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(action_button(
                        "claude-auth-save",
                        t(cx, "accounts.save"),
                        cx.listener(|this, _: &ClickEvent, _, cx| this.save_claude_auth(cx)),
                    ))
                    .when(can_verify, |d| {
                        d.child(action_button(
                            "claude-auth-verify",
                            t(cx, "accounts.verify"),
                            cx.listener(|this, _: &ClickEvent, _, cx| this.verify_auth(AuthAgent::Claude, cx)),
                        ))
                    })
                    .child(action_button(
                        "claude-auth-reset",
                        t(cx, "accounts.reset"),
                        cx.listener(|this, _: &ClickEvent, _, cx| this.reset_auth(AuthAgent::Claude, cx)),
                    )),
            )
            .children(status_line(&form.claude_status));

        // ---- Codex
        let mut codex_methods = div().flex().flex_wrap().gap_1();
        for method in CodexMethod::ALL {
            codex_methods = codex_methods.child(chip(
                SharedString::from(format!("codex-auth-{method:?}")),
                t(cx, method.label()),
                form.codex_method == method,
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(form) = this.accounts_form.as_mut() {
                        form.codex_method = method;
                        form.codex_status = None;
                    }
                    cx.notify();
                }),
            ));
        }
        let method = form.codex_method;
        let mut codex = section("Codex")
            .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "accounts.codex_intro")))
            .child(codex_methods)
            .child(div().t_small().text_color(hex(Chrome::FOREGROUND)).child(t(cx, method.hint())));
        match method {
            CodexMethod::Cli => {}
            CodexMethod::ApiKey => {
                codex = codex
                    .child(row_with_hint(t(cx, "accounts.api_key"), t(cx, "accounts.secret_hint"), field(&form.codex_secret)))
                    .child(row_with_hint(t(cx, "accounts.base_url"), t(cx, "accounts.codex_base_url_hint"), field(&form.codex_base_url)))
                    .child(saved_line(codex_saved));
            }
            CodexMethod::AuthJson => {
                let imported = match &form.saved.codex_json {
                    Some(CodexAuthSummary::ApiKey) => t(cx, "accounts.auth_json_api_key").to_string(),
                    Some(CodexAuthSummary::ChatGpt { email, plan }) => tf(
                        cx,
                        "accounts.auth_json_chatgpt",
                        &[("email", email.as_deref().unwrap_or("?")), ("plan", plan.as_deref().unwrap_or("?"))],
                    ),
                    None => t(cx, "accounts.nothing_saved").to_string(),
                };
                codex = codex
                    .child(row_with_hint(
                        t(cx, "accounts.auth_json"),
                        t(cx, "accounts.auth_json_field_hint"),
                        div().flex().gap_2().items_center().child(field(&form.codex_json)).child(action_button(
                            "codex-auth-json-pick",
                            t(cx, "accounts.choose_file"),
                            cx.listener(|this, _: &ClickEvent, _, cx| this.pick_codex_auth_json(cx)),
                        )),
                    ))
                    .child(div().t_small().text_color(hex(Chrome::MUTED)).child(imported));
            }
        }
        codex = codex
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(action_button(
                        "codex-auth-save",
                        t(cx, "accounts.save"),
                        cx.listener(|this, _: &ClickEvent, _, cx| this.save_codex_auth(cx)),
                    ))
                    .when(method == CodexMethod::ApiKey, |d| {
                        d.child(action_button(
                            "codex-auth-verify",
                            t(cx, "accounts.verify"),
                            cx.listener(|this, _: &ClickEvent, _, cx| this.verify_auth(AuthAgent::Codex, cx)),
                        ))
                    })
                    .child(action_button(
                        "codex-auth-reset",
                        t(cx, "accounts.reset"),
                        cx.listener(|this, _: &ClickEvent, _, cx| this.reset_auth(AuthAgent::Codex, cx)),
                    )),
            )
            .children(status_line(&form.codex_status));

        div().flex().flex_col().child(claude).child(codex).child(
            section(t(cx, "accounts.security"))
                .child(div().t_small().text_color(hex(Chrome::MUTED)).child(tf(cx, "accounts.security_body", &[("store", store)])))
                .child(div().t_small().text_color(hex(Chrome::MUTED)).child(t(cx, "accounts.applies"))),
        )
    }
}
