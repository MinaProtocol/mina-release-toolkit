//! The local console.
//!
//! A web server bound to the loopback interface, serving one page and a small
//! JSON API over the same code the CLI uses. It exists so the hardfork
//! parameters can be filled in and checked in a form instead of pasted as a
//! block of environment variables.
//!
//! It deliberately renders no build state. Creating a build returns
//! Buildkite's own URL and sends the operator there; Buildkite remains the
//! place where builds are watched.
//!
//! # Why a page served on localhost needs guarding
//!
//! Any web page open in the same browser can send requests to a port on
//! localhost. Three rules close that off, and all three matter:
//!
//! 1. The listener binds `127.0.0.1`, never `0.0.0.0`, so nothing off the
//!    machine can reach it.
//! 2. Every request must carry a token generated at startup. The page gets it
//!    from the URL it was opened with and sends it in a header, which a
//!    cross-origin page cannot set without a preflight this server never
//!    grants.
//! 3. The `Host` header must be a loopback address. This is what stops DNS
//!    rebinding, where an attacker's domain resolves to 127.0.0.1 and the
//!    browser treats their page as the origin.
//!
//! No CORS headers are sent, by omission and on purpose.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rand::RngCore;
use serde::Deserialize;
use serde_json::json;

use crate::cache_admin;
use crate::config::{Project, Registry};
use crate::error::{OpsError, OpsResult};
use crate::inventory::{self, InventoryQuery};
use crate::nightly::{self, NightlyQuery};
use crate::pipelines;

const TOKEN_HEADER: &str = "x-mina-ops-token";
const CONSOLE: &str = include_str!("console.html");

pub struct ServeOptions {
    pub port: u16,
    pub project: String,
    /// Keep the token between runs, so one bookmark stays valid.
    pub persist_token: bool,
    /// Replace a stored token with a new one.
    pub rotate_token: bool,
}

#[derive(Clone)]
struct AppState {
    registry: Arc<Registry>,
    project: String,
    token: Arc<String>,
    port: u16,
}

impl AppState {
    fn project(&self) -> OpsResult<&Project> {
        self.registry.project(&self.project)
    }
}

/// 32 random bytes, hex encoded.
fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn token_path() -> Option<PathBuf> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|dir| dir.join("mina-ops").join("console-token"))
}

fn looks_like_token(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit())
}

/// Where the token came from, so the operator is told rather than left to
/// guess whether their bookmark still works.
pub enum TokenOrigin {
    Fresh,
    Reused,
    Stored,
}

/// A stored token trades a little secrecy for a bookmark that keeps working.
///
/// The file is owner-only. If its permissions have drifted wider they are
/// tightened rather than trusted, because a token another local account can
/// read is a token that account can use.
fn resolve_token(options: &ServeOptions) -> (String, TokenOrigin, Vec<String>) {
    let mut notes = Vec::new();

    if !options.persist_token {
        return (new_token(), TokenOrigin::Fresh, notes);
    }

    let Some(path) = token_path() else {
        notes.push("no state directory available, so the token is not stored".to_string());
        return (new_token(), TokenOrigin::Fresh, notes);
    };

    if !options.rotate_token {
        if let Ok(stored) = std::fs::read_to_string(&path) {
            let stored = stored.trim().to_string();
            if looks_like_token(&stored) {
                if let Some(note) = tighten_permissions(&path) {
                    notes.push(note);
                }
                return (stored, TokenOrigin::Reused, notes);
            }
            notes.push(format!(
                "{} did not hold a valid token, so a new one was written",
                path.display()
            ));
        }
    }

    let token = new_token();
    match store_token(&path, &token) {
        Ok(()) => (token, TokenOrigin::Stored, notes),
        Err(e) => {
            notes.push(format!(
                "could not store the token in {}: {e}",
                path.display()
            ));
            (token, TokenOrigin::Fresh, notes)
        }
    }
}

fn store_token(path: &Path, token: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    use std::io::Write;
    file.write_all(token.as_bytes())?;
    // An existing file keeps its old mode, so set it explicitly as well.
    tighten_permissions(path);
    Ok(())
}

/// Returns a note when the permissions had to be corrected.
fn tighten_permissions(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(path).ok()?;
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o600);
            std::fs::set_permissions(path, permissions).ok()?;
            return Some(format!(
                "{} was readable by others ({mode:o}); tightened to 600",
                path.display()
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    None
}

pub async fn serve(registry: Registry, options: ServeOptions) -> OpsResult<()> {
    // Fail before binding if the project is unknown.
    registry.project(&options.project)?;

    let (token, origin, notes) = resolve_token(&options);
    let state = AppState {
        registry: Arc::new(registry),
        project: options.project,
        token: Arc::new(token.clone()),
        port: options.port,
    };

    let app = Router::new()
        .route("/", get(console))
        .route("/api/schema", get(schema))
        .route("/api/artifacts", get(artifacts))
        .route("/api/nightly", get(nightly_report))
        .route("/api/cache", get(cache_list))
        .route("/api/cache/detail", get(cache_detail))
        .route("/api/cache/delete", post(cache_delete))
        .route("/api/pipelines", get(pipelines_list))
        .route("/api/pipelines/validate", post(validate_pipeline))
        .route("/api/pipelines/trigger", post(trigger_pipeline))
        .with_state(state);

    let address = format!("127.0.0.1:{}", options.port);
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .map_err(|e| OpsError::Other(format!("cannot bind {address}: {e}")))?;

    for note in &notes {
        eprintln!("note: {note}");
    }
    println!("mina-ops console: http://{address}/?t={token}");
    match origin {
        TokenOrigin::Reused => println!(
            "The stored token was reused, so a bookmark of this URL keeps working. \
             Use --rotate-token to replace it."
        ),
        TokenOrigin::Stored => println!(
            "This token is stored in {} and will be reused next time.",
            token_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        ),
        TokenOrigin::Fresh => {
            println!("The token is required, and changes on every start.")
        }
    }

    axum::serve(listener, app)
        .await
        .map_err(|e| OpsError::Other(format!("server stopped: {e}")))
}

/// Loopback hosts the browser may present. Anything else is a rebinding
/// attempt or a misconfiguration, and is refused either way.
fn host_is_loopback(host: &str, port: u16) -> bool {
    let expected = [
        format!("127.0.0.1:{port}"),
        format!("[::1]:{port}"),
        format!("localhost:{port}"),
    ];
    expected.iter().any(|allowed| allowed == host)
}

/// The response a request must be refused with, or `None` when it may
/// proceed. Both rules are checked here so no route can forget one.
fn refusal(state: &AppState, headers: &HeaderMap, supplied: Option<&str>) -> Option<Response> {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !host_is_loopback(host, state.port) {
        return Some(
            (
                StatusCode::FORBIDDEN,
                "this server answers only on the loopback interface",
            )
                .into_response(),
        );
    }

    let presented = headers
        .get(TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .or(supplied)
        .unwrap_or_default();
    if presented != state.token.as_str() {
        return Some((StatusCode::UNAUTHORIZED, "missing or wrong token").into_response());
    }
    None
}

#[derive(Debug, Deserialize)]
struct TokenQuery {
    t: Option<String>,
}

async fn console(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<TokenQuery>,
) -> Response {
    if let Some(refused) = refusal(&state, &headers, query.t.as_deref()) {
        return refused;
    }
    // The page carries the token so its own requests can present it in a
    // header, which cross-origin pages cannot forge.
    Html(CONSOLE.replace("__TOKEN__", &state.token)).into_response()
}

async fn schema(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(refused) = refusal(&state, &headers, None) {
        return refused;
    }
    let Ok(project) = state.project() else {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "unknown project");
    };

    Json(json!({
        "project": state.project,
        "repo": project.repo,
        "artifacts": project.defaults.artifacts,
        "debian_codenames": project.defaults.codenames,
        "channels": ["unstable", "alpha", "beta", "stable"],
        "nightly_pipeline": project.buildkite.nightly_pipeline,
        "nightly_branch": project.buildkite.nightly_branch,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct ArtifactsQuery {
    commit: Option<String>,
    version: Option<String>,
    channel: Option<String>,
    codenames: Option<String>,
    artifacts: Option<String>,
    repo_path: Option<String>,
    #[serde(default)]
    skip_docker: bool,
    #[serde(default)]
    skip_debian: bool,
}

async fn artifacts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ArtifactsQuery>,
) -> Response {
    if let Some(refused) = refusal(&state, &headers, None) {
        return refused;
    }
    let Ok(project) = state.project() else {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "unknown project");
    };

    if query.commit.is_none() && query.version.is_none() {
        return json_error(StatusCode::BAD_REQUEST, "give a commit, a version, or both");
    }

    // A short hash needs a checkout to expand. The form's field wins, then
    // MINA_REPO, so the common case needs no field at all.
    let repo_path = query
        .repo_path
        .clone()
        .filter(|p| !p.trim().is_empty())
        .or_else(|| std::env::var("MINA_REPO").ok());

    let commit = match &query.commit {
        Some(commit) => {
            match inventory::resolve_commit(commit, repo_path.as_deref().map(std::path::Path::new))
            {
                Ok(resolved) => Some(resolved),
                Err(e) => return json_error(StatusCode::BAD_REQUEST, &e.to_string()),
            }
        }
        None => None,
    };

    let inventory_query = InventoryQuery {
        commit,
        version: query.version.clone(),
        channel: query
            .channel
            .clone()
            .unwrap_or_else(|| project.defaults.channel.clone()),
        artifacts: split(query.artifacts.as_deref())
            .unwrap_or_else(|| project.defaults.artifacts.clone()),
        codenames: split(query.codenames.as_deref())
            .unwrap_or_else(|| project.defaults.codenames.clone()),
        network: None,
        profile: project.defaults.profile.clone(),
        max_builds: 5,
        skip_buildkite: false,
        skip_debian: query.skip_debian,
        skip_docker: query.skip_docker,
        skip_github: false,
        skip_cache: false,
    };

    match inventory::collect(&state.project, project, &inventory_query).await {
        Ok(inventory) => Json(inventory).into_response(),
        Err(e) => json_error(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct NightlyParams {
    pipeline: Option<String>,
    branch: Option<String>,
    last: Option<usize>,
}

async fn nightly_report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<NightlyParams>,
) -> Response {
    if let Some(refused) = refusal(&state, &headers, None) {
        return refused;
    }
    let Ok(project) = state.project() else {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "unknown project");
    };

    let Some(pipeline) = query
        .pipeline
        .clone()
        .or_else(|| project.buildkite.nightly_pipeline.clone())
    else {
        return json_error(StatusCode::BAD_REQUEST, "no nightly pipeline configured");
    };

    let nightly_query = NightlyQuery {
        pipeline,
        branch: query
            .branch
            .clone()
            .or_else(|| project.buildkite.nightly_branch.clone()),
        last: query.last.unwrap_or(3).clamp(1, 20),
        skip_github: false,
    };

    match nightly::collect(&state.project, project, &nightly_query).await {
        Ok(report) => Json(report).into_response(),
        Err(e) => json_error(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}

async fn cache_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(refused) = refusal(&state, &headers, None) {
        return refused;
    }
    let Ok(project) = state.project() else {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "unknown project");
    };
    match cache_admin::list(project).await {
        Ok(listing) => Json(listing).into_response(),
        Err(e) => json_error(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct BuildQuery {
    build_id: String,
}

async fn cache_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<BuildQuery>,
) -> Response {
    if let Some(refused) = refusal(&state, &headers, None) {
        return refused;
    }
    let Ok(project) = state.project() else {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "unknown project");
    };
    match cache_admin::detail(project, &query.build_id).await {
        Ok(detail) => Json(detail).into_response(),
        Err(e) => json_error(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}

async fn cache_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<cache_admin::DeleteRequest>,
) -> Response {
    if let Some(refused) = refusal(&state, &headers, None) {
        return refused;
    }
    let Ok(project) = state.project() else {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "unknown project");
    };

    match cache_admin::delete(project, &request).await {
        Ok(report) => {
            // Every real deletion is recorded, whatever the outcome page shows.
            if report.deleted {
                log_deletion(&report);
            }
            Json(report).into_response()
        }
        Err(e) => json_error(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}

/// Appends to `~/.local/state/mina-ops/deletions.log`. Best effort: a failure
/// to write the log must not hide the result of the deletion itself.
fn log_deletion(report: &cache_admin::DeleteReport) {
    let Some(dir) = dirs::state_dir().or_else(dirs::data_local_dir) else {
        return;
    };
    let dir = dir.join("mina-ops");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let line = format!(
        "{} {} removed {} freeing {}\n",
        chrono::Utc::now().to_rfc3339(),
        triggered_by(),
        report.build_id,
        report.freed.as_deref().unwrap_or("unknown")
    );
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("deletions.log"))
    {
        let _ = file.write_all(line.as_bytes());
    }
}

async fn pipelines_list(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(refused) = refusal(&state, &headers, None) {
        return refused;
    }
    let Ok(project) = state.project() else {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "unknown project");
    };
    Json(json!({ "pipelines": project.pipelines })).into_response()
}

async fn validate_pipeline(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<pipelines::TriggerRequest>,
) -> Response {
    if let Some(refused) = refusal(&state, &headers, None) {
        return refused;
    }
    let Ok(project) = state.project() else {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "unknown project");
    };
    match pipelines::validate(project, &request).await {
        Ok(validation) => Json(validation).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

async fn trigger_pipeline(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<pipelines::TriggerRequest>,
) -> Response {
    if let Some(refused) = refusal(&state, &headers, None) {
        return refused;
    }
    let Ok(project) = state.project() else {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "unknown project");
    };
    match pipelines::trigger(project, &request).await {
        // Refused by the server's own re-check, which is what is returned.
        Ok(Err(validation)) => (StatusCode::BAD_REQUEST, Json(validation)).into_response(),
        Ok(Ok(result)) => Json(result).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}

fn triggered_by() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn split(value: Option<&str>) -> Option<Vec<String>> {
    let values: Vec<String> = value?
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    (!values.is_empty()).then_some(values)
}

fn json_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        AppState {
            registry: Arc::new(Registry::builtin().unwrap()),
            project: "mina".to_string(),
            token: Arc::new("secret-token".to_string()),
            port: 7777,
        }
    }

    fn headers(host: &str, token: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::HOST, host.parse().unwrap());
        if let Some(token) = token {
            headers.insert(TOKEN_HEADER, token.parse().unwrap());
        }
        headers
    }

    #[test]
    fn loopback_hosts_are_accepted_and_others_are_not() {
        assert!(host_is_loopback("127.0.0.1:7777", 7777));
        assert!(host_is_loopback("localhost:7777", 7777));
        assert!(host_is_loopback("[::1]:7777", 7777));
        // A rebinding attempt presents an attacker-controlled name.
        assert!(!host_is_loopback("evil.example.com:7777", 7777));
        // Right name, wrong port: not this server.
        assert!(!host_is_loopback("127.0.0.1:9999", 7777));
        assert!(!host_is_loopback("", 7777));
    }

    #[test]
    fn a_request_from_another_origin_is_refused_even_with_the_token() {
        let state = state();
        let refused = refusal(
            &state,
            &headers("evil.example.com:7777", Some("secret-token")),
            None,
        );
        assert!(
            refused.is_some(),
            "the host check must run before the token"
        );
    }

    #[test]
    fn a_loopback_request_without_a_token_is_refused() {
        let state = state();
        assert!(refusal(&state, &headers("127.0.0.1:7777", None), None).is_some());
    }

    #[test]
    fn a_wrong_token_is_refused() {
        let state = state();
        assert!(refusal(&state, &headers("127.0.0.1:7777", Some("guess")), None).is_some());
    }

    #[test]
    fn the_token_may_arrive_in_the_header_or_the_query() {
        let state = state();
        assert!(refusal(
            &state,
            &headers("127.0.0.1:7777", Some("secret-token")),
            None
        )
        .is_none());
        assert!(refusal(
            &state,
            &headers("127.0.0.1:7777", None),
            Some("secret-token")
        )
        .is_none());
    }

    #[test]
    fn tokens_differ_and_are_long_enough() {
        let first = new_token();
        let second = new_token();
        assert_ne!(first, second);
        assert_eq!(first.len(), 64, "32 bytes, hex encoded");
        assert!(looks_like_token(&first));
    }

    #[test]
    fn only_a_full_hex_token_is_accepted_from_disk() {
        assert!(looks_like_token(&"a".repeat(64)));
        assert!(!looks_like_token(&"a".repeat(63)));
        assert!(!looks_like_token(&"z".repeat(64)));
        assert!(!looks_like_token(""));
    }

    #[test]
    fn a_stored_token_is_written_owner_only_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("console-token");
        let token = new_token();

        store_token(&path, &token).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), token);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o600,
                "a token another account can read is a token they can use"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn permissions_that_drifted_wider_are_tightened_not_trusted() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("console-token");
        store_token(&path, &new_token()).unwrap();

        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o644);
        std::fs::set_permissions(&path, permissions).unwrap();

        let note = tighten_permissions(&path);
        assert!(note.is_some(), "the correction should be reported");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // Already correct: nothing to say.
        assert!(tighten_permissions(&path).is_none());
    }

    /// The page is one inline script, so a duplicate top-level binding is a
    /// syntax error that disables the entire console — silently, because
    /// nothing on the server side notices. This catches that class of edit.
    #[test]
    fn the_console_script_declares_each_binding_once() {
        let script = CONSOLE
            .split_once("<script>")
            .and_then(|(_, rest)| rest.split_once("</script>"))
            .map(|(script, _)| script)
            .expect("the console has an inline script");

        let mut seen: Vec<&str> = Vec::new();
        for line in script.lines() {
            // Top level means column zero. An indented binding lives inside a
            // function, where shadowing is ordinary and harmless.
            for keyword in ["let ", "const ", "var "] {
                if let Some(rest) = line.strip_prefix(keyword) {
                    let name = rest
                        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .next()
                        .unwrap_or_default();
                    if name.is_empty() {
                        continue;
                    }
                    assert!(
                        !seen.contains(&name),
                        "'{name}' is declared twice at the top level, which breaks the whole script"
                    );
                    seen.push(name);
                }
            }
        }
        assert!(seen.contains(&"TOKEN"), "the scan found no bindings at all");
    }

    #[test]
    fn the_console_carries_the_token_and_no_placeholder_remains() {
        let rendered = CONSOLE.replace("__TOKEN__", "abc123");
        assert!(rendered.contains("abc123"));
        assert!(!rendered.contains("__TOKEN__"));
    }

    #[test]
    fn comma_separated_lists_are_split_and_blanks_ignored() {
        assert_eq!(
            split(Some("noble, bookworm")),
            Some(vec!["noble".to_string(), "bookworm".to_string()])
        );
        assert_eq!(split(Some("  ")), None);
        assert_eq!(split(None), None);
    }
}
