//! API connectors: plain HTTP APIs exposed to Claude Code / Codex as MCP tools.
//!
//! Security model:
//! - The connector definition (name, base URL, auth style, endpoints) lives in
//!   `<data dir>/connectors.json` and contains **no secrets**.
//! - The credential is stored in the macOS Keychain (encrypted by the OS, per-user ACL) and read
//!   only by the `mcp-connector` process at request time. It is never written to agent config
//!   files, logs or tool results.
//! - Requests are confined to the connector's base URL: paths must be relative, `..` segments are
//!   rejected, redirects are not followed (so credentials can't leak to other hosts), HTTPS is
//!   required except for localhost, and responses are size- and time-limited.

use crate::fsutil;
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

const KEYCHAIN_SERVICE: &str = "run.agentty.connector";
const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
const MAX_TOOL_TEXT: usize = 100_000;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Auth {
    None,
    /// `Authorization: Bearer <secret>`
    Bearer,
    /// `<name>: <secret>`
    Header {
        name: String,
    },
    /// `?<name>=<secret>`
    Query {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub name: String,
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connector {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub auth: Auth,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub endpoints: Vec<Endpoint>,
    /// Also expose a generic `request` tool for any path under the base URL.
    #[serde(default = "default_true")]
    pub allow_any_path: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ConnectorStore {
    pub connectors: Vec<Connector>,
}

impl ConnectorStore {
    fn path() -> PathBuf {
        fsutil::data_dir().join("connectors.json")
    }

    pub fn load() -> Self {
        std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Connector> {
        self.connectors.iter().find(|c| c.id == id)
    }

    /// Adds or replaces a connector and stores its secret. The secret is required for auth types
    /// other than `None` when the connector is new.
    pub fn upsert(&mut self, connector: Connector, secret: Option<&str>) -> Result<()> {
        connector.validate()?;
        let exists = self.get(&connector.id).is_some();
        match (&connector.auth, secret.filter(|s| !s.is_empty())) {
            (Auth::None, _) => {
                let _ = secrets::delete(&connector.id);
            }
            (_, Some(secret)) => secrets::store(&connector.id, secret)?,
            (_, None) => ensure!(exists && secrets::load(&connector.id).is_ok(), "an API key is required"),
        }
        self.connectors.retain(|c| c.id != connector.id);
        self.connectors.push(connector);
        self.save()
    }

    pub fn remove(&mut self, id: &str) -> Result<()> {
        let _ = secrets::delete(id);
        self.connectors.retain(|c| c.id != id);
        self.save()
    }
}

/// Parses `METHOD /path [tool_name] [description…]` entries separated by `;` or newlines.
pub fn parse_endpoints(text: &str) -> Result<Vec<Endpoint>> {
    let mut endpoints = Vec::new();
    for entry in text.split([';', '\n']).map(str::trim).filter(|e| !e.is_empty()) {
        let mut parts = entry.split_whitespace();
        let method_name = method(parts.next().unwrap_or_default())?.to_string();
        let path = safe_path(parts.next().context("each endpoint needs a path")?)?.to_string();
        let rest: Vec<&str> = parts.collect();
        let is_name = |w: &&str| w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        let (name, description) = match rest.first() {
            Some(first) if is_name(first) => (first.to_string(), rest[1..].join(" ")),
            _ => {
                let derived: String = format!("{}_{}", method_name.to_lowercase(), path)
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .collect::<String>()
                    .split('_')
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("_");
                (derived, rest.join(" "))
            }
        };
        endpoints.push(Endpoint { name, method: method_name, path, description });
    }
    Ok(endpoints)
}

pub fn slugify(name: &str) -> String {
    let slug: String = name.trim().to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    slug.split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-")
}

impl Connector {
    pub fn new(name: &str, base_url: &str, auth: Auth) -> Self {
        let slug = slugify(name);
        Self {
            id: slug.clone(),
            name: slug,
            base_url: base_url.trim().trim_end_matches('/').to_string(),
            auth,
            description: String::new(),
            endpoints: Vec::new(),
            allow_any_path: true,
        }
    }

    /// MCP server name used when registering with an agent.
    pub fn server_name(&self) -> String {
        format!("agentty-{}", self.id)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.id.is_empty() && self.id.len() <= 48 && self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "name must contain letters or numbers"
        );
        let url = url::Url::parse(&self.base_url).context("base URL is not a valid URL")?;
        let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        ensure!(
            url.scheme() == "https" || (url.scheme() == "http" && local),
            "base URL must use https (http is only allowed for localhost)"
        );
        ensure!(url.username().is_empty() && url.password().is_none(), "put credentials in the API key field, not in the URL");
        ensure!(url.query().is_none() && url.fragment().is_none(), "base URL must not contain a query or fragment");
        if let Auth::Header { name } | Auth::Query { name } = &self.auth {
            ensure!(
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_".contains(c)),
                "invalid header or parameter name"
            );
        }
        for endpoint in &self.endpoints {
            ensure!(
                !endpoint.name.is_empty() && endpoint.name.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)),
                "invalid endpoint name {}",
                endpoint.name
            );
            method(&endpoint.method)?;
            safe_path(&endpoint.path)?;
        }
        Ok(())
    }

    /// Resolves a relative path against the base URL, refusing anything that escapes it.
    pub fn resolve(&self, path: &str) -> Result<url::Url> {
        let path = safe_path(path)?;
        let base = url::Url::parse(&self.base_url)?;
        let url = url::Url::parse(&format!("{}{}", self.base_url, path))?;
        ensure!(
            url.scheme() == base.scheme()
                && url.host_str() == base.host_str()
                && url.port_or_known_default() == base.port_or_known_default(),
            "request must stay on the connector's host"
        );
        ensure!(url.path().starts_with(base.path().trim_end_matches('/')), "request must stay under the base URL");
        Ok(url)
    }
}

fn method(method: &str) -> Result<&'static str> {
    Ok(match method.to_ascii_uppercase().as_str() {
        "GET" => "GET",
        "POST" => "POST",
        "PUT" => "PUT",
        "PATCH" => "PATCH",
        "DELETE" => "DELETE",
        other => bail!("unsupported HTTP method {other}"),
    })
}

fn safe_path(path: &str) -> Result<&str> {
    ensure!(path.starts_with('/'), "path must start with /");
    ensure!(!path.contains("://") && !path.starts_with("//"), "path must be relative to the base URL");
    ensure!(!path.contains('\\') && !path.chars().any(char::is_control), "path contains invalid characters");
    let decoded = path.replace("%2e", ".").replace("%2E", ".").replace("%2f", "/").replace("%2F", "/");
    ensure!(!decoded.split(['/', '?']).any(|segment| segment == ".." || segment == "."), "path must not contain . or .. segments");
    Ok(path)
}

pub mod secrets {
    use super::KEYCHAIN_SERVICE;
    use anyhow::Result;

    #[cfg(target_os = "macos")]
    pub fn store(id: &str, secret: &str) -> Result<()> {
        security_framework::passwords::set_generic_password(KEYCHAIN_SERVICE, id, secret.as_bytes())?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub fn load(id: &str) -> Result<String> {
        let bytes = security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, id)?;
        Ok(String::from_utf8(bytes)?)
    }

    #[cfg(target_os = "macos")]
    pub fn delete(id: &str) -> Result<()> {
        security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, id)?;
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn store(_: &str, _: &str) -> Result<()> {
        anyhow::bail!("secure credential storage is only implemented for macOS")
    }

    #[cfg(not(target_os = "macos"))]
    pub fn load(_: &str) -> Result<String> {
        anyhow::bail!("secure credential storage is only implemented for macOS")
    }

    #[cfg(not(target_os = "macos"))]
    pub fn delete(_: &str) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------------------------

pub struct Request {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Option<Value>,
}

/// Performs a request with the connector's credential. Returns (status, body text).
pub fn execute(connector: &Connector, request: &Request) -> Result<(u16, String)> {
    let secret = match connector.auth {
        Auth::None => None,
        _ => Some(secrets::load(&connector.id).context("API key not found in the Keychain")?),
    };
    let redact = |text: String| match &secret {
        Some(secret) if secret.len() >= 4 => text.replace(secret.as_str(), "***"),
        _ => text,
    };

    let mut url = connector.resolve(&request.path)?;
    {
        let mut pairs = url.query_pairs_mut();
        for (k, v) in &request.query {
            pairs.append_pair(k, v);
        }
        if let (Auth::Query { name }, Some(secret)) = (&connector.auth, &secret) {
            pairs.append_pair(name, secret);
        }
    }
    let agent = crate::http::agent_builder().timeout(REQUEST_TIMEOUT).redirects(0).build();
    let mut req = agent.request(method(&request.method)?, url.as_str()).set("User-Agent", concat!("Agentty/", env!("CARGO_PKG_VERSION")));
    match (&connector.auth, &secret) {
        (Auth::Bearer, Some(secret)) => req = req.set("Authorization", &format!("Bearer {secret}")),
        (Auth::Header { name }, Some(secret)) => req = req.set(name, secret),
        _ => {}
    }
    let response = match &request.body {
        Some(body) => req.send_json(body.clone()),
        None => req.call(),
    };
    let response = match response {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(err) => bail!(redact(format!("request failed: {err}"))),
    };
    let status = response.status();
    let mut body = String::new();
    response.into_reader().take(MAX_RESPONSE_BYTES).read_to_string(&mut body).context("could not read the response body")?;
    Ok((status, redact(body)))
}

// ---------------------------------------------------------------------------------------------
// MCP server (stdio, newline-delimited JSON-RPC)
// ---------------------------------------------------------------------------------------------

fn tools(connector: &Connector) -> Vec<Value> {
    let mut tools: Vec<Value> = connector
        .endpoints
        .iter()
        .map(|e| {
            json!({
                "name": e.name,
                "description": format!("{} {} {}", e.method.to_uppercase(), e.path, e.description).trim().to_string(),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pathParams": { "type": "object", "description": "Values for {placeholders} in the path", "additionalProperties": { "type": "string" } },
                        "query": { "type": "object", "additionalProperties": { "type": "string" } },
                        "body": { "type": "object", "description": "JSON request body" }
                    }
                }
            })
        })
        .collect();
    if connector.allow_any_path {
        tools.push(json!({
            "name": "request",
            "description": format!("Call the {} API ({}). {}", connector.name, connector.base_url, connector.description).trim().to_string(),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "method": { "type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE"] },
                    "path": { "type": "string", "description": "Path relative to the base URL, starting with /" },
                    "query": { "type": "object", "additionalProperties": { "type": "string" } },
                    "body": { "type": "object", "description": "JSON request body" }
                },
                "required": ["method", "path"]
            }
        }));
    }
    tools
}

fn string_map(value: &Value) -> Vec<(String, String)> {
    value
        .as_object()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect())
        .unwrap_or_default()
}

fn call_tool(connector: &Connector, name: &str, args: &Value) -> Result<(u16, String)> {
    let request = if name == "request" && connector.allow_any_path {
        Request {
            method: args["method"].as_str().unwrap_or("GET").to_string(),
            path: args["path"].as_str().context("path is required")?.to_string(),
            query: string_map(&args["query"]),
            body: args.get("body").filter(|b| !b.is_null()).cloned(),
        }
    } else {
        let endpoint = connector.endpoints.iter().find(|e| e.name == name).context("unknown tool")?;
        let mut path = endpoint.path.clone();
        for (key, value) in string_map(&args["pathParams"]) {
            let encoded: String = url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
            path = path.replace(&format!("{{{key}}}"), &encoded);
        }
        ensure!(!path.contains('{'), "missing path parameters");
        Request {
            method: endpoint.method.clone(),
            path,
            query: string_map(&args["query"]),
            body: args.get("body").filter(|b| !b.is_null()).cloned(),
        }
    };
    execute(connector, &request)
}

fn handle(connector: &Connector, message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let result = match message["method"].as_str().unwrap_or("") {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": connector.server_name(), "version": env!("CARGO_PKG_VERSION") }
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools(connector) })),
        "tools/call" => {
            let name = message["params"]["name"].as_str().unwrap_or("");
            let text = match call_tool(connector, name, &message["params"]["arguments"]) {
                Ok((status, body)) => {
                    let mut body = body;
                    if body.len() > MAX_TOOL_TEXT {
                        let cut = (0..=MAX_TOOL_TEXT).rev().find(|i| body.is_char_boundary(*i)).unwrap_or(0);
                        body.truncate(cut);
                        body.push_str("\n…(truncated)");
                    }
                    Ok((format!("HTTP {status}\n{body}"), !(200..300).contains(&status)))
                }
                Err(err) => Ok((format!("Error: {err:#}"), true)),
            };
            text.map(|(text, is_error): (String, bool)| json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
        }
        other => Err(json!({ "code": -32601, "message": format!("method not found: {other}") })),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    })
}

/// Runs the MCP server for one connector until stdin closes.
pub fn serve(id: &str) -> Result<()> {
    let store = ConnectorStore::load();
    let connector = store.get(id).cloned().with_context(|| format!("connector {id} not found"))?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(&connector, &message),
            Err(_) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "parse error" } })),
        };
        if let Some(reply) = reply {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connector() -> Connector {
        let mut c = Connector::new("My API", "https://api.example.com/v1", Auth::Bearer);
        c.endpoints.push(Endpoint {
            name: "get_user".into(),
            method: "GET".into(),
            path: "/users/{id}".into(),
            description: "Fetch".into(),
        });
        c
    }

    #[test]
    fn parses_endpoint_lines() {
        let endpoints = parse_endpoints("GET /users/{id} get_user Fetch a user; post /issues\nDELETE /items/{id}").unwrap();
        assert_eq!(
            endpoints[0],
            Endpoint { name: "get_user".into(), method: "GET".into(), path: "/users/{id}".into(), description: "Fetch a user".into() }
        );
        assert_eq!(endpoints[1].name, "post_issues");
        assert_eq!(endpoints[2].name, "delete_items_id");
        assert!(parse_endpoints("TRACE /x").is_err());
        assert!(parse_endpoints("GET ../x").is_err());
    }

    #[test]
    fn slugs_and_validation() {
        assert_eq!(slugify("  My API!! v2 "), "my-api-v2");
        assert!(connector().validate().is_ok());
        assert!(Connector::new("x", "http://api.example.com", Auth::None).validate().is_err(), "plain http");
        assert!(Connector::new("x", "http://localhost:8080", Auth::None).validate().is_ok());
        assert!(Connector::new("x", "https://user:pw@api.example.com", Auth::None).validate().is_err(), "credentials in URL");
        assert!(Connector::new("x", "https://api.example.com?key=1", Auth::None).validate().is_err());
    }

    #[test]
    fn requests_stay_under_the_base_url() {
        let c = connector();
        assert_eq!(c.resolve("/users/1?x=2").unwrap().as_str(), "https://api.example.com/v1/users/1?x=2");
        for bad in ["users", "//evil.com/x", "/../admin", "/a/%2e%2e/b", "https://evil.com", "/a\\b", "/x\n"] {
            assert!(c.resolve(bad).is_err(), "{bad} should be rejected");
        }
        // `@` inside a path is just a path character; the host cannot change.
        assert_eq!(c.resolve("/@evil.com").unwrap().host_str(), Some("api.example.com"));
    }

    /// Touches the real login Keychain; run with `cargo test -- --ignored keychain`.
    #[test]
    #[ignore]
    fn keychain_roundtrip() {
        let id = format!("agentty-test-{}", std::process::id());
        secrets::store(&id, "s3cr3t-value").unwrap();
        assert_eq!(secrets::load(&id).unwrap(), "s3cr3t-value");
        secrets::delete(&id).unwrap();
        assert!(secrets::load(&id).is_err());
    }

    #[test]
    fn mcp_protocol_basics() {
        let c = connector();
        let init = handle(&c, &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})).unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "agentty-my-api");
        let list = handle(&c, &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
        let names: Vec<_> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(names, ["get_user", "request"]);
        assert!(handle(&c, &json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
        let unknown = handle(&c, &json!({"jsonrpc":"2.0","id":3,"method":"resources/list"})).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
        // Missing path parameters are reported as tool errors without making a request.
        let call = handle(&c, &json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_user","arguments":{}}})).unwrap();
        assert_eq!(call["result"]["isError"], true);
    }

    #[test]
    fn local_http_roundtrip_with_header_auth_and_no_redirects() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap();
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let response = if request.starts_with("GET /api/redirect") {
                    "HTTP/1.1 302 Found\r\nLocation: http://evil.example/steal\r\nContent-Length: 0\r\n\r\n".to_string()
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}".to_string()
                };
                stream.write_all(response.as_bytes()).unwrap();
                seen.push(request);
            }
            seen
        });
        let c = Connector::new("local", &format!("http://127.0.0.1:{port}/api"), Auth::None);
        let (status, body) =
            execute(&c, &Request { method: "GET".into(), path: "/items".into(), query: vec![("q".into(), "a b".into())], body: None })
                .unwrap();
        assert_eq!((status, body.as_str()), (200, "{\"ok\":true}"));
        let (status, _) = execute(&c, &Request { method: "GET".into(), path: "/redirect".into(), query: vec![], body: None }).unwrap();
        assert_eq!(status, 302, "redirects are returned, not followed");
        let seen = server.join().unwrap();
        assert!(seen[0].starts_with("GET /api/items?q=a+b HTTP/1.1"));
    }
}
