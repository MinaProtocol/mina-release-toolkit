//! Pipelines that can be started from the console.
//!
//! Most of them take almost nothing: a branch, and occasionally one knob such
//! as a job name or a filter. Only the hardfork pipeline has a parameter set
//! worth checking in depth. So a pipeline is *declared* — its fields, their
//! types and their rules live in the project registry — and only the deep
//! checks are code.
//!
//! Adding a pipeline should mean editing YAML, not this file.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::adapters::buildkite::{token_from_environment, BuildkiteClient, NewBuild};
use crate::config::Project;
use crate::error::{OpsError, OpsResult};
use crate::hardfork;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Text,
    Select,
    /// Present as `"1"` when ticked, absent when not — the shape the
    /// pipelines' own scripts test for.
    Checkbox,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    /// The environment variable this field becomes.
    pub name: String,
    pub label: String,
    #[serde(default = "default_field_type")]
    pub kind: FieldType,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub help: Option<String>,
    /// Permitted values, for a select or a checked text field.
    #[serde(default)]
    pub options: Vec<String>,
    /// A regular expression the value must match.
    #[serde(default)]
    pub pattern: Option<String>,
}

fn default_field_type() -> FieldType {
    FieldType::Text
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineSpec {
    /// Stable identifier used by the API and the UI.
    pub key: String,
    pub label: String,
    /// Buildkite pipeline slug.
    pub slug: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default_branch: Option<String>,
    /// Environment set on every build of this pipeline.
    #[serde(default)]
    pub always_env: BTreeMap<String, String>,
    #[serde(default)]
    pub fields: Vec<Field>,
    /// Names a set of built-in checks that reach the outside world. Only
    /// `hardfork` exists today.
    #[serde(default)]
    pub checks: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", content = "detail", rename_all = "snake_case")]
pub enum Outcome {
    Passed(String),
    /// The value is wrong. Submitting would waste a build.
    Failed(String),
    /// The check could not run. Reported, but never a reason to block.
    Unknown(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub field: String,
    #[serde(flatten)]
    pub outcome: Outcome,
}

impl Check {
    pub fn passed(field: &str, detail: impl Into<String>) -> Self {
        Check {
            field: field.to_string(),
            outcome: Outcome::Passed(detail.into()),
        }
    }
    pub fn failed(field: &str, detail: impl Into<String>) -> Self {
        Check {
            field: field.to_string(),
            outcome: Outcome::Failed(detail.into()),
        }
    }
    pub fn unknown(field: &str, detail: impl Into<String>) -> Self {
        Check {
            field: field.to_string(),
            outcome: Outcome::Unknown(detail.into()),
        }
    }
    pub fn has_failed(&self) -> bool {
        matches!(self.outcome, Outcome::Failed(_))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Validation {
    pub pipeline: String,
    pub slug: String,
    pub branch: String,
    pub checks: Vec<Check>,
    /// Exactly what Buildkite would receive, shown before anything is created.
    pub env: BTreeMap<String, String>,
}

impl Validation {
    pub fn is_submittable(&self) -> bool {
        !self.checks.iter().any(Check::has_failed)
    }
}

/// The request the console sends, for validation and for triggering alike.
#[derive(Debug, Clone, Deserialize)]
pub struct TriggerRequest {
    pub pipeline: String,
    pub branch: String,
    #[serde(default)]
    pub values: BTreeMap<String, String>,
    /// Must be true to create a build. Never a side effect of filling a form.
    #[serde(default)]
    pub confirm: bool,
}

pub fn find<'a>(project: &'a Project, key: &str) -> OpsResult<&'a PipelineSpec> {
    project
        .pipelines
        .iter()
        .find(|spec| spec.key == key)
        .ok_or_else(|| {
            let known: Vec<&str> = project.pipelines.iter().map(|s| s.key.as_str()).collect();
            OpsError::Config(format!(
                "unknown pipeline '{key}'. Known pipelines: {}",
                known.join(", ")
            ))
        })
}

/// The environment a build would carry: the always-set values, then each
/// field that has one.
pub fn to_env(spec: &PipelineSpec, values: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut env = spec.always_env.clone();

    for field in &spec.fields {
        let value = values
            .get(&field.name)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .or_else(|| field.default.clone());

        let Some(value) = value else { continue };
        match field.kind {
            // An unticked checkbox is absent rather than empty, because the
            // pipelines' scripts test for emptiness to decide.
            FieldType::Checkbox if value != "1" && value != "true" => continue,
            FieldType::Checkbox => {
                env.insert(field.name.clone(), "1".to_string());
            }
            _ => {
                env.insert(field.name.clone(), value);
            }
        }
    }
    env
}

/// Checks that need nothing but the declaration.
pub fn check_declared(
    spec: &PipelineSpec,
    branch: &str,
    env: &BTreeMap<String, String>,
) -> Vec<Check> {
    let mut checks = Vec::new();

    if branch.trim().is_empty() {
        checks.push(Check::failed("branch", "a branch is required"));
    } else {
        checks.push(Check::passed("branch", branch));
    }

    for field in &spec.fields {
        let value = env.get(&field.name);

        match value {
            None => {
                if field.required {
                    checks.push(Check::failed(&field.name, "this field is required"));
                }
                // An absent optional field is not worth a line.
                continue;
            }
            Some(value) => {
                if !field.options.is_empty() && !field.options.contains(value) {
                    checks.push(Check::failed(
                        &field.name,
                        format!("must be one of {}", field.options.join(", ")),
                    ));
                    continue;
                }
                if let Some(pattern) = &field.pattern {
                    match regex::Regex::new(pattern) {
                        Ok(regex) if !regex.is_match(value) => {
                            checks.push(Check::failed(
                                &field.name,
                                field
                                    .help
                                    .clone()
                                    .unwrap_or_else(|| format!("must match {pattern}")),
                            ));
                            continue;
                        }
                        Err(e) => {
                            checks.push(Check::unknown(
                                &field.name,
                                format!("the declared pattern is not valid: {e}"),
                            ));
                            continue;
                        }
                        _ => {}
                    }
                }
                checks.push(Check::passed(&field.name, value));
            }
        }
    }

    checks
}

pub async fn validate(project: &Project, request: &TriggerRequest) -> OpsResult<Validation> {
    let spec = find(project, &request.pipeline)?;
    let env = to_env(spec, &request.values);

    let mut checks = check_declared(spec, &request.branch, &env);

    // A pipeline may ask for checks that reach the outside world, or that
    // know more about a field than a pattern can express.
    if spec.checks.as_deref() == Some("hardfork") {
        let deep = hardfork::deep_checks(&env, project).await;
        // The deeper result replaces the declared one for the same field, so
        // a field is reported once, by whichever check knows most about it.
        let deepened: Vec<&str> = deep.iter().map(|c| c.field.as_str()).collect();
        checks.retain(|check| !deepened.contains(&check.field.as_str()));
        checks.extend(deep);
    }

    Ok(Validation {
        pipeline: spec.key.clone(),
        slug: spec.slug.clone(),
        branch: request.branch.clone(),
        checks,
        env,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct TriggerResult {
    pub web_url: String,
    pub number: u64,
    pub state: String,
    pub pipeline: String,
    pub env: BTreeMap<String, String>,
}

/// Creates the build, after re-running every check here rather than trusting
/// the caller's copy of them.
pub async fn trigger(
    project: &Project,
    request: &TriggerRequest,
) -> OpsResult<Result<TriggerResult, Validation>> {
    if !request.confirm {
        return Err(OpsError::Config(
            "confirm must be true to create a build".to_string(),
        ));
    }

    let validation = validate(project, request).await?;
    if !validation.is_submittable() {
        return Ok(Err(validation));
    }

    let spec = find(project, &request.pipeline)?;
    let token = token_from_environment()?;

    let mut env = validation.env.clone();
    env.insert("TRIGGERED_BY".to_string(), triggered_by());

    let new_build = NewBuild {
        commit: "HEAD".to_string(),
        branch: request.branch.clone(),
        message: format!("{} (via mina-ops)", spec.label),
        env: env.clone(),
    };

    let client = BuildkiteClient::new(&project.buildkite.org, token);
    let created = client.create_build(&spec.slug, &new_build).await?;

    Ok(Ok(TriggerResult {
        web_url: created.web_url,
        number: created.number,
        state: created.state,
        pipeline: spec.key.clone(),
        env,
    }))
}

fn triggered_by() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> PipelineSpec {
        PipelineSpec {
            key: "docker".into(),
            label: "Build docker images".into(),
            slug: "mina-build-docker".into(),
            description: None,
            default_branch: Some("compatible".into()),
            always_env: BTreeMap::from([("GIT_LFS_SKIP_SMUDGE".into(), "1".into())]),
            fields: vec![
                Field {
                    name: "BUILDKITE_PIPELINE_FILTER".into(),
                    label: "Filter".into(),
                    kind: FieldType::Text,
                    required: true,
                    default: None,
                    placeholder: None,
                    help: None,
                    options: vec![],
                    pattern: Some("^[A-Za-z0-9]+$".into()),
                },
                Field {
                    name: "BUILDKITE_PIPELINE_FILTER_MODE".into(),
                    label: "Mode".into(),
                    kind: FieldType::Select,
                    required: false,
                    default: Some("All".into()),
                    placeholder: None,
                    help: None,
                    options: vec!["All".into(), "Fast".into()],
                    pattern: None,
                },
                Field {
                    name: "DEBUG".into(),
                    label: "Debug".into(),
                    kind: FieldType::Checkbox,
                    required: false,
                    default: None,
                    placeholder: None,
                    help: None,
                    options: vec![],
                    pattern: None,
                },
            ],
            checks: None,
        }
    }

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn failed(checks: &[Check]) -> Vec<&str> {
        checks
            .iter()
            .filter(|c| c.has_failed())
            .map(|c| c.field.as_str())
            .collect()
    }

    #[test]
    fn always_env_and_defaults_are_applied() {
        let env = to_env(
            &spec(),
            &values(&[("BUILDKITE_PIPELINE_FILTER", "DockerBuild")]),
        );
        assert_eq!(env.get("GIT_LFS_SKIP_SMUDGE").unwrap(), "1");
        assert_eq!(env.get("BUILDKITE_PIPELINE_FILTER").unwrap(), "DockerBuild");
        // Not supplied, so the declared default stands in.
        assert_eq!(env.get("BUILDKITE_PIPELINE_FILTER_MODE").unwrap(), "All");
    }

    #[test]
    fn a_blank_value_falls_back_to_the_default_rather_than_being_sent_empty() {
        let env = to_env(
            &spec(),
            &values(&[("BUILDKITE_PIPELINE_FILTER_MODE", "   ")]),
        );
        assert_eq!(env.get("BUILDKITE_PIPELINE_FILTER_MODE").unwrap(), "All");
    }

    #[test]
    fn an_unticked_checkbox_is_absent_not_empty() {
        let env = to_env(&spec(), &values(&[("DEBUG", "")]));
        assert!(!env.contains_key("DEBUG"));

        let ticked = to_env(&spec(), &values(&[("DEBUG", "1")]));
        assert_eq!(ticked.get("DEBUG").unwrap(), "1");
    }

    #[test]
    fn a_required_field_left_out_is_a_failure() {
        let env = to_env(&spec(), &values(&[]));
        assert_eq!(
            failed(&check_declared(&spec(), "compatible", &env)),
            vec!["BUILDKITE_PIPELINE_FILTER"]
        );
    }

    #[test]
    fn a_value_outside_the_declared_options_is_a_failure() {
        let env = to_env(
            &spec(),
            &values(&[
                ("BUILDKITE_PIPELINE_FILTER", "DockerBuild"),
                ("BUILDKITE_PIPELINE_FILTER_MODE", "Sideways"),
            ]),
        );
        assert_eq!(
            failed(&check_declared(&spec(), "compatible", &env)),
            vec!["BUILDKITE_PIPELINE_FILTER_MODE"]
        );
    }

    #[test]
    fn a_value_failing_the_declared_pattern_is_a_failure() {
        let env = to_env(
            &spec(),
            &values(&[("BUILDKITE_PIPELINE_FILTER", "not a filter!")]),
        );
        assert_eq!(
            failed(&check_declared(&spec(), "compatible", &env)),
            vec!["BUILDKITE_PIPELINE_FILTER"]
        );
    }

    #[test]
    fn a_missing_branch_is_a_failure() {
        let env = to_env(
            &spec(),
            &values(&[("BUILDKITE_PIPELINE_FILTER", "DockerBuild")]),
        );
        assert_eq!(failed(&check_declared(&spec(), "  ", &env)), vec!["branch"]);
    }

    #[test]
    fn a_well_formed_request_passes_everything() {
        let env = to_env(
            &spec(),
            &values(&[("BUILDKITE_PIPELINE_FILTER", "DockerBuild")]),
        );
        assert!(failed(&check_declared(&spec(), "compatible", &env)).is_empty());
    }

    #[test]
    fn a_deeper_check_replaces_the_declared_one_for_the_same_field() {
        // Two checks naming the same field must collapse to the later one,
        // so a reader sees each field once.
        let mut checks = vec![
            Check::passed("CODENAMES_CONFIG", "Jammy_Amd64"),
            Check::passed("NETWORK", "Devnet"),
        ];
        let deep = vec![Check::passed("CODENAMES_CONFIG", "1 entries")];
        let deepened: Vec<&str> = deep.iter().map(|c| c.field.as_str()).collect();
        checks.retain(|check| !deepened.contains(&check.field.as_str()));
        checks.extend(deep);

        let fields: Vec<&str> = checks.iter().map(|c| c.field.as_str()).collect();
        assert_eq!(fields, vec!["NETWORK", "CODENAMES_CONFIG"]);
        assert_eq!(
            checks.last().unwrap().outcome,
            Outcome::Passed("1 entries".into())
        );
    }

    #[test]
    fn an_invalid_declared_pattern_is_unknown_rather_than_a_failure() {
        let mut broken = spec();
        broken.fields[0].pattern = Some("([".into());
        let env = to_env(&broken, &values(&[("BUILDKITE_PIPELINE_FILTER", "x")]));
        let checks = check_declared(&broken, "compatible", &env);
        assert!(failed(&checks).is_empty());
        assert!(checks
            .iter()
            .any(|c| matches!(c.outcome, Outcome::Unknown(_))));
    }
}
