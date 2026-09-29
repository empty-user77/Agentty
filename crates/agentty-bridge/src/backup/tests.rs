use super::*;
use serde_json::json;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentty-backup-{name}-{}-{}", std::process::id(), crate::sync::new_id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn no_secrets(_: &str, _: &str) -> Option<String> {
    None
}

#[test]
fn an_imported_connector_key_is_bound_to_the_host_its_file_names() {
    let connectors = json!([
        { "id": "linear", "baseUrl": "https://api.linear.example/graphql" },
        { "id": "broken", "baseUrl": "not a url" }
    ]);
    assert_eq!(connector_origin(Some(&connectors), "linear").as_deref(), Some("https://api.linear.example"));
    assert_eq!(connector_origin(Some(&connectors), "broken"), None);
    assert_eq!(connector_origin(Some(&connectors), "linear@origin"), None);
    assert_eq!(connector_origin(None, "linear"), None);
}

fn write(dir: &Path, name: &str, value: &Value) {
    fs::write(dir.join(name), serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn sample(dir: &Path) {
    write(
        dir,
        "settings.json",
        &json!({"settingsVersion": 6, "theme": "Nord", "fontSize": 14.0, "sidebarWidth": 312.0, "recentDirs": ["/tmp/x"]}),
    );
    write(dir, "commands.json", &json!({"commands": [{"name": "Dev", "command": "npm run dev"}]}));
    write(dir, "connectors.json", &json!({"connectors": [{"id": "c1", "name": "Example API"}]}));
    write(dir, "db-connections.json", &json!([{"id": "d1", "name": "local", "project": "/somewhere/else"}]));
    write(dir, "agent-auth.json", &json!({"claude": {"kind": "cli"}}));
    fs::create_dir_all(dir.join("themes")).unwrap();
    fs::write(dir.join("themes/Solarized.itermcolors"), "<plist/>").unwrap();
    fs::write(dir.join("themes/notes.txt"), "not a theme").unwrap();
}

#[test]
fn an_export_leaves_out_what_belongs_to_one_computer() {
    let dir = temp_dir("export");
    sample(&dir);
    let bundle = collect_from(&dir, Scope::Full, None, "0.0.0", &no_secrets).unwrap();
    let settings = bundle.settings.as_ref().unwrap();
    assert_eq!(settings["theme"], "Nord");
    assert!(settings.get("sidebarWidth").is_none());
    assert!(settings.get("recentDirs").is_none());
    assert_eq!(bundle.settings_version, Some(6));
    assert_eq!(bundle.files.len(), 4);
    assert_eq!(bundle.themes.keys().collect::<Vec<_>>(), ["Solarized.itermcolors"]);
    assert!(bundle.secrets.is_none());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn sync_carries_only_the_synced_files() {
    let dir = temp_dir("sync-scope");
    sample(&dir);
    let bundle = collect_from(&dir, Scope::Sync, None, "0.0.0", &no_secrets).unwrap();
    assert_eq!(bundle.files.keys().collect::<Vec<_>>(), ["commands.json", "connectors.json"]);
    assert!(bundle.plugins.is_empty());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn secrets_travel_sealed_and_only_the_ones_the_settings_use() {
    let dir = temp_dir("secrets");
    sample(&dir);
    let read = |service: &str, account: &str| -> Option<String> {
        match (service, account) {
            (crate::connectors::KEYCHAIN_SERVICE, "c1") => Some("placeholder-connector-token".into()),
            (DATABASE_SERVICE, "d1") => Some("placeholder-db-password".into()),
            (crate::connectors::KEYCHAIN_SERVICE, "gone") => Some("never exported".into()),
            _ => None,
        }
    };
    let bundle = collect_from(&dir, Scope::Full, Some("pw"), "0.0.0", &read).unwrap();
    let text = serde_json::to_string(&bundle).unwrap();
    assert!(!text.contains("placeholder-connector-token"));
    let plain = crypto::open("pw", bundle.secrets.as_ref().unwrap()).unwrap();
    let secrets: Vec<Secret> = serde_json::from_slice(&plain).unwrap();
    let accounts: Vec<&str> = secrets.iter().map(|s| s.account.as_str()).collect();
    assert_eq!(accounts, ["c1", "d1"]);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn an_import_keeps_this_computers_own_settings() {
    let merged = merge_settings(
        json!({"settingsVersion": 6, "theme": "Nord", "sidebarWidth": 200.0, "onboardingDone": true}),
        &json!({"theme": "Dracula", "sidebarWidth": 999.0, "fontSize": 16.0}),
        Some(6),
    );
    assert_eq!(merged["theme"], "Dracula");
    assert_eq!(merged["fontSize"], 16.0);
    assert_eq!(merged["sidebarWidth"], 200.0);
    assert_eq!(merged["onboardingDone"], true);
}

#[test]
fn settings_from_an_older_version_are_migrated_on_load() {
    let merged = merge_settings(json!({"settingsVersion": 6}), &json!({"theme": "x"}), Some(3));
    assert_eq!(merged["settingsVersion"], 3);
    let merged = merge_settings(json!({"settingsVersion": 6}), &json!({"theme": "x"}), Some(9));
    assert_eq!(merged["settingsVersion"], 6);
}

#[test]
fn files_and_themes_are_written_back() {
    let from = temp_dir("from");
    sample(&from);
    let bundle = collect_from(&from, Scope::Full, None, "0.0.0", &no_secrets).unwrap();
    let to = temp_dir("to");
    write(&to, "settings.json", &json!({"settingsVersion": 6, "theme": "Other", "sidebarWidth": 111.0}));
    let mut report = ImportReport::default();
    apply_to(&to, &bundle, &mut report).unwrap();
    assert_eq!(report.files, 4);
    assert_eq!(report.themes, 1);
    let settings = report.settings.unwrap();
    assert_eq!(settings["theme"], "Nord");
    assert_eq!(settings["sidebarWidth"], 111.0);
    // Settings are the app's to write (it holds them in memory).
    assert_eq!(read_json(&to.join("settings.json")).unwrap()["theme"], "Other");
    assert_eq!(read_json(&to.join("commands.json")), read_json(&from.join("commands.json")));
    assert!(to.join("themes/Solarized.itermcolors").is_file());
    let _ = fs::remove_dir_all(from);
    let _ = fs::remove_dir_all(to);
}

#[test]
fn a_theme_name_cannot_leave_the_themes_folder() {
    assert!(theme_name_ok("Nord.itermcolors"));
    assert!(!theme_name_ok("../settings.itermcolors"));
    assert!(!theme_name_ok("..\\x.itermcolors"));
    assert!(!theme_name_ok(".hidden.itermcolors"));
    assert!(!theme_name_ok("theme.json"));
}

#[test]
fn only_known_services_take_secrets() {
    assert!(known_service(DATABASE_SERVICE));
    assert!(known_service(crate::agent_auth::SERVICE));
    assert!(!known_service("run.agentty.browser"));
    assert!(!known_service("com.apple.something"));
}

#[test]
fn database_projects_follow_the_home_folder() {
    let home = crate::fsutil::home();
    let project = home.join("src").join("app").to_string_lossy().to_string();
    let mut value = json!([{"id": "d1", "project": project}, {"id": "d2", "project": "/opt/elsewhere"}]);
    map_projects(&mut value, &home_to_tilde);
    assert_eq!(value[0]["project"], "~/src/app");
    assert_eq!(value[1]["project"], "/opt/elsewhere");
    map_projects(&mut value, &tilde_to_home);
    assert_eq!(value[0]["project"], project);
}

#[test]
fn a_file_says_what_it_is() {
    assert!(parse(b"{\"kind\":\"something-else\",\"version\":1}").is_err());
    assert!(parse(b"not json").is_err());
    assert!(parse(format!("{{\"kind\":\"{KIND}\",\"version\":99}}").as_bytes()).is_err());
    assert!(parse(format!("{{\"kind\":\"{KIND}\",\"version\":1}}").as_bytes()).is_ok());
}

#[test]
fn the_fingerprint_ignores_key_order_and_what_is_not_synced() {
    let a = Bundle { settings: Some(json!({"a": 1, "b": {"x": 1, "y": 2}})), ..Default::default() };
    let b = Bundle {
        settings: Some(serde_json::from_str(r#"{"b": {"y": 2, "x": 1}, "a": 1}"#).unwrap()),
        created_at: "later".into(),
        device_name: "Other".into(),
        ..Default::default()
    };
    assert_eq!(fingerprint(&a), fingerprint(&b));
    let c = Bundle { settings: Some(json!({"a": 2, "b": {"x": 1, "y": 2}})), ..Default::default() };
    assert_ne!(fingerprint(&a), fingerprint(&c));
}

#[test]
fn a_file_this_computer_keeps_to_itself_is_not_replaced_by_sync() {
    let dir = temp_dir("held");
    let token = ["ghp_", &"Z".repeat(36)].concat();
    let mine = json!({ "commands": [{ "name": "deploy", "run": format!("curl -H 'Authorization: token {token}'") }] });
    fs::write(dir.join("commands.json"), serde_json::to_vec(&mine).unwrap()).unwrap();
    fs::write(dir.join("connectors.json"), b"[]").unwrap();
    let mut incoming = Bundle { kind: KIND.into(), version: FORMAT_VERSION, ..Default::default() };
    incoming.files.insert("commands.json".into(), json!({ "commands": [{ "name": "hello", "run": "echo hi" }] }));
    incoming.files.insert("connectors.json".into(), json!([{ "name": "api" }]));
    import_synced_to(&dir, &incoming).unwrap();
    let commands: serde_json::Value = serde_json::from_slice(&fs::read(dir.join("commands.json")).unwrap()).unwrap();
    assert_eq!(commands, mine, "the command holding a credential stays");
    let connectors: serde_json::Value = serde_json::from_slice(&fs::read(dir.join("connectors.json")).unwrap()).unwrap();
    assert_eq!(connectors, json!([{ "name": "api" }]), "the rest still follows");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn plugin_settings_go_in_the_file_and_secret_ones_only_sealed() {
    let dir = temp_dir("plugin-settings");
    let token = ["ghp_", &"Z".repeat(36)].concat();
    for (id, value) in [("launch", json!({ "region": "eu" })), ("notes", json!({ "token": token })), ("../bad", json!({ "x": 1 }))] {
        let folder = dir.join("plugin-data").join(id);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("storage.json"), value.to_string()).unwrap();
    }
    let plain = collect_from(&dir, Scope::Full, None, "0", &no_secrets).unwrap();
    assert_eq!(plain.plugin_settings.keys().collect::<Vec<_>>(), ["launch"]);
    assert!(!serde_json::to_string(&plain).unwrap().contains(&token), "without a password the secret one stays out");

    let sealed = collect_from(&dir, Scope::Full, Some("correct horse battery"), "0", &no_secrets).unwrap();
    assert!(!serde_json::to_string(&sealed).unwrap().contains(&token), "only sealed");
    let opened: Vec<Secret> =
        serde_json::from_slice(&crypto::open("correct horse battery", sealed.secrets.as_ref().unwrap()).unwrap()).unwrap();
    assert!(opened.iter().any(|s| s.service == PLUGIN_SETTINGS_SERVICE && s.account == "notes"));

    // Sync never carries them: its bundle leaves plugin settings to their own files.
    assert!(collect_from(&dir, Scope::Sync, None, "0", &no_secrets).unwrap().plugin_settings.is_empty());

    let target = temp_dir("plugin-settings-in");
    let mut report = ImportReport::default();
    apply_to(&target, &plain, &mut report).unwrap();
    assert_eq!(report.plugin_settings, 1);
    let restored: serde_json::Value = serde_json::from_slice(&fs::read(target.join("plugin-data/launch/storage.json")).unwrap()).unwrap();
    assert_eq!(restored, json!({ "region": "eu" }));
    let _ = fs::remove_dir_all(dir);
    let _ = fs::remove_dir_all(target);
}
