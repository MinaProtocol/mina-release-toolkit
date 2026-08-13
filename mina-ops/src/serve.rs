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

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::adapters::buildkite::{token_from_environment, BuildkiteClient, NewBuild};
use crate::config::{Project, Registry};
use crate::error::{OpsError, OpsResult};
use crate::hardfork::{self, HardforkParams, Validation};
use crate::inventory::{self, InventoryQuery};
use crate::nightly::{self, NightlyQuery};

const TOKEN_HEADER: &str = "x-mina-ops-token";
const CONSOLE: &str = include_str!("console.html");

pub struct ServeOptions {
    pub port: u16,
    pub project: String,
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

/// 32 random bytes, hex encoded. Regenerated on every start, so a token that
/// leaks into a shell history is useless once the server stops.
fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub async fn serve(registry: Registry, options: ServeOptions) -> OpsResult<()> {
    // Fail before binding if the project is unknown.
    registry.project(&options.project)?;

    let token = new_token();
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
        .route("/api/hardfork/validate", post(validate_hardfork))
        .route("/api/hardfork/trigger", post(trigger_hardfork))
        .with_state(state);

    let address = format!("127.0.0.1:{}", options.port);
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .map_err(|e| OpsError::Other(format!("cannot bind {address}: {e}")))?;

    println!("mina-ops console: http://{address}/?t={token}");
    println!("Open that URL. The token is required, and changes on every start.");

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
        "codenames": hardfork::CODENAMES,
        "architectures": hardfork::ARCHITECTURES,
        "networks": hardfork::NETWORKS,
        "repos": hardfork::REPOS,
        "artifacts": project.defaults.artifacts,
        "debian_codenames": project.defaults.codenames,
        "channels": ["unstable", "alpha", "beta", "stable"],
        "hardfork_pipeline": project.buildkite.hardfork_pipeline,
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

async fn validate_hardfork(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(params): Json<HardforkParams>,
) -> Response {
    if let Some(refused) = refusal(&state, &headers, None) {
        return refused;
    }
    let Ok(project) = state.project() else {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "unknown project");
    };
    let Some(pipeline) = project.buildkite.hardfork_pipeline.clone() else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "no hardfork pipeline configured for this project",
        );
    };

    Json(hardfork::validate(&params, project, &pipeline).await).into_response()
}

#[derive(Debug, Deserialize)]
struct TriggerRequest {
    #[serde(flatten)]
    params: HardforkParams,
    /// Must be true. A trigger is never a side effect of filling in a form.
    #[serde(default)]
    confirm: bool,
}

#[derive(Debug, Serialize)]
struct TriggerResponse {
    web_url: String,
    number: u64,
    state: String,
    env: BTreeMap<String, String>,
}

async fn trigger_hardfork(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<TriggerRequest>,
) -> Response {
    if let Some(refused) = refusal(&state, &headers, None) {
        return refused;
    }
    let Ok(project) = state.project() else {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "unknown project");
    };
    let Some(pipeline) = project.buildkite.hardfork_pipeline.clone() else {
        return json_error(
            StatusCode::BAD_REQUEST,
            "no hardfork pipeline configured for this project",
        );
    };

    if !request.confirm {
        return json_error(
            StatusCode::BAD_REQUEST,
            "confirm must be true to create a build",
        );
    }

    // Re-checked here rather than trusted from the browser: the client's
    // copy of the validation proves nothing about this request.
    let validation: Validation = hardfork::validate(&request.params, project, &pipeline).await;
    if !validation.is_submittable() {
        return (StatusCode::BAD_REQUEST, Json(validation)).into_response();
    }

    let token = match token_from_environment() {
        Ok(token) => token,
        Err(e) => return json_error(StatusCode::BAD_REQUEST, &e.to_string()),
    };

    let mut env = request.params.to_env();
    // The Buildkite token belongs to the operator, so Buildkite already
    // records who created the build. This makes it visible in the build's own
    // environment as well.
    env.insert("TRIGGERED_BY".into(), triggered_by());

    let new_build = NewBuild {
        commit: "HEAD".to_string(),
        branch: request.params.branch.clone(),
        message: format!(
            "hardfork packages for {} at {} (via mina-ops)",
            request.params.network, request.params.genesis_timestamp
        ),
        env,
    };

    let client = BuildkiteClient::new(&project.buildkite.org, token);
    match client.create_build(&pipeline, &new_build).await {
        Ok(created) => Json(TriggerResponse {
            web_url: created.web_url,
            number: created.number,
            state: created.state,
            env: new_build.env,
        })
        .into_response(),
        Err(e) => json_error(StatusCode::BAD_GATEWAY, &e.to_string()),
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
    fn tokens_differ_between_starts_and_are_long_enough() {
        let first = new_token();
        let second = new_token();
        assert_ne!(first, second);
        assert_eq!(first.len(), 64, "32 bytes, hex encoded");
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
