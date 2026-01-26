//! Buildkite pipeline bindings backed by the official schema.
//!
//! ```rust
//! use buildkite_pipeline::{CommandStepBuilder, PipelineBuilder};
//!
//! let pipeline = PipelineBuilder::new()
//!     .env("CI", "true")
//!     .step(CommandStepBuilder::new("cargo test").label(":test_tube: unit"))
//!     .build();
//!
//! println!("{}", pipeline.to_yaml().unwrap());
//! ```

pub mod pipeline;
pub mod schema;
pub mod steps;
pub mod version;

pub use pipeline::{Pipeline, PipelineBuilder};
pub use steps::{CommandStepBuilder, Step};

/// Convenience exports for quick prototyping.
pub mod prelude {
    pub use crate::schema;
    pub use crate::{CommandStepBuilder, Pipeline, PipelineBuilder, Step};
}
