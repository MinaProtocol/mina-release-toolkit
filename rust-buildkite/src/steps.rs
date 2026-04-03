use crate::schema::{self, PipelineStepsItem};
use serde_json::{self, Value};

/// A schema-backed representation of pipeline steps.
#[derive(Debug, Clone)]
pub enum Step {
    Command(CommandStepBuilder),
    Raw(schema::PipelineStepsItem),
}

impl Step {
    pub fn raw(item: schema::PipelineStepsItem) -> Self {
        Step::Raw(item)
    }

    pub(crate) fn into_item(self) -> PipelineStepsItem {
        match self {
            Step::Command(cmd) => cmd.into_item(),
            Step::Raw(item) => item,
        }
    }
}

/// Builder for command steps.
#[derive(Debug, Clone)]
pub struct CommandStepBuilder {
    inner: schema::CommandStep,
}

impl CommandStepBuilder {
    /// Create a command step that runs a single command string.
    pub fn new(command: impl Into<String>) -> Self {
        let mut inner = schema::CommandStep::default();
        inner.command = Some(schema::CommandStepCommand::Variant1(command.into()));
        Self { inner }
    }

    /// Provide multiple commands (run sequentially).
    pub fn commands<I, S>(commands: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut inner = schema::CommandStep::default();
        inner.command = Some(schema::CommandStepCommand::Variant0(
            commands.into_iter().map(Into::into).collect(),
        ));
        Self { inner }
    }

    /// Set the label for the step.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.inner.label = Some(schema::Label(label.into()));
        self
    }

    /// Assign a unique key to the step.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.inner.key = Some(schema::Key(key.into()));
        self
    }

    /// Add an environment variable.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.inner
            .env
            .get_or_insert_with(|| schema::Env(serde_json::Map::new()))
            .0
            .insert(key.into(), Value::String(value.into()));
        self
    }

    /// Configure matrix execution.
    pub fn matrix(mut self, matrix: schema::Matrix) -> Self {
        self.inner.matrix = Some(matrix);
        self
    }

    /// Configure retries.
    pub fn retry(mut self, retry: schema::CommandStepRetry) -> Self {
        self.inner.retry = Some(retry);
        self
    }

    /// Set plugins.
    pub fn plugins(mut self, plugins: schema::Plugins) -> Self {
        self.inner.plugins = Some(plugins);
        self
    }

    /// Number of parallel jobs spawned from this step.
    pub fn parallelism(mut self, parallelism: i64) -> Self {
        self.inner.parallelism = Some(parallelism);
        self
    }

    /// Access the underlying schema struct for advanced customization.
    pub fn customize(mut self, f: impl FnOnce(&mut schema::CommandStep)) -> Self {
        f(&mut self.inner);
        self
    }

    /// Finalize the builder.
    pub fn build(self) -> schema::CommandStep {
        self.inner
    }

    fn into_item(self) -> PipelineStepsItem {
        let mut item = PipelineStepsItem::default();
        item.subtype_6 = Some(self.inner);
        item
    }
}

impl From<CommandStepBuilder> for Step {
    fn from(value: CommandStepBuilder) -> Self {
        Step::Command(value)
    }
}
