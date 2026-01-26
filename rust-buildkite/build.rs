use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

use prettyplease::unparse;
use schemars::schema::RootSchema;
use serde_json::Value;
use typify::{TypeSpace, TypeSpaceSettings};

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=schema/pipeline.schema.json");

    let schema_path = PathBuf::from("schema/pipeline.schema.json");
    let schema_contents = fs::read_to_string(&schema_path)?;
    let mut schema_value: Value = serde_json::from_str(&schema_contents)?;
    normalize_bool_strings(&mut schema_value);
    hoist_property_refs(&mut schema_value)?;
    strip_not_constraints(&mut schema_value);

    let root: RootSchema = serde_json::from_value(schema_value)?;

    let mut settings = TypeSpaceSettings::default();
    settings.with_struct_builder(true);
    settings.with_derive("PartialEq".into());
    settings.with_derive("Eq".into());

    let mut type_space = TypeSpace::new(&settings);
    type_space.add_root_schema(root)?;

    let tokens = type_space.to_stream();
    let syntax = syn::parse2::<syn::File>(tokens)?;
    let formatted = unparse(&syntax);

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    fs::write(out_dir.join("schema_types.rs"), formatted)?;
    Ok(())
}

fn normalize_bool_strings(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(replacement) = map
                .get("enum")
                .and_then(|v| v.as_array())
                .and_then(|arr| bool_string_enum(arr))
            {
                map.remove("enum");
                map.insert("anyOf".to_string(), replacement);
            }

            for v in map.values_mut() {
                normalize_bool_strings(v);
            }
        }
        Value::Array(items) => {
            for v in items {
                normalize_bool_strings(v);
            }
        }
        _ => {}
    }
}

fn bool_string_enum(values: &[Value]) -> Option<Value> {
    if values.len() != 4 {
        return None;
    }

    let mut seen_bool_true = false;
    let mut seen_bool_false = false;
    let mut seen_string_true = false;
    let mut seen_string_false = false;

    for value in values {
        match value {
            Value::Bool(true) => seen_bool_true = true,
            Value::Bool(false) => seen_bool_false = true,
            Value::String(s) if s == "true" => seen_string_true = true,
            Value::String(s) if s == "false" => seen_string_false = true,
            _ => return None,
        }
    }

    if seen_bool_true && seen_bool_false && seen_string_true && seen_string_false {
        let any_of = Value::Array(vec![
            serde_json::json!({ "type": "boolean" }),
            serde_json::json!({ "type": "string", "enum": ["true", "false"] }),
        ]);
        Some(any_of)
    } else {
        None
    }
}

fn hoist_property_refs(value: &mut Value) -> Result<(), Box<dyn Error>> {
    let mut refs = Vec::new();
    collect_refs(value, &mut refs);
    refs.sort();
    refs.dedup();

    let mut updates = Vec::new();
    for reference in refs {
        if let Some((pointer, new_name)) = property_ref_target(&reference) {
            let schema = value.pointer(&pointer).cloned();
            updates.push((reference, pointer, new_name, schema));
        }
    }

    {
        let definitions = value
            .get_mut("definitions")
            .and_then(|v| v.as_object_mut())
            .ok_or_else(|| "schema missing definitions".to_string())?;

        for (_, pointer, new_name, schema) in &updates {
            if definitions.contains_key(new_name) {
                continue;
            }

            let schema_value = schema
                .clone()
                .ok_or_else(|| format!("missing schema for pointer {}", pointer))?;
            definitions.insert(new_name.clone(), schema_value);
        }
    }

    for (reference, _, new_name, _) in updates {
        replace_refs(value, &reference, &format!("#/definitions/{}", new_name));
    }

    Ok(())
}

fn collect_refs(value: &Value, refs: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(reference)) = map.get("$ref") {
                refs.push(reference.clone());
            }
            for v in map.values() {
                collect_refs(v, refs);
            }
        }
        Value::Array(items) => {
            for v in items {
                collect_refs(v, refs);
            }
        }
        _ => {}
    }
}

fn replace_refs(value: &mut Value, from: &str, to: &str) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(r)) = map.get_mut("$ref") {
                if r == from {
                    *r = to.to_string();
                }
            }
            for v in map.values_mut() {
                replace_refs(v, from, to);
            }
        }
        Value::Array(items) => {
            for v in items {
                replace_refs(v, from, to);
            }
        }
        _ => {}
    }
}

fn strip_not_constraints(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("not");
            for v in map.values_mut() {
                strip_not_constraints(v);
            }
        }
        Value::Array(items) => {
            for v in items {
                strip_not_constraints(v);
            }
        }
        _ => {}
    }
}

fn property_ref_target(reference: &str) -> Option<(String, String)> {
    const PREFIX: &str = "#/definitions/";
    if !reference.starts_with(PREFIX) || !reference.contains("/properties/") {
        return None;
    }

    let pointer = reference.trim_start_matches('#').to_string();
    let rest = &reference[PREFIX.len()..];
    let sanitized = rest.trim_start_matches('/').replace('/', "_");

    Some((pointer, sanitized))
}
