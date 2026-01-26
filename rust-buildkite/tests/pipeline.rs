use buildkite_pipeline::{CommandStepBuilder, Pipeline, PipelineBuilder};

#[test]
fn builds_simple_pipeline() {
    let pipeline = PipelineBuilder::new()
        .env("CI", "true")
        .step(
            CommandStepBuilder::new("cargo test")
                .label(":test_tube: tests")
                .key("tests"),
        )
        .build();

    let yaml = pipeline.to_yaml().unwrap();
    assert!(yaml.contains("cargo test"));
    assert!(yaml.contains("tests"));
}

#[test]
fn roundtrip_json() {
    let pipeline = PipelineBuilder::new()
        .step(CommandStepBuilder::new("echo hello"))
        .build();

    let json = pipeline.to_json().unwrap();
    let parsed = Pipeline::from_json(&json).unwrap();
    let reparsed = parsed.to_json().unwrap();
    assert!(reparsed.contains("echo hello"));
}
