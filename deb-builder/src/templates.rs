use anyhow::{Context, Result};
use minijinja::{context, Environment};
use serde::Serialize;

const POLICY_FILE_TEMPLATE: &str = r#"<?xml version="1.0"?>
<!DOCTYPE Policy SYSTEM "https://www.debian.org/debsig/1.0/policy.dtd">
<Policy xmlns="https://www.debian.org/debsig/1.0/">

  <!-- Here name and description can be anything. -->
  <Origin Name="Verification" id="{{ key_id }}" Description="{{ description }}" />

  <Selection>
    <Required Type="origin" File="{{ key_filename }}" id="{{ key_id }}"/>
  </Selection>

  <Verification MinOptional="0">
    <Required Type="origin" File="{{ key_filename }}" id="{{ key_id }}"/>
  </Verification>

</Policy>
"#;

const DEBIAN_CONTROL_FILE_TEMPLATE: &str = r#"
{%- autoescape false -%}
{% for property in properties %}{{ property.name }}: {{ property.value }}
{% endfor %}Description:
 {{ description }}
 Built from {{ githash }} by {{ buildurl }}
{% endautoescape -%}
"#;

pub struct PolicyFileInput<'a> {
    pub key_filename: &'a str,
    pub key_id: &'a str,
    pub description: &'a str,
}

pub fn format_policy_file(input: &PolicyFileInput<'_>) -> Result<String> {
    let mut env = Environment::new();
    env.add_template("policy", POLICY_FILE_TEMPLATE)?;
    let tmpl = env.get_template("policy")?;
    tmpl.render(context! {
        key_filename => input.key_filename,
        key_id => input.key_id,
        description => input.description,
    })
    .context("rendering policy template")
}

#[derive(Debug, Clone)]
pub struct DebianControl {
    pub package_name: String,
    pub version: String,
    pub vendor: String,
    pub package_authors: String,
    pub package_maintainer: String,
    pub package_description: String,
    pub package_section: String,
    pub package_priority: String,
    pub package_homepage: String,
    pub package_installed_size: String,
    pub package_source: String,
    pub architecture: String,
    pub suite: String,
    pub codename: String,
    pub depends: Option<Vec<String>>,
    pub suggested_depends: Option<Vec<String>>,
    pub recommended_depends: Option<Vec<String>>,
    pub pre_depends: Option<Vec<String>>,
    pub conflicts: Option<Vec<String>>,
    pub replaces: Option<Vec<String>>,
    pub provides: Option<Vec<String>>,
    pub license: String,
    pub githash: String,
    pub buildurl: String,
}

#[derive(Serialize)]
struct Property<'a> {
    name: &'a str,
    value: String,
}

// NOTE: parity with OCaml — the original `format_control_file` only emits the
// 12 hardcoded fields below.  Depends / Suggests / Recommends / Pre-Depends /
// Conflicts / Replaces / Provides / Vendor / Authors are stored on the input
// struct (and validated by the CLI) but are NOT written to the control file.
// This appears to be a latent bug in the OCaml original; preserving behavior
// for now, to be revisited as a separate change once parity is established.
pub fn format_control_file(input: &DebianControl) -> Result<String> {
    let mut env = Environment::new();
    env.add_template("control", DEBIAN_CONTROL_FILE_TEMPLATE)?;
    let tmpl = env.get_template("control")?;

    let properties: Vec<Property> = vec![
        Property { name: "Package", value: input.package_name.clone() },
        Property { name: "Version", value: input.version.clone() },
        Property { name: "Architecture", value: input.architecture.clone() },
        Property { name: "Maintainer", value: input.package_maintainer.clone() },
        Property { name: "Section", value: input.package_section.clone() },
        Property { name: "Priority", value: input.package_priority.clone() },
        Property { name: "Homepage", value: input.package_homepage.clone() },
        Property { name: "Installed-Size", value: input.package_installed_size.clone() },
        Property { name: "Source", value: input.package_source.clone() },
        Property { name: "Suite", value: input.suite.clone() },
        Property { name: "Codename", value: input.codename.clone() },
        Property { name: "License", value: input.license.clone() },
    ];

    tmpl.render(context! {
        description => &input.package_description,
        githash => &input.githash,
        buildurl => &input.buildurl,
        properties => properties,
    })
    .context("rendering control template")
}
