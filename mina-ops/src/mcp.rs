//! MCP server over stdio.
//!
//! Exposes the same inventory the CLI prints, so an agent can ask "what exists
//! for this commit" during an incident without shelling out and parsing text.
//!
//! The transport is line-delimited JSON-RPC 2.0 on stdin and stdout, so
//! nothing else may be written to stdout while the server runs. Diagnostics go
//! to stderr.
//!
//! Credentials are inherited from the process, which is what makes this safe:
//! the server can reach exactly what the person running it can reach, and it
//! only reads.

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::config::{Project, Registry};
use crate::error::{OpsError, OpsResult};
use crate::inventory::{self, InventoryQuery};

/// Used when the client does not name a version of its own.
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";
const SERVER_NAME: &str = "mina-ops";

pub async fn serve(registry: Registry) -> OpsResult<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| OpsError::Other(format!("stdin: {e}")))?
    {
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle(request, &registry).await,
            Err(e) => Some(error_response(
                Value::Null,
                -32700,
                &format!("parse error: {e}"),
            )),
        };

        // Notifications get no reply at all.
        if let Some(response) = response {
            let mut text = serde_json::to_string(&response)
                .map_err(|e| OpsError::Other(format!("cannot serialise response: {e}")))?;
            text.push('\n');
            stdout
                .write_all(text.as_bytes())
                .await
                .map_err(|e| OpsError::Other(format!("stdout: {e}")))?;
            stdout
                .flush()
                .await
                .map_err(|e| OpsError::Other(format!("stdout: {e}")))?;
        }
    }

    Ok(())
}

/// Returns `None` for notifications, which by JSON-RPC carry no `id` and must
/// not be answered.
pub async fn handle(request: Value, registry: &Registry) -> Option<Value> {
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    // No id means a notification, which is answered with silence.
    let id = request.get("id").cloned()?;

    match method {
        "initialize" => {
            let protocol = request
                .get("params")
                .and_then(|p| p.get("protocolVersion"))
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_PROTOCOL_VERSION);
            Some(result_response(
                id,
                json!({
                    "protocolVersion": protocol,
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": SERVER_NAME,
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }),
            ))
        }
        "ping" => Some(result_response(id, json!({}))),
        "tools/list" => Some(result_response(id, json!({ "tools": tool_definitions() }))),
        "tools/call" => Some(handle_tool_call(id, &request, registry).await),
        other => Some(error_response(
            id,
            -32601,
            &format!("unknown method '{other}'"),
        )),
    }
}

async fn handle_tool_call(id: Value, request: &Value, registry: &Registry) -> Value {
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let project_name = arguments
        .get("project")
        .and_then(Value::as_str)
        .unwrap_or("mina")
        .to_string();

    let project = match registry.project(&project_name) {
        Ok(project) => project,
        Err(e) => return tool_error(id, &e.to_string()),
    };

    let query = match name {
        "mina_artifacts" => artifacts_query(&arguments, project),
        "mina_builds" => builds_query(&arguments, project),
        other => return tool_error(id, &format!("unknown tool '{other}'")),
    };

    let query = match query {
        Ok(query) => query,
        Err(e) => return tool_error(id, &e.to_string()),
    };

    let commit = query.commit.clone();
    let repo_path = arguments.get("repo_path").and_then(Value::as_str);
    let query = match resolve_query_commit(query, commit.as_deref(), repo_path) {
        Ok(query) => query,
        Err(e) => return tool_error(id, &e.to_string()),
    };

    match inventory::collect(&project_name, project, &query).await {
        Ok(inventory) => match serde_json::to_string_pretty(&inventory) {
            Ok(text) => result_response(id, json!({ "content": [text_content(&text)] })),
            Err(e) => tool_error(id, &format!("cannot serialise the inventory: {e}")),
        },
        Err(e) => tool_error(id, &e.to_string()),
    }
}

fn resolve_query_commit(
    mut query: InventoryQuery,
    commit: Option<&str>,
    repo_path: Option<&str>,
) -> OpsResult<InventoryQuery> {
    if let Some(commit) = commit {
        query.commit = Some(inventory::resolve_commit(
            commit,
            repo_path.map(std::path::Path::new),
        )?);
    }
    Ok(query)
}

pub fn artifacts_query(arguments: &Value, project: &Project) -> OpsResult<InventoryQuery> {
    let commit = string_arg(arguments, "commit");
    let version = string_arg(arguments, "version");
    if commit.is_none() && version.is_none() {
        return Err(OpsError::Config(
            "give a commit, a version, or both".to_string(),
        ));
    }

    Ok(InventoryQuery {
        commit,
        version,
        channel: string_arg(arguments, "channel")
            .unwrap_or_else(|| project.defaults.channel.clone()),
        artifacts: list_arg(arguments, "artifacts")
            .unwrap_or_else(|| project.defaults.artifacts.clone()),
        codenames: list_arg(arguments, "codenames")
            .unwrap_or_else(|| project.defaults.codenames.clone()),
        network: string_arg(arguments, "network"),
        profile: string_arg(arguments, "profile").or_else(|| project.defaults.profile.clone()),
        max_builds: arguments
            .get("max_builds")
            .and_then(Value::as_u64)
            .unwrap_or(5) as usize,
        skip_buildkite: bool_arg(arguments, "skip_buildkite"),
        skip_debian: bool_arg(arguments, "skip_debian"),
        skip_docker: bool_arg(arguments, "skip_docker"),
    })
}

pub fn builds_query(arguments: &Value, project: &Project) -> OpsResult<InventoryQuery> {
    let commit = string_arg(arguments, "commit")
        .ok_or_else(|| OpsError::Config("a commit is required".to_string()))?;

    Ok(InventoryQuery {
        commit: Some(commit),
        version: None,
        channel: project.defaults.channel.clone(),
        artifacts: Vec::new(),
        codenames: Vec::new(),
        network: None,
        profile: None,
        max_builds: arguments
            .get("max_builds")
            .and_then(Value::as_u64)
            .unwrap_or(10) as usize,
        skip_buildkite: false,
        skip_debian: true,
        skip_docker: true,
    })
}

fn string_arg(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Accepts either a list of strings or one comma-separated string, because
/// both forms turn up in tool calls.
fn list_arg(arguments: &Value, key: &str) -> Option<Vec<String>> {
    match arguments.get(key) {
        Some(Value::Array(items)) => {
            let values: Vec<String> = items
                .iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            (!values.is_empty()).then_some(values)
        }
        Some(Value::String(text)) => {
            let values: Vec<String> = text
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            (!values.is_empty()).then_some(values)
        }
        _ => None,
    }
}

fn bool_arg(arguments: &Value, key: &str) -> bool {
    arguments.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "mina_artifacts",
            "description": "For one commit or version, report which Debian packages and Docker images exist, and which Buildkite builds ran. Each check reports present, missing, or unknown with a reason; unknown means the check could not run and must not be read as missing. Anything that limited the answer is listed in 'warnings'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "commit": { "type": "string", "description": "Commit hash. A short hash needs repo_path to expand it." },
                    "version": { "type": "string", "description": "Package version, when already known, e.g. 3.3.0-8c0c2e6. Skips version resolution." },
                    "repo_path": { "type": "string", "description": "Local checkout used to expand a short commit hash." },
                    "channel": { "type": "string", "description": "Release channel: unstable, alpha, beta or stable. Selects the repositories and the network." },
                    "artifacts": { "type": "array", "items": { "type": "string" }, "description": "Artifacts to check, e.g. mina-daemon, mina-archive." },
                    "codenames": { "type": "array", "items": { "type": "string" }, "description": "Debian codenames, e.g. bullseye, bookworm, noble." },
                    "network": { "type": "string", "description": "Network, when it differs from the channel default." },
                    "profile": { "type": "string", "description": "Build profile: lightnet or instrumented." },
                    "max_builds": { "type": "integer", "description": "Builds whose artifacts are listed. Buildkite allows 200 requests a minute." },
                    "skip_buildkite": { "type": "boolean" },
                    "skip_debian": { "type": "boolean" },
                    "skip_docker": { "type": "boolean" },
                    "project": { "type": "string", "description": "Project in the registry. Defaults to mina." }
                }
            }
        }),
        json!({
            "name": "mina_builds",
            "description": "Buildkite builds for one commit, across every pipeline in the organisation. Note that Mina pipelines upload logs and tools to Buildkite, not .deb files, so an artifact count of zero is normal and does not mean packages were deleted.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "commit": { "type": "string", "description": "Commit hash. A short hash needs repo_path to expand it." },
                    "repo_path": { "type": "string", "description": "Local checkout used to expand a short commit hash." },
                    "max_builds": { "type": "integer", "description": "Builds whose artifacts are listed." },
                    "project": { "type": "string", "description": "Project in the registry. Defaults to mina." }
                },
                "required": ["commit"]
            }
        }),
    ]
}

fn text_content(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

fn result_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// A failed tool is reported inside the result, not as a protocol error, so
/// the caller sees the reason rather than a transport failure.
fn tool_error(id: Value, message: &str) -> Value {
    result_response(
        id,
        json!({ "content": [text_content(message)], "isError": true }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Registry {
        Registry::builtin().unwrap()
    }

    #[tokio::test]
    async fn initialize_echoes_the_client_protocol_version() {
        let request = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2024-11-05" }
        });
        let response = handle(request, &registry()).await.unwrap();
        assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(response["result"]["serverInfo"]["name"], SERVER_NAME);
    }

    #[tokio::test]
    async fn initialize_without_a_version_uses_the_default() {
        let request = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" });
        let response = handle(request, &registry()).await.unwrap();
        assert_eq!(
            response["result"]["protocolVersion"],
            DEFAULT_PROTOCOL_VERSION
        );
    }

    #[tokio::test]
    async fn notifications_are_not_answered() {
        let request = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle(request, &registry()).await.is_none());
    }

    #[tokio::test]
    async fn tools_are_listed_with_schemas() {
        let request = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
        let response = handle(request, &registry()).await.unwrap();
        let tools = response["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"mina_artifacts"));
        assert!(names.contains(&"mina_builds"));
        assert!(tools[0]["inputSchema"]["type"] == "object");
    }

    #[tokio::test]
    async fn unknown_methods_are_a_protocol_error() {
        let request = json!({ "jsonrpc": "2.0", "id": 3, "method": "nope" });
        let response = handle(request, &registry()).await.unwrap();
        assert_eq!(response["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn an_unknown_tool_is_reported_in_the_result() {
        let request = json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": { "name": "nope", "arguments": {} }
        });
        let response = handle(request, &registry()).await.unwrap();
        assert_eq!(response["result"]["isError"], true);
        assert!(response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("unknown tool"));
    }

    #[tokio::test]
    async fn calling_artifacts_without_a_commit_or_version_explains_itself() {
        let request = json!({
            "jsonrpc": "2.0", "id": 5, "method": "tools/call",
            "params": { "name": "mina_artifacts", "arguments": {} }
        });
        let response = handle(request, &registry()).await.unwrap();
        assert_eq!(response["result"]["isError"], true);
        assert!(response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("commit"));
    }

    #[test]
    fn arguments_fall_back_to_the_project_defaults() {
        let registry = registry();
        let project = registry.project("mina").unwrap();
        let query = artifacts_query(&json!({ "version": "3.3.0-8c0c2e6" }), project).unwrap();
        assert_eq!(query.channel, project.defaults.channel);
        assert_eq!(query.codenames, project.defaults.codenames);
        assert_eq!(query.max_builds, 5);
    }

    #[test]
    fn lists_accept_arrays_and_comma_separated_strings() {
        let registry = registry();
        let project = registry.project("mina").unwrap();

        let from_array = artifacts_query(
            &json!({ "version": "v", "codenames": ["noble", "bookworm"] }),
            project,
        )
        .unwrap();
        assert_eq!(from_array.codenames, vec!["noble", "bookworm"]);

        let from_string = artifacts_query(
            &json!({ "version": "v", "codenames": "noble, bookworm" }),
            project,
        )
        .unwrap();
        assert_eq!(from_string.codenames, vec!["noble", "bookworm"]);
    }

    #[test]
    fn empty_strings_are_treated_as_absent() {
        let registry = registry();
        let project = registry.project("mina").unwrap();
        let query = artifacts_query(&json!({ "version": "v", "channel": "  " }), project).unwrap();
        assert_eq!(query.channel, project.defaults.channel);
    }

    #[test]
    fn a_builds_query_asks_only_about_builds() {
        let registry = registry();
        let project = registry.project("mina").unwrap();
        let query = builds_query(&json!({ "commit": "abc" }), project).unwrap();
        assert!(query.is_builds_only());
        assert_eq!(query.max_builds, 10);
    }

    #[test]
    fn a_builds_query_requires_a_commit() {
        let registry = registry();
        let project = registry.project("mina").unwrap();
        assert!(builds_query(&json!({}), project).is_err());
    }
}
