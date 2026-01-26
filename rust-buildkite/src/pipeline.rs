use crate::schema::{self, JsonSchemaForBuildkitePipelineConfigurationFiles, PipelineSteps};
use crate::steps::Step;
use serde::{Deserialize, Serialize};
use serde_json::{self, Value};

/// Wrapper around the generated schema pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pipeline {
    inner: JsonSchemaForBuildkitePipelineConfigurationFiles,
}

impl Pipeline {
    pub fn to_yaml(&self) -> Result<String, serde_yaml::Error> {
        serde_yaml::to_string(&self.inner)
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.inner)
    }

    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml::Error> {
        Ok(Self {
            inner: serde_yaml::from_str(yaml)?,
        })
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        Ok(Self {
            inner: serde_json::from_str(json)?,
        })
    }

    pub fn into_inner(self) -> JsonSchemaForBuildkitePipelineConfigurationFiles {
        self.inner
    }
}

/// Fluent builder for pipelines.
#[derive(Debug)]
pub struct PipelineBuilder {
    inner: JsonSchemaForBuildkitePipelineConfigurationFiles,
    steps: Vec<Step>,
}

impl PipelineBuilder {
    pub fn new() -> Self {
        Self {
            inner: empty_pipeline(),
            steps: Vec::new(),
        }
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.inner
            .env
            .get_or_insert_with(|| schema::Env(serde_json::Map::new()))
            .0
            .insert(key.into(), Value::String(value.into()));
        self
    }

    pub fn step(mut self, step: impl Into<Step>) -> Self {
        self.steps.push(step.into());
        self
    }

    pub fn raw_step(mut self, step: schema::PipelineStepsItem) -> Self {
        self.steps.push(Step::raw(step));
        self
    }

    pub fn notify(mut self, notify: schema::BuildNotify) -> Self {
        self.inner.notify = Some(notify);
        self
    }

    pub fn build(mut self) -> Pipeline {
        let items = self.steps.into_iter().map(Step::into_item).collect();
        self.inner.steps = PipelineSteps(items);
        Pipeline { inner: self.inner }
    }
}

fn empty_pipeline() -> JsonSchemaForBuildkitePipelineConfigurationFiles {
    JsonSchemaForBuildkitePipelineConfigurationFiles {
        agents: None,
        env: None,
        image: None,
        notify: None,
        priority: None,
        secrets: None,
        steps: PipelineSteps(Vec::new()),
    }
}
