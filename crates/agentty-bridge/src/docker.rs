//! Docker for the project a pane works in: its compose file or Dockerfile, the compose services (or,
//! without compose, the project's containers) with their state and ports, and start / stop /
//! restart / `up -d` / `down`.
//!
//! Only names, images, states and ports are read — never a service's environment: compose files
//! and `.env` often hold database passwords, and `docker compose config` would print them resolved.
//! Every command is an argv array (no shell), and a service or container name reaches one only
//! after Docker itself reported it and it passed [`valid_name`].

use anyhow::{bail, Result};
use serde_json::Value;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Compose file names `docker compose` finds on its own, in its order of preference.
pub const COMPOSE_FILES: [&str; 4] = ["compose.yaml", "compose.yml", "docker-compose.yml", "docker-compose.yaml"];
/// Listing containers: a daemon that is still starting must not hold the panel forever.
const QUERY_TIMEOUT: Duration = Duration::from_secs(15);
/// `up -d` may pull images first.
const ACTION_TIMEOUT: Duration = Duration::from_secs(600);

/// Docker files at the root of a project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Files {
    pub compose: Option<PathBuf>,
    pub dockerfile: bool,
}

pub fn detect(root: &Path) -> Files {
    Files {
        compose: COMPOSE_FILES.iter().map(|name| root.join(name)).find(|path| path.is_file()),
        dockerfile: root.join("Dockerfile").is_file(),
    }
}

/// How to reach a compose project: run in `dir`, with `-p project` and `-f file`s when they aren't
/// what compose would pick there by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compose {
    pub dir: PathBuf,
    pub project: Option<String>,
    pub files: Vec<PathBuf>,
}

impl Compose {
    /// `compose [-p project] [-f file]…`, the start of every compose command.
    pub fn args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec!["compose".into()];
        if let Some(project) = self.project.as_ref().filter(|p| valid_name(p)) {
            args.extend(["-p".into(), project.into()]);
        }
        for file in &self.files {
            args.extend(["-f".into(), file.clone().into_os_string()]);
        }
        args
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    /// Running, health check not passed yet.
    Starting,
    /// Failing its health check, or restarting over and over.
    Unhealthy,
    Paused,
    Stopped,
    /// A compose service without a container (never started, or removed by `down`).
    NotCreated,
}

impl State {
    /// From Docker's `State` (`running`, `exited`, …) and its health (compose's `Health`, or the
    /// `Status` text `docker ps` gives: `Up 3 minutes (unhealthy)`).
    fn parse(state: &str, health: &str) -> Self {
        match state {
            "running" if health.contains("unhealthy") => State::Unhealthy,
            "running" if health.contains("starting") => State::Starting,
            "running" => State::Running,
            "restarting" => State::Unhealthy,
            "paused" => State::Paused,
            _ => State::Stopped,
        }
    }

    pub fn is_up(self) -> bool {
        matches!(self, State::Running | State::Starting | State::Unhealthy)
    }
}

/// A compose service, or a container of the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// Compose service name, or container name.
    pub name: String,
    /// Container id, when there is a container.
    pub id: Option<String>,
    pub image: String,
    pub state: State,
    /// Docker's own summary (`Up 5 minutes (healthy)`, `Exited (1) 2 hours ago`).
    pub status: String,
    /// Published ports, `host→container` (`/udp` when not TCP).
    pub ports: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Engine {
    #[default]
    Ready,
    /// No `docker` program.
    NoCli,
    /// `docker` is there but its daemon doesn't answer: the first line of what it said.
    NotRunning(String),
}

/// Everything the panel shows about one project.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Overview {
    pub root: PathBuf,
    pub files: Files,
    pub engine: Engine,
    /// The project's compose project: from its compose file, or found through the labels of its
    /// containers (a compose file elsewhere in the project, started with `-f`).
    pub compose: Option<Compose>,
    /// Compose services, or without compose the project's containers.
    pub services: Vec<Service>,
    /// Something went wrong reading the services (said without details: see [`short_error`]).
    pub error: Option<String>,
}

impl Overview {
    /// Whether the project uses Docker at all: files for it, or containers of it.
    pub fn relevant(&self) -> bool {
        self.files.compose.is_some() || self.files.dockerfile || self.compose.is_some() || !self.services.is_empty()
    }

    pub fn running(&self) -> usize {
        self.services.iter().filter(|s| s.state.is_up()).count()
    }
}

/// Folders Docker installs its CLI into without always reaching the PATH of a GUI app: Docker
/// Desktop, Homebrew, Rancher Desktop, OrbStack, snap.
fn docker_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let mut dirs = Vec::new();
    if cfg!(windows) {
        for base in ["ProgramFiles", "ProgramW6432"] {
            if let Some(dir) = std::env::var_os(base) {
                dirs.push(PathBuf::from(dir).join("Docker").join("Docker").join("resources").join("bin"));
            }
        }
    } else {
        if let Some(home) = &home {
            for sub in [".docker/bin", ".rd/bin", ".orbstack/bin"] {
                dirs.push(home.join(sub));
            }
        }
        for dir in ["/usr/local/bin", "/opt/homebrew/bin", "/Applications/Docker.app/Contents/Resources/bin", "/snap/bin"] {
            dirs.push(PathBuf::from(dir));
        }
    }
    dirs.retain(|dir| dir.is_absolute() && dir.is_dir());
    dirs
}

/// `PATH` for Docker commands: the app's own, then Docker's folders (the compose plugin and
/// credential helpers are found through it).
fn search_path() -> OsString {
    crate::process::merge_paths(&crate::process::current_path(), &docker_dirs())
}

/// The `docker` program, if installed.
pub fn program() -> Option<PathBuf> {
    crate::process::which_in("docker", &search_path())
}

/// Docker's output, or what went wrong. Killed (by its own handle) after `timeout`.
fn run(program: &Path, args: &[OsString], dir: Option<&Path>, timeout: Duration) -> Result<String, String> {
    let mut command = crate::process::command(program);
    command
        .args(args)
        .env("PATH", search_path())
        .env("DOCKER_CLI_HINTS", "false")
        .env("COMPOSE_ANSI", "never")
        .env("COMPOSE_PROGRESS", "plain")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let mut child = command.spawn().map_err(|err| err.to_string())?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let stdout = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let stderr = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("docker did not answer in time".into());
            }
            Err(err) => return Err(err.to_string()),
        }
    };
    let (stdout, stderr) = (stdout.join().unwrap_or_default(), stderr.join().unwrap_or_default());
    if status.success() {
        Ok(stdout)
    } else {
        Err(if stderr.trim().is_empty() { stdout } else { stderr })
    }
}

/// The last line of an error worth showing, at most 240 characters. A line that mentions the
/// environment is replaced by a generic note: compose quotes the offending text in some errors
/// (an interpolation it could not read), and that text can be a password.
pub fn short_error(text: &str) -> String {
    let line = text.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or("docker failed");
    let lower = line.to_lowercase();
    if ["environment", "interpolat", "password", "secret", "token"].iter().any(|word| lower.contains(word)) {
        return "docker compose could not read the compose file".into();
    }
    let line = line.strip_prefix("Error response from daemon: ").unwrap_or(line);
    match line.char_indices().nth(240) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_string(),
    }
}

/// JSON output of `--format json`: one object per line (current Docker), or one array (compose
/// before v2.21).
fn json_rows(text: &str) -> Vec<Value> {
    let text = text.trim();
    if text.starts_with('[') {
        return serde_json::from_str::<Vec<Value>>(text).unwrap_or_default();
    }
    text.lines().filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok()).filter(Value::is_object).collect()
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

/// Sorted, without the IPv4 / IPv6 duplicates Docker lists for every port.
fn tidy_ports(mut ports: Vec<String>) -> Vec<String> {
    ports.sort_by_key(|p| (p.split('→').next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(u32::MAX), p.clone()));
    ports.dedup();
    ports
}

fn port_label(host: &str, container: &str, protocol: &str) -> String {
    if protocol.is_empty() || protocol == "tcp" {
        format!("{host}→{container}")
    } else {
        format!("{host}→{container}/{protocol}")
    }
}

/// Rows of `docker compose ps --all --format json`.
pub fn parse_compose_ps(output: &str) -> Vec<Service> {
    json_rows(output)
        .iter()
        .filter_map(|row| {
            let name = text(row, "Service");
            if !valid_name(&name) {
                return None;
            }
            let ports = row["Publishers"]
                .as_array()
                .map(|list| {
                    list.iter()
                        .filter(|p| p["PublishedPort"].as_u64().unwrap_or(0) > 0)
                        .map(|p| {
                            port_label(
                                &p["PublishedPort"].as_u64().unwrap_or(0).to_string(),
                                &p["TargetPort"].as_u64().unwrap_or(0).to_string(),
                                p["Protocol"].as_str().unwrap_or("tcp"),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Service {
                name,
                id: Some(text(row, "ID")).filter(|id| valid_name(id)),
                image: text(row, "Image"),
                state: State::parse(&text(row, "State"), &text(row, "Health")),
                status: text(row, "Status"),
                ports: tidy_ports(ports),
            })
        })
        .collect()
}

/// `0.0.0.0:3306->3306/tcp, :::3306->3306/tcp, 33060/tcp` → `["3306→3306"]`: published ports only.
pub fn parse_ports(text: &str) -> Vec<String> {
    let ports = text
        .split(',')
        .filter_map(|entry| {
            let (host, container) = entry.trim().split_once("->")?;
            let host_port = host.rsplit(':').next()?.trim();
            let (container_port, protocol) = container.split_once('/').unwrap_or((container, "tcp"));
            (!host_port.is_empty()).then(|| port_label(host_port, container_port.trim(), protocol.trim()))
        })
        .collect();
    tidy_ports(ports)
}

/// A container as `docker ps --all --format json` lists it, with the compose labels that tie it
/// to a project folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    pub service: Service,
    pub labels: Labels,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Labels {
    pub project: Option<String>,
    pub working_dir: Option<PathBuf>,
    pub config_files: Vec<PathBuf>,
    pub service: Option<String>,
}

/// The compose labels out of `docker ps`'s `k=v,k=v` label text. Values with a comma
/// (`config_files` of several files) continue in the pieces that have no `=` of their own.
fn parse_labels(text: &str) -> Labels {
    let mut pairs: Vec<(String, String)> = Vec::new();
    // A piece that is a path (`/work/dev.yml`, `C:\\work\\dev.yml`) continues the value before it,
    // even with a `=` in it; label keys never start like that (`desktop.docker.io/ports/…` does not).
    let is_path = |piece: &str| piece.starts_with(['/', '\\']) || piece.get(1..3).is_some_and(|s| s == ":\\" || s == ":/");
    for piece in text.split(',') {
        match piece.split_once('=') {
            Some((key, value)) if !is_path(piece) => pairs.push((key.to_string(), value.to_string())),
            _ => {
                if let Some((_, value)) = pairs.last_mut() {
                    value.push(',');
                    value.push_str(piece);
                }
            }
        }
    }
    let get = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).filter(|v| !v.is_empty());
    Labels {
        project: get("com.docker.compose.project"),
        working_dir: get("com.docker.compose.project.working_dir").map(PathBuf::from),
        config_files: get("com.docker.compose.project.config_files").map(|v| v.split(',').map(PathBuf::from).collect()).unwrap_or_default(),
        service: get("com.docker.compose.service"),
    }
}

/// Rows of `docker ps --all --format json`.
pub fn parse_docker_ps(output: &str) -> Vec<Container> {
    json_rows(output)
        .iter()
        .filter_map(|row| {
            let name = text(row, "Names").split(',').next().unwrap_or_default().to_string();
            if !valid_name(&name) {
                return None;
            }
            let status = text(row, "Status");
            Some(Container {
                service: Service {
                    id: Some(text(row, "ID")).filter(|id| valid_name(id)),
                    image: text(row, "Image"),
                    state: State::parse(&text(row, "State"), &status),
                    status,
                    ports: parse_ports(&text(row, "Ports")),
                    name,
                },
                labels: parse_labels(&text(row, "Labels")),
            })
        })
        .collect()
}

/// A lowercase name Docker would derive from a folder name (`My App` → `myapp`), to recognize
/// images and containers named after the project.
fn slug(name: &str) -> String {
    name.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect()
}

/// Whether a container without compose labels belongs to the project in `root`: its image or its
/// name is the project's folder name (`docker build -t my-app .`, `docker run --name my-app`).
fn named_after(container: &Container, root: &Path) -> bool {
    let Some(project) = root.file_name().map(|n| slug(&n.to_string_lossy())).filter(|s| s.len() >= 2) else { return false };
    let image = container.service.image.rsplit('/').next().unwrap_or_default();
    let image = image.split([':', '@']).next().unwrap_or_default();
    image == project || slug(&container.service.name) == project || container.service.name.starts_with(&format!("{project}-"))
}

/// The compose project the project in `root` runs, found through its containers' labels.
fn compose_from_labels(containers: &[Container], root: &Path, files: &Files) -> Option<Compose> {
    // Compose records the resolved folder (`/private/tmp/…` for `/tmp/…` on macOS).
    let real = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let real_root = real(root);
    let dir_of = |c: &Container| c.labels.working_dir.as_deref().map(real);
    let inside = |c: &&Container| dir_of(c).is_some_and(|dir| dir.starts_with(&real_root));
    // A project run from the root itself (the compose file there, maybe with `-p`) comes first.
    let found = containers
        .iter()
        .filter(inside)
        .find(|c| dir_of(c).as_deref() == Some(real_root.as_path()))
        .or_else(|| containers.iter().find(inside))?;
    let dir = if dir_of(found).as_deref() == Some(real_root.as_path()) { root.to_path_buf() } else { found.labels.working_dir.clone()? };
    let project = found.labels.project.clone().filter(|p| valid_name(p));
    let at_root = dir == root && files.compose.is_some();
    // Its files, unless compose would find them there by itself anyway.
    let default = |f: &PathBuf| f.parent() == Some(dir.as_path()) && f.file_name().is_some_and(|n| COMPOSE_FILES.iter().any(|c| n == *c));
    let files = if at_root || found.labels.config_files.iter().all(default) {
        Vec::new()
    } else {
        found.labels.config_files.iter().filter(|f| f.is_absolute() && f.is_file()).cloned().collect()
    };
    Some(Compose { dir, project, files })
}

/// Compose services by name, merged with the containers compose
/// reported; services without a container are listed as [`State::NotCreated`].
fn merge_services(names: &[String], mut running: Vec<Service>) -> Vec<Service> {
    let mut names: Vec<&String> = names.iter().filter(|n| valid_name(n)).collect();
    names.sort();
    names.dedup();
    running.sort_by(|a, b| a.name.cmp(&b.name));
    let mut services = Vec::new();
    for name in names {
        let mine: Vec<Service> = running.iter().filter(|s| &s.name == name).cloned().collect();
        running.retain(|s| &s.name != name);
        if mine.is_empty() {
            services.push(Service {
                name: name.clone(),
                id: None,
                image: String::new(),
                state: State::NotCreated,
                status: String::new(),
                ports: Vec::new(),
            });
        } else {
            services.extend(mine);
        }
    }
    services.extend(running);
    services
}

/// What Docker says about the project in `root`. Blocking; run it off the UI thread.
pub fn overview(root: &Path) -> Overview {
    let files = detect(root);
    let mut overview = Overview { root: root.to_path_buf(), files: files.clone(), ..Default::default() };
    let Some(docker) = program() else {
        overview.engine = Engine::NoCli;
        return overview;
    };
    let all = match run(&docker, &["ps".into(), "--all".into(), "--format".into(), "json".into()], None, QUERY_TIMEOUT) {
        Ok(output) => parse_docker_ps(&output),
        Err(err) => {
            overview.engine = Engine::NotRunning(short_error(&err));
            if files.compose.is_some() {
                overview.compose = Some(Compose { dir: root.to_path_buf(), project: None, files: Vec::new() });
            }
            return overview;
        }
    };
    let compose = match compose_from_labels(&all, root, &files) {
        Some(found) if files.compose.is_none() || found.dir == root => Some(found),
        _ => files.compose.as_ref().map(|_| Compose { dir: root.to_path_buf(), project: None, files: Vec::new() }),
    };
    let Some(compose) = compose else {
        overview.services =
            all.into_iter().filter(|c| c.labels.project.is_none() && named_after(c, root)).map(|c| c.service).take(50).collect();
        return overview;
    };
    let mut ps_args = compose.args();
    ps_args.extend(["ps".into(), "--all".into(), "--format".into(), "json".into()]);
    let listed = run(&docker, &ps_args, Some(&compose.dir), QUERY_TIMEOUT);
    let mut names_args = compose.args();
    names_args.extend(["config".into(), "--services".into()]);
    // Service names only: `config` without `--services` would print the environment resolved.
    let names: Vec<String> = run(&docker, &names_args, Some(&compose.dir), QUERY_TIMEOUT)
        .map(|out| out.lines().map(str::trim).filter(|n| valid_name(n)).map(str::to_string).collect())
        .unwrap_or_default();
    match listed {
        Ok(output) => overview.services = merge_services(&names, parse_compose_ps(&output)),
        Err(err) => {
            overview.error = Some(short_error(&err));
            overview.services = merge_services(&names, Vec::new());
        }
    }
    overview.compose = Some(compose);
    overview
}

/// A name Docker reported (service, container, project, container id), safe as one argument:
/// letters, digits, `.`, `_`, `-`, not starting with `-` (never read as an option).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('-')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
    /// `up -d`: creates what is missing and starts it.
    Up,
    /// `down`: stops and removes the containers and networks. Volumes are kept (no `-v`).
    Down,
}

/// The argv (without the program) of `action` on a compose project, or one of its services.
pub fn compose_args(compose: &Compose, action: Action, service: Option<&str>) -> Result<Vec<OsString>> {
    if service.is_some_and(|s| !valid_name(s)) {
        bail!("invalid service name");
    }
    let mut args = compose.args();
    let verb: &[&str] = match action {
        Action::Start => &["start"],
        Action::Stop => &["stop"],
        Action::Restart => &["restart"],
        Action::Up => &["up", "-d"],
        Action::Down if service.is_some() => bail!("down applies to the whole project"),
        Action::Down => &["down"],
    };
    args.extend(verb.iter().map(OsString::from));
    args.extend(service.map(OsString::from));
    Ok(args)
}

/// The argv (without the program) of `action` on a container that isn't part of compose.
pub fn container_args(id: &str, action: Action) -> Result<Vec<OsString>> {
    if !valid_name(id) {
        bail!("invalid container id");
    }
    let verb = match action {
        Action::Start | Action::Up => "start",
        Action::Stop => "stop",
        Action::Restart => "restart",
        Action::Down => bail!("down applies to compose projects"),
    };
    Ok(vec![verb.into(), id.into()])
}

/// Runs `docker <args>` in `dir`. Blocking; run it off the UI thread.
pub fn execute(args: &[OsString], dir: &Path) -> Result<()> {
    let Some(docker) = program() else { bail!("docker was not found") };
    run(&docker, args, Some(dir), ACTION_TIMEOUT).map(|_| ()).map_err(|err| anyhow::anyhow!(short_error(&err)))
}

/// The command line a terminal runs to follow a service's logs (`docker compose logs -f db`) or a
/// container's (`docker logs -f web`), as argv.
pub fn logs_argv(compose: Option<&Compose>, name: &str) -> Result<Vec<String>> {
    if !valid_name(name) {
        bail!("invalid name");
    }
    let mut argv = vec!["docker".to_string()];
    match compose {
        Some(compose) => {
            argv.extend(compose.args().iter().map(|a| a.to_string_lossy().into_owned()));
            argv.extend(["logs", "-f", "--tail", "200", name].map(String::from));
        }
        None => argv.extend(["logs", "-f", "--tail", "200", name].map(String::from)),
    }
    Ok(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agentty-docker-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_compose_files_and_dockerfiles() {
        let dir = temp("detect");
        assert_eq!(detect(&dir), Files::default());
        std::fs::write(dir.join("Dockerfile"), "FROM scratch\n").unwrap();
        std::fs::write(dir.join("docker-compose.yml"), "services: {}\n").unwrap();
        assert_eq!(detect(&dir), Files { compose: Some(dir.join("docker-compose.yml")), dockerfile: true });
        // compose.yaml is what compose itself prefers.
        std::fs::write(dir.join("compose.yaml"), "services: {}\n").unwrap();
        assert_eq!(detect(&dir).compose, Some(dir.join("compose.yaml")));
        // A folder named like a compose file is not one.
        let other = temp("detect-dir");
        std::fs::create_dir_all(other.join("compose.yml")).unwrap();
        assert_eq!(detect(&other).compose, None);
        std::fs::remove_dir_all(dir).ok();
        std::fs::remove_dir_all(other).ok();
    }

    #[test]
    fn parses_compose_ps_lines_and_arrays() {
        let lines = concat!(
            r#"{"ID":"a1b2c3","Name":"shop-db-1","Service":"db","Image":"mysql:8.4","State":"running","Health":"healthy","Status":"Up 3 minutes (healthy)","Publishers":[{"URL":"0.0.0.0","TargetPort":3306,"PublishedPort":3306,"Protocol":"tcp"},{"URL":"::","TargetPort":3306,"PublishedPort":3306,"Protocol":"tcp"},{"URL":"","TargetPort":33060,"PublishedPort":0,"Protocol":"tcp"}]}"#,
            "\n",
            r#"{"ID":"d4e5f6","Name":"shop-cache-1","Service":"cache","Image":"redis:7","State":"running","Health":"unhealthy","Status":"Up 1 minute (unhealthy)","Publishers":[{"URL":"0.0.0.0","TargetPort":6379,"PublishedPort":16379,"Protocol":"tcp"}]}"#,
            "\n",
            r#"{"ID":"778899","Name":"shop-worker-1","Service":"worker","Image":"shop-worker","State":"exited","Health":"","Status":"Exited (1) 2 minutes ago","Publishers":null}"#,
            "\n"
        );
        let services = parse_compose_ps(lines);
        assert_eq!(services.len(), 3);
        assert_eq!(services[0].name, "db");
        assert_eq!(services[0].image, "mysql:8.4");
        assert_eq!(services[0].state, State::Running);
        assert_eq!(services[0].ports, vec!["3306→3306"]);
        assert_eq!(services[1].state, State::Unhealthy);
        assert_eq!(services[1].ports, vec!["16379→6379"]);
        assert_eq!(services[2].state, State::Stopped);
        assert!(services[2].ports.is_empty());
        // Compose before v2.21 printed one array.
        let array = r#"[{"ID":"a1","Service":"db","Image":"postgres:16","State":"restarting","Publishers":[]}]"#;
        let services = parse_compose_ps(array);
        assert_eq!((services[0].name.as_str(), services[0].state), ("db", State::Unhealthy));
        assert!(parse_compose_ps("").is_empty());
        assert!(parse_compose_ps("not json\n").is_empty());
    }

    #[test]
    fn never_keeps_a_name_that_could_be_an_option_or_shell_text() {
        let rows = concat!(
            r#"{"ID":"1","Service":"--privileged","Image":"x","State":"running"}"#,
            "\n",
            r#"{"ID":"2","Service":"db; rm -rf ~","Image":"x","State":"running"}"#,
            "\n",
            r#"{"ID":"3","Service":"ok_name-1.2","Image":"x","State":"running"}"#
        );
        let names: Vec<String> = parse_compose_ps(rows).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["ok_name-1.2"]);
        assert!(!valid_name(""));
        assert!(!valid_name("-p"));
        assert!(!valid_name("a b"));
        assert!(!valid_name("$(id)"));
        assert!(valid_name("redis"));
    }

    #[test]
    fn parses_docker_ps_rows_and_their_compose_labels() {
        let row = r#"{"Command":"\"docker-entrypoint.s…\"","ID":"0123456789ab","Image":"postgres:16","Labels":"desktop.docker.io/ports/5432/tcp=:5432,com.docker.compose.project=shop,com.docker.compose.project.working_dir=/work/shop/infra,com.docker.compose.project.config_files=/work/shop/infra/base.yml,/work/shop/infra/dev.yml,com.docker.compose.service=db,desktop.docker.io/ports.scheme=v2","Names":"shop-db-1","Ports":"0.0.0.0:5432->5432/tcp, [::]:5432->5432/tcp, 0.0.0.0:5353->53/udp","State":"running","Status":"Up 2 hours (health: starting)"}"#;
        let containers = parse_docker_ps(row);
        assert_eq!(containers.len(), 1);
        let c = &containers[0];
        assert_eq!(c.service.name, "shop-db-1");
        assert_eq!(c.service.id.as_deref(), Some("0123456789ab"));
        assert_eq!(c.service.state, State::Starting);
        assert_eq!(c.service.ports, vec!["5353→53/udp", "5432→5432"]);
        assert_eq!(c.labels.project.as_deref(), Some("shop"));
        assert_eq!(c.labels.working_dir.as_deref(), Some(Path::new("/work/shop/infra")));
        assert_eq!(c.labels.config_files, vec![PathBuf::from("/work/shop/infra/base.yml"), PathBuf::from("/work/shop/infra/dev.yml")]);
        assert_eq!(c.labels.service.as_deref(), Some("db"));
        assert_eq!(parse_ports("6379/tcp"), Vec::<String>::new());
        assert_eq!(parse_ports(""), Vec::<String>::new());
    }

    #[test]
    fn finds_the_compose_project_of_a_folder_through_labels() {
        let container = |name: &str, project: &str, dir: &str, files: &str| Container {
            service: Service {
                name: name.into(),
                id: Some("1".into()),
                image: "x".into(),
                state: State::Running,
                status: String::new(),
                ports: Vec::new(),
            },
            labels: Labels {
                project: Some(project.into()),
                working_dir: Some(dir.into()),
                config_files: files.split(',').filter(|f| !f.is_empty()).map(PathBuf::from).collect(),
                service: Some("db".into()),
            },
        };
        let root = Path::new("/work/shop");
        let others = container("blog-db-1", "blog", "/work/blog", "/work/blog/compose.yaml");
        let nested = container("shop-db-1", "shop", "/work/shop/infra", "/work/shop/infra/compose.yaml");
        let found = compose_from_labels(&[others.clone(), nested.clone()], root, &Files::default()).unwrap();
        assert_eq!(found, Compose { dir: "/work/shop/infra".into(), project: Some("shop".into()), files: Vec::new() });
        // Another project's containers never count, even with a name that starts the same.
        let lookalike = container("shopper-db-1", "shopper", "/work/shopper", "");
        assert_eq!(compose_from_labels(&[others, lookalike], root, &Files::default()), None);
        // Started from the root with `-p`: that project name is kept.
        let at_root = container("custom-db-1", "custom", "/work/shop", "/work/shop/compose.yaml");
        let files = Files { compose: Some("/work/shop/compose.yaml".into()), dockerfile: false };
        let found = compose_from_labels(&[nested, at_root], root, &files).unwrap();
        assert_eq!(found, Compose { dir: root.into(), project: Some("custom".into()), files: Vec::new() });
    }

    #[test]
    fn recognizes_containers_named_after_the_project() {
        let container = |name: &str, image: &str| Container {
            service: Service {
                name: name.into(),
                id: None,
                image: image.into(),
                state: State::Running,
                status: String::new(),
                ports: Vec::new(),
            },
            labels: Labels::default(),
        };
        let root = Path::new("/work/My-App");
        assert!(named_after(&container("zen_turing", "my-app:latest"), root));
        assert!(named_after(&container("zen_turing", "ghcr.io/me/my-app@sha256:abc"), root));
        assert!(named_after(&container("my-app-web", "nginx"), root));
        assert!(!named_after(&container("zen_turing", "my-apple"), root));
        assert!(!named_after(&container("other", "nginx"), root));
    }

    #[test]
    fn lists_every_compose_service_even_without_a_container() {
        let names = ["db".to_string(), "cache".to_string()];
        let db = parse_compose_ps(r#"{"ID":"1","Service":"db","Image":"mysql","State":"running"}"#);
        let services = merge_services(&names, db);
        assert_eq!(
            services.iter().map(|s| (s.name.as_str(), s.state)).collect::<Vec<_>>(),
            [("cache", State::NotCreated), ("db", State::Running)]
        );
    }

    #[test]
    fn commands_are_argv_and_down_never_removes_volumes() {
        let compose = Compose { dir: "/work/shop".into(), project: Some("shop".into()), files: vec!["/work/shop/dev.yml".into()] };
        let args = |action, service| {
            compose_args(&compose, action, service).map(|a| a.iter().map(|s| s.to_string_lossy().into_owned()).collect::<Vec<_>>())
        };
        assert_eq!(args(Action::Up, None).unwrap(), ["compose", "-p", "shop", "-f", "/work/shop/dev.yml", "up", "-d"]);
        assert_eq!(args(Action::Restart, Some("db")).unwrap(), ["compose", "-p", "shop", "-f", "/work/shop/dev.yml", "restart", "db"]);
        let down = args(Action::Down, None).unwrap();
        assert_eq!(down.last().map(String::as_str), Some("down"));
        assert!(!down.iter().any(|a| a == "-v" || a == "--volumes"));
        assert!(args(Action::Down, Some("db")).is_err());
        assert!(args(Action::Stop, Some("db && reboot")).is_err());
        assert!(container_args("--rm", Action::Stop).is_err());
        assert_eq!(container_args("abc123", Action::Stop).unwrap(), [OsString::from("stop"), OsString::from("abc123")]);
        assert_eq!(logs_argv(None, "web").unwrap(), ["docker", "logs", "-f", "--tail", "200", "web"]);
        assert!(logs_argv(Some(&compose), "db").unwrap().ends_with(&[
            "logs".into(),
            "-f".into(),
            "--tail".into(),
            "200".into(),
            "db".into()
        ]));
        // A project name that could be an option is left out rather than passed on.
        let odd = Compose { dir: "/w".into(), project: Some("-x".into()), files: Vec::new() };
        assert_eq!(odd.args(), [OsString::from("compose")]);
    }

    #[test]
    fn errors_never_repeat_environment_values() {
        assert_eq!(
            short_error("Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?\n"),
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?"
        );
        let leaked = "invalid interpolation format for services.db.environment.MYSQL_ROOT_PASSWORD: \"not_a_real_password${\"";
        assert!(!short_error(leaked).contains("not_a_real_password"));
        assert_eq!(short_error("Error response from daemon: No such container: x"), "No such container: x");
        assert_eq!(short_error(""), "docker failed");
    }
}
