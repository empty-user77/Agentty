//! End to end: devices syncing through a local bare repository.

use super::*;
use anyhow::Context as _;
use restore::{restore_in, RestoreOutcome, RestoreRequest};
use std::process::Command;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentty-sync-e2e-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn bare(dir: &Path) -> String {
    let status = Command::new("git").args(["init", "--quiet", "--bare", "-b", "main"]).arg(dir).status().unwrap();
    assert!(status.success());
    dir.to_string_lossy().to_string()
}

fn device(id: &str, name: &str, url: &str) -> SyncConfig {
    SyncConfig {
        remote: Some(Remote::Git { url: url.into() }),
        branch: "main".into(),
        device_id: id.into(),
        device_name: name.into(),
        ..Default::default()
    }
}

/// Transcripts under `home/<agent>/`, as a test's agent folders.
fn locate(home: &Path) -> Locate {
    let home = home.to_path_buf();
    let root = home.clone();
    Locate {
        transcript: Box::new(move |agent, id| {
            // Anywhere below the agent's folder, as restored sessions sit in a project folder.
            let mut files = Vec::new();
            crate::fsutil::jsonl_files(&home.join(agent.id()), 2, &mut files);
            files
                .into_iter()
                // Codex names its rollouts `rollout-<time>-<id>.jsonl`.
                .find(|p| p.file_stem().is_some_and(|s| s == id || s.to_string_lossy().ends_with(&format!("-{id}"))))
                .context("missing")
        }),
        root: Box::new(move |agent| root.join(agent.id())),
        knows_folder: Box::new(|_, _, _| true),
    }
}

fn workspace(sync_id: &str, cwd: &Path, sessions: &[&str]) -> WorkspaceInput {
    WorkspaceInput {
        local_key: format!("0:{sync_id}"),
        sync_id: Some(sync_id.into()),
        plugin: None,
        name: Some("App".into()),
        group: None,
        color: None,
        cwd: cwd.to_path_buf(),
        state: WorkspaceState::Open,
        sessions: sessions
            .iter()
            .map(|id| SessionInput {
                agent: Agent::Claude,
                id: (*id).into(),
                title: format!("session {id}"),
                open: true,
                cwd: None,
                parent: None,
                started_ms: None,
            })
            .collect(),
    }
}

#[test]
fn two_devices_share_a_workspace_without_conflicts() {
    let root = temp("two");
    let url = bare(&root.join("remote.git"));
    let (home_a, home_b) = (root.join("home-a"), root.join("home-b"));
    fs::create_dir_all(home_a.join("claude")).unwrap();
    fs::create_dir_all(home_b.join("claude")).unwrap();
    fs::write(home_a.join("claude/sa.jsonl"), "{\"n\":1}\n").unwrap();
    fs::write(home_b.join("claude/sb.jsonl"), "{\"m\":1}\n").unwrap();

    let a = device("dev-a", "Office", &url);
    let b = device("dev-b", "Home", &url);
    let repo_a = Repo::new(root.join("clone-a"), a.remote.clone().unwrap(), "main");
    let repo_b = Repo::new(root.join("clone-b"), b.remote.clone().unwrap(), "main");
    repo_a.ensure_clone().unwrap();
    repo_b.ensure_clone().unwrap();

    let request_a = SyncRequest { workspaces: vec![workspace("ws1", &root, &["sa"])], ..Default::default() };
    let request_b = SyncRequest { workspaces: vec![workspace("ws1", &root, &["sb"])], ..Default::default() };

    // The first sync sets the repository up and pushes.
    let first = run(&repo_a, &mut a.clone(), &request_a, &locate(&home_a)).unwrap();
    assert!(first.pushed);
    assert_eq!(first.sessions_uploaded, 1);
    assert!(repo_a.dir.join(MARKER_FILE).is_file());
    // Each top folder says what it holds.
    for folder in ["devices", "workspace", "plugin"] {
        assert!(repo_a.dir.join(folder).join("README.md").is_file(), "{folder}/README.md");
    }

    // The other device adds its section and its session next to the first one's.
    let second = run(&repo_b, &mut b.clone(), &request_b, &locate(&home_b)).unwrap();
    assert!(second.pushed);
    let meta = &second.overview.workspaces[0];
    assert_eq!(meta.devices.len(), 2);
    assert_eq!(meta.sessions().len(), 2);
    assert_eq!(second.overview.devices.len(), 2);

    // A appends to its transcript; B pushed in between: A's pass picks B's work up first.
    fs::write(home_a.join("claude/sa.jsonl"), "{\"n\":1}\n{\"n\":2}\n").unwrap();
    let third = run(&repo_a, &mut a.clone(), &request_a, &locate(&home_a)).unwrap();
    assert!(third.pushed);
    let chunks = repo_a.dir.join("workspace/ws1/devices/dev-a/claude/sa");
    assert_eq!(fs::read_to_string(chunks.join("0002.jsonl")).unwrap(), "{\"n\":2}\n");
    assert!(repo_a.dir.join("workspace/ws1/devices/dev-b/claude/sb/0001.jsonl").is_file());

    // The session comes back whole on the other computer.
    repo_b.fetch().unwrap();
    repo_b.reset_to_remote().unwrap();
    let rebuilt = root.join("restored/sa.jsonl");
    session::rebuild(&repo_b.dir.join("workspace/ws1/devices/dev-a/claude/sa"), &rebuilt, None, &|l| l.to_string()).unwrap();
    assert_eq!(fs::read_to_string(rebuilt).unwrap(), "{\"n\":1}\n{\"n\":2}\n");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_unchanged_workspace_commits_nothing() {
    let root = temp("idle");
    let url = bare(&root.join("remote.git"));
    let home = root.join("home");
    fs::create_dir_all(home.join("claude")).unwrap();
    fs::write(home.join("claude/s.jsonl"), "{\"n\":1}\n").unwrap();
    let a = device("dev-a", "A", &url);
    let repo = Repo::new(root.join("clone"), a.remote.clone().unwrap(), "main");
    repo.ensure_clone().unwrap();
    let request = SyncRequest { workspaces: vec![workspace("ws", &root, &["s"])], ..Default::default() };
    assert!(run(&repo, &mut a.clone(), &request, &locate(&home)).unwrap().pushed);
    let again = run(&repo, &mut a.clone(), &request, &locate(&home)).unwrap();
    assert!(!again.pushed, "a second pass with nothing new must not commit");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_push_that_lost_the_race_is_rejected_then_retried() {
    let root = temp("race");
    let url = bare(&root.join("remote.git"));
    let home = root.join("home");
    fs::create_dir_all(home.join("claude")).unwrap();
    fs::write(home.join("claude/s.jsonl"), "{\"n\":1}\n").unwrap();
    let a = device("dev-a", "A", &url);
    let b = device("dev-b", "B", &url);
    let repo_a = Repo::new(root.join("clone-a"), a.remote.clone().unwrap(), "main");
    let repo_b = Repo::new(root.join("clone-b"), b.remote.clone().unwrap(), "main");
    repo_a.ensure_clone().unwrap();
    repo_b.ensure_clone().unwrap();
    let request = SyncRequest { workspaces: vec![workspace("ws", &root, &["s"])], ..Default::default() };
    run(&repo_a, &mut a.clone(), &request, &locate(&home)).unwrap();

    // B commits on a stale clone: its push is refused, not forced.
    initialize(&repo_b.dir, "dev-b").unwrap();
    fs::write(repo_b.dir.join("stale.txt"), "x").unwrap();
    assert!(repo_b.commit_all("stale", &Author { device_name: "B" }).unwrap());
    assert_eq!(repo_b.push().unwrap(), Pushed::Rejected);

    // A full pass fetches, drops the stale commit and pushes B's own files.
    let outcome = run(&repo_b, &mut b.clone(), &request, &locate(&home)).unwrap();
    assert!(outcome.pushed);
    assert!(!repo_b.dir.join("stale.txt").exists());
    assert!(repo_b.dir.join("workspace/ws/devices/dev-a").is_dir());
    assert!(repo_b.dir.join("workspace/ws/devices/dev-b").is_dir());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_repository_with_other_files_is_refused() {
    let root = temp("foreign");
    let url = bare(&root.join("remote.git"));
    let seed = Repo::new(root.join("seed"), Remote::Git { url: url.clone() }, "main");
    seed.ensure_clone().unwrap();
    fs::write(seed.dir.join("main.rs"), "fn main() {}").unwrap();
    seed.commit_all("code", &Author { device_name: "x" }).unwrap();
    assert_eq!(seed.push().unwrap(), Pushed::Done);

    let config = device("dev-a", "A", &url);
    let repo = Repo::new(root.join("clone"), config.remote.clone().unwrap(), "main");
    repo.ensure_clone().unwrap();
    let err = run(&repo, &mut config.clone(), &SyncRequest::default(), &locate(&root)).unwrap_err();
    assert_eq!(err, SyncError::NotSyncRepository);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn purged_workspaces_leave_the_repository() {
    let root = temp("purge");
    let url = bare(&root.join("remote.git"));
    let home = root.join("home");
    fs::create_dir_all(home.join("claude")).unwrap();
    fs::write(home.join("claude/s.jsonl"), "{\"n\":1}\n").unwrap();
    let a = device("dev-a", "A", &url);
    let repo = Repo::new(root.join("clone"), a.remote.clone().unwrap(), "main");
    repo.ensure_clone().unwrap();
    run(&repo, &mut a.clone(), &SyncRequest { workspaces: vec![workspace("ws", &root, &["s"])], ..Default::default() }, &locate(&home))
        .unwrap();
    assert!(repo.dir.join("workspace/ws").is_dir());
    let outcome =
        run(&repo, &mut a.clone(), &SyncRequest { purge: vec![(None, "ws".into())], ..Default::default() }, &locate(&home)).unwrap();
    assert!(outcome.pushed);
    assert!(!repo.dir.join("workspace/ws").exists());
    assert!(outcome.overview.workspaces.is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn closed_sessions_stay_listed() {
    let root = temp("closed");
    let url = bare(&root.join("remote.git"));
    let home = root.join("home");
    fs::create_dir_all(home.join("claude")).unwrap();
    fs::write(home.join("claude/s1.jsonl"), "{\"n\":1}\n").unwrap();
    fs::write(home.join("claude/s2.jsonl"), "{\"n\":1}\n").unwrap();
    let a = device("dev-a", "A", &url);
    let repo = Repo::new(root.join("clone"), a.remote.clone().unwrap(), "main");
    repo.ensure_clone().unwrap();
    run(
        &repo,
        &mut a.clone(),
        &SyncRequest { workspaces: vec![workspace("ws", &root, &["s1", "s2"])], ..Default::default() },
        &locate(&home),
    )
    .unwrap();
    let outcome = run(
        &repo,
        &mut a.clone(),
        &SyncRequest { workspaces: vec![workspace("ws", &root, &["s2"])], ..Default::default() },
        &locate(&home),
    )
    .unwrap();
    let meta = &outcome.overview.workspaces[0];
    let s1 = meta.devices["dev-a"].sessions.iter().find(|s| s.id == "s1").unwrap();
    assert!(!s1.open);
    assert!(s1.closed_at.is_some());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ids_from_outside_never_become_paths_out_of_the_clone() {
    let root = Path::new("/r");
    assert!(workspace_dir(root, None, "../x").is_none());
    assert!(workspace_dir(root, Some(".."), "ws").is_none());
    assert!(workspace_dir(root, Some("a/b"), "ws").is_none());
    assert_eq!(workspace_dir(root, Some("launch"), "ws"), Some(PathBuf::from("/r/plugin/launch/ws")));
    assert_ne!(new_id(), new_id());
}

#[test]
fn the_same_workspace_on_another_computer_joins_it() {
    let root = temp("join");
    let url = bare(&root.join("remote.git"));
    let (home_a, home_b) = (root.join("home-a"), root.join("home-b"));
    fs::create_dir_all(home_a.join("claude")).unwrap();
    fs::create_dir_all(home_b.join("claude")).unwrap();
    fs::write(home_a.join("claude/sa.jsonl"), "{\"n\":1}\n").unwrap();
    fs::write(home_b.join("claude/sb.jsonl"), "{\"m\":1}\n").unwrap();
    let mut a = device("dev-a", "Office", &url);
    let mut b = device("dev-b", "Home", &url);
    let repo_a = Repo::new(root.join("clone-a"), a.remote.clone().unwrap(), "main");
    let repo_b = Repo::new(root.join("clone-b"), b.remote.clone().unwrap(), "main");
    repo_a.ensure_clone().unwrap();
    repo_b.ensure_clone().unwrap();

    // Neither computer has an id for "App" yet.
    let fresh = |session: &str| {
        let mut input = workspace("unused", &root, &[session]);
        input.sync_id = None;
        input.local_key = "0:7".into();
        SyncRequest { workspaces: vec![input], ..Default::default() }
    };
    run(&repo_a, &mut a, &fresh("sa"), &locate(&home_a)).unwrap();
    let outcome = run(&repo_b, &mut b, &fresh("sb"), &locate(&home_b)).unwrap();
    assert_eq!(a.workspaces["0:7"], b.workspaces["0:7"], "one workspace in the repository");
    assert_eq!(outcome.overview.workspaces.len(), 1);
    assert_eq!(outcome.overview.workspaces[0].devices.len(), 2);

    // A second local workspace of the same project on B gets one of its own.
    let mut second = workspace("unused", &root, &["sb"]);
    second.sync_id = None;
    second.local_key = "0:8".into();
    run(&repo_b, &mut b, &SyncRequest { workspaces: vec![second], ..Default::default() }, &locate(&home_b)).unwrap();
    assert_ne!(b.workspaces["0:7"], b.workspaces["0:8"]);
    let _ = fs::remove_dir_all(root);
}

/// Computer A syncs a session in `/a/app`; returns (root, remote url, A's clone, A's config).
fn a_synced(name: &str, lines: &str) -> (PathBuf, String, Repo, SyncConfig) {
    let root = temp(name);
    let url = bare(&root.join("remote.git"));
    let home_a = root.join("home-a");
    fs::create_dir_all(home_a.join("claude")).unwrap();
    fs::write(home_a.join("claude/s.jsonl"), lines).unwrap();
    let mut a = device("dev-a", "Office", &url);
    let repo_a = Repo::new(root.join("clone-a"), a.remote.clone().unwrap(), "main");
    repo_a.ensure_clone().unwrap();
    let mut input = workspace("ws", Path::new("/a/app"), &["s"]);
    input.sessions[0].cwd = Some(PathBuf::from("/a/app"));
    run(&repo_a, &mut a, &SyncRequest { workspaces: vec![input], ..Default::default() }, &locate(&home_a)).unwrap();
    (root, url, repo_a, a)
}

fn b_clone(root: &Path, url: &str) -> (Repo, SyncConfig) {
    let b = device("dev-b", "Home", url);
    let repo = Repo::new(root.join("clone-b"), b.remote.clone().unwrap(), "main");
    repo.ensure_clone().unwrap();
    repo.fetch().unwrap();
    repo.reset_to_remote().unwrap();
    (repo, b)
}

fn request(workspace_cwd: &Path) -> RestoreRequest {
    RestoreRequest {
        plugin: None,
        sync_id: "ws".into(),
        device: "dev-a".into(),
        agent: Agent::Claude,
        session: "s".into(),
        workspace_cwd: workspace_cwd.to_path_buf(),
        keep_local: false,
    }
}

#[test]
fn a_session_comes_to_another_computer_in_its_folder() {
    let (root, url, _, _) = a_synced("restore-new", "{\"cwd\":\"/a/app\",\"sessionId\":\"s\",\"uuid\":\"1\"}\n");
    let (repo_b, b) = b_clone(&root, &url);
    let home_b = root.join("home-b");
    let here = root.join("b-app");
    fs::create_dir_all(&here).unwrap();
    let restored = restore_in(&repo_b.dir, &b, &request(&here), &locate(&home_b)).unwrap();
    assert_eq!(restored.outcome, RestoreOutcome::Downloaded);
    assert_eq!(restored.id, "s");
    assert_eq!(restored.cwd, here);
    // Where Claude Code looks for a session of that folder, with the folder changed.
    let folder: String = here.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let text = fs::read_to_string(home_b.join("claude").join(folder).join("s.jsonl")).unwrap();
    assert!(text.contains(&format!("\"cwd\":{}", serde_json::to_string(&here.to_string_lossy()).unwrap())), "{text}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_local_session_is_updated_kept_or_branched() {
    let first = "{\"sessionId\":\"s\",\"uuid\":\"1\"}\n";
    let second = "{\"sessionId\":\"s\",\"uuid\":\"2\"}\n";
    let (root, url, _, _) = a_synced("restore-local", &format!("{first}{second}"));
    let (repo_b, b) = b_clone(&root, &url);
    let home_b = root.join("home-b");
    let local = home_b.join("claude/-b-app/s.jsonl");
    fs::create_dir_all(local.parent().unwrap()).unwrap();

    // B has the start of it: brought up to date.
    fs::write(&local, first).unwrap();
    let restored = restore_in(&repo_b.dir, &b, &request(&root), &locate(&home_b)).unwrap();
    assert_eq!(restored.outcome, RestoreOutcome::Updated);
    assert_eq!(fs::read_to_string(&local).unwrap(), format!("{first}{second}"));

    // B went further: left alone.
    let third = "{\"sessionId\":\"s\",\"uuid\":\"3\"}\n";
    fs::write(&local, format!("{first}{second}{third}")).unwrap();
    let restored = restore_in(&repo_b.dir, &b, &request(&root), &locate(&home_b)).unwrap();
    assert_eq!(restored.outcome, RestoreOutcome::UpToDate);

    // B went elsewhere: the synced one becomes a new session, B's stays as it is.
    let mine = format!("{first}{{\"sessionId\":\"s\",\"uuid\":\"b2\"}}\n");
    fs::write(&local, &mine).unwrap();
    let restored = restore_in(&repo_b.dir, &b, &request(&root), &locate(&home_b)).unwrap();
    assert_eq!(restored.outcome, RestoreOutcome::Branched);
    assert_ne!(restored.id, "s");
    assert_eq!(fs::read_to_string(&local).unwrap(), mine);
    let branch = (locate(&home_b).transcript)(Agent::Claude, &restored.id).unwrap();
    let text = fs::read_to_string(branch).unwrap();
    assert!(text.contains(&format!("\"sessionId\":\"{}\"", restored.id)) && !text.contains("\"sessionId\":\"s\""));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_continued_session_says_where_it_came_from() {
    let (root, url, _, _) = a_synced("restore-origin", "{\"sessionId\":\"s\",\"uuid\":\"1\"}\n");
    let (repo_b, mut b) = b_clone(&root, &url);
    let home_b = root.join("home-b");
    let restored = restore_in(&repo_b.dir, &b, &request(&root), &locate(&home_b)).unwrap();
    b.origins.insert(
        restore::origin_key(Agent::Claude, &restored.id),
        model::ParentRef { device: "dev-a".into(), session: "s".into(), message: None },
    );
    b.workspaces.insert("0:ws".into(), "ws".into());
    let outcome =
        run(&repo_b, &mut b, &SyncRequest { workspaces: vec![workspace("ws", &root, &["s"])], ..Default::default() }, &locate(&home_b))
            .unwrap();
    let meta = &outcome.overview.workspaces[0];
    let mine = meta.devices["dev-b"].sessions.iter().find(|s| s.id == "s").unwrap();
    assert_eq!(mine.parent.as_ref().map(|p| p.device.as_str()), Some("dev-a"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_session_open_here_is_never_rewritten_under_it() {
    let first = "{\"sessionId\":\"s\",\"uuid\":\"1\"}\n";
    let (root, url, _, _) = a_synced("restore-open", &format!("{first}{{\"sessionId\":\"s\",\"uuid\":\"2\"}}\n"));
    let (repo_b, b) = b_clone(&root, &url);
    let home_b = root.join("home-b");
    let local = home_b.join("claude/-b-app/s.jsonl");
    fs::create_dir_all(local.parent().unwrap()).unwrap();
    fs::write(&local, first).unwrap();
    let mut open = request(&root);
    open.keep_local = true;
    let restored = restore_in(&repo_b.dir, &b, &open, &locate(&home_b)).unwrap();
    assert_eq!(restored.outcome, RestoreOutcome::Branched);
    assert_eq!(fs::read_to_string(&local).unwrap(), first);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_synced_session_is_shared_as_context() {
    let lines = concat!(
        "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"Fix the login bug\"},\"uuid\":\"1\",\"sessionId\":\"s\"}\n",
        "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"The token check was inverted.\"}]},\"uuid\":\"2\",\"sessionId\":\"s\"}\n",
    );
    let (root, _, repo_a, _) = a_synced("context", lines);
    let request = restore::ContextRequest {
        plugin: None,
        sync_id: "ws".into(),
        device: "dev-a".into(),
        agent: Agent::Claude,
        session: "s".into(),
        to: Agent::Claude,
    };
    let (manifest, turns) = restore::context_turns(&repo_a.dir, &root.join("scratch"), &request).unwrap();
    assert_eq!(manifest.title, "session s");
    assert_eq!(turns.len(), 2);
    assert!(turns[0].text.contains("Fix the login bug"));
    assert!(turns[1].text.contains("token check"));
    // The rebuilt transcript is gone once read.
    assert!(fs::read_dir(root.join("scratch")).map(|d| d.count()).unwrap_or(0) == 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_codex_rollout_that_went_on_separately_becomes_a_branch() {
    let root = temp("codex-branch");
    let url = bare(&root.join("remote.git"));
    let (home_a, home_b) = (root.join("home-a"), root.join("home-b"));
    let id = "019a0000-0000-7000-8000-000000000001";
    let name = format!("rollout-2026-09-26T10-00-00-{id}.jsonl");
    let meta = format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"cwd\":\"/a/app\"}}}}\n");
    fs::create_dir_all(home_a.join("codex")).unwrap();
    fs::write(home_a.join("codex").join(&name), format!("{meta}{{\"type\":\"x\",\"n\":1}}\n{{\"type\":\"x\",\"n\":2}}\n")).unwrap();
    let mut a = device("dev-a", "A", &url);
    let repo_a = Repo::new(root.join("clone-a"), a.remote.clone().unwrap(), "main");
    repo_a.ensure_clone().unwrap();
    let mut input = workspace("ws", Path::new("/a/app"), &[]);
    input.sessions.push(SessionInput {
        agent: Agent::Codex,
        id: id.into(),
        title: "t".into(),
        open: true,
        cwd: None,
        parent: None,
        started_ms: None,
    });
    run(&repo_a, &mut a, &SyncRequest { workspaces: vec![input], ..Default::default() }, &locate(&home_a)).unwrap();

    // B went its own way after the first line.
    let (repo_b, b) = b_clone(&root, &url);
    fs::create_dir_all(home_b.join("codex")).unwrap();
    let local = home_b.join("codex").join(&name);
    fs::write(&local, format!("{meta}{{\"type\":\"x\",\"n\":9}}\n")).unwrap();
    let mut request = request(&root);
    request.agent = Agent::Codex;
    request.session = id.into();
    let restored = restore_in(&repo_b.dir, &b, &request, &locate(&home_b)).unwrap();
    assert_eq!(restored.outcome, RestoreOutcome::Branched);
    let branch = home_b.join("codex").join(name.replace(id, &restored.id));
    let text = fs::read_to_string(branch).unwrap();
    assert!(text.starts_with(&format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{}\"", restored.id)), "{text}");
    assert!(text.contains("\"n\":2"));
    assert_eq!(fs::read_to_string(&local).unwrap(), format!("{meta}{{\"type\":\"x\",\"n\":9}}\n"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn sessions_kept_elsewhere_are_listed_but_not_uploaded() {
    let root = temp("unsupported");
    let url = bare(&root.join("remote.git"));
    let home = root.join("home");
    fs::create_dir_all(home.join("agy")).unwrap();
    // A database, as Antigravity keeps its conversations: never cut into chunks.
    fs::write(home.join("agy").join("c1.jsonl"), [0u8, 1, 2, b'\n', 3]).unwrap();
    let mut a = device("dev-a", "A", &url);
    let repo = Repo::new(root.join("clone"), a.remote.clone().unwrap(), "main");
    repo.ensure_clone().unwrap();
    let mut input = workspace("ws", &root, &[]);
    input.sessions.push(SessionInput {
        agent: Agent::Agy,
        id: "c1".into(),
        title: "t".into(),
        open: true,
        cwd: None,
        parent: None,
        started_ms: None,
    });
    let outcome = run(&repo, &mut a, &SyncRequest { workspaces: vec![input], ..Default::default() }, &locate(&home)).unwrap();
    assert!(!repo.dir.join("workspace/ws/devices/dev-a/agy").exists());
    assert_eq!(outcome.overview.workspaces[0].devices["dev-a"].sessions[0].id, "c1");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn gemini_and_kimi_sessions_come_back_only_into_the_same_folder() {
    let root = temp("same-folder");
    let url = bare(&root.join("remote.git"));
    let (home_a, home_b) = (root.join("home-a"), root.join("home-b"));
    fs::create_dir_all(home_a.join("gemini")).unwrap();
    fs::write(home_a.join("gemini").join("g1.jsonl"), "{\"sessionId\":\"g1\"}\n").unwrap();
    let mut a = device("dev-a", "A", &url);
    let repo_a = Repo::new(root.join("clone-a"), a.remote.clone().unwrap(), "main");
    repo_a.ensure_clone().unwrap();
    let mut input = workspace("ws", Path::new("/a/app"), &[]);
    input.sessions.push(SessionInput {
        agent: Agent::Gemini,
        id: "g1".into(),
        title: "t".into(),
        open: true,
        cwd: None,
        parent: None,
        started_ms: None,
    });
    run(&repo_a, &mut a, &SyncRequest { workspaces: vec![input], ..Default::default() }, &locate(&home_a)).unwrap();
    let (repo_b, b) = b_clone(&root, &url);
    let mut request = request(&root);
    request.agent = Agent::Gemini;
    request.session = "g1".into();
    // `/a/app` is not on this computer: refused, nothing written.
    let err = restore_in(&repo_b.dir, &b, &request, &locate(&home_b)).unwrap_err();
    assert!(format!("{err:#}").contains("same folder"), "{err:#}");
    assert!(!home_b.join("gemini").exists());
    let _ = fs::remove_dir_all(root);
}
