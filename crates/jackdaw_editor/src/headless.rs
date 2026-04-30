//! Headless operator dispatch for the per-project editor binary's
//! `--headless` mode.
//!
//! The launcher's `dispatch_editor_op` (see [`crate::operator_routing`])
//! delegates editor-scope operators to the project's editor binary by
//! spawning it with `--headless <op-id> <json>`. The binary's `main`
//! detects the flag and calls [`run_headless_operator`], which spins
//! up a Bevy `App`, registers every operator the editor knows about,
//! dispatches the request, and exits with a status code derived from
//! the [`OperatorResult`](jackdaw_api::op::OperatorResult).
//!
//! No window is opened. The render plugin is added with no GPU
//! backend so resource registration paths still run, mirroring the
//! integration-test `headless_app()` helper.

use std::process::ExitCode;

use bevy::prelude::*;
use bevy::render::{
    RenderPlugin,
    settings::{RenderCreation, WgpuSettings},
};
use bevy::winit::WinitPlugin;
use jackdaw_api::op::{
    CallOperatorError, OperatorParameters, OperatorResult, OperatorWorldExt as _,
};
use jackdaw_jsn::PropertyValue;
use jackdaw_loader::DylibLoaderPlugin;

use crate::EditorPlugins;

/// Exit code returned when an operator finishes successfully.
const EXIT_OK: u8 = 0;
/// Exit code returned when an operator's availability gate refused
/// the call (mirrors `OperatorResult::Cancelled`). Distinguished from
/// generic failure so shell callers can branch on a no-op result.
const EXIT_CANCELLED: u8 = 2;
/// Exit code returned for unparseable params, unknown ids, or any
/// other dispatch failure.
const EXIT_FAILURE: u8 = 1;

/// Build a minimal Bevy `App` with the editor's operator catalog,
/// dispatch the named operator, and return an [`ExitCode`].
///
/// `op_id` is the operator's string identifier (e.g. `scene.import_gltf`).
/// `json` is JSON-encoded params (e.g. `{"path":"/foo.glb"}`); empty
/// strings are treated as no params.
///
/// Behaviour:
/// - `OperatorResult::Finished` / `Running` -> [`ExitCode::SUCCESS`].
/// - `OperatorResult::Cancelled` -> exit code `2`.
/// - JSON parse error / unknown id / dispatch error -> [`ExitCode::FAILURE`].
#[expect(
    clippy::print_stderr,
    reason = "headless mode is a CLI-driven shell tool; errors must reach the parent process via stderr"
)]
pub fn run_headless_operator(op_id: &str, json: &str) -> ExitCode {
    let params = match parse_params_json(json) {
        Ok(params) => params,
        Err(err) => {
            eprintln!("error: failed to parse operator params: {err}");
            return ExitCode::from(EXIT_FAILURE);
        }
    };

    let mut app = build_headless_app();
    // Run the startup pass and tick once so every built-in extension
    // is registered, enabled, and its operators populated in the
    // `OperatorIndex`. Mirrors the test-suite `editor_test_app()`
    // helper.
    app.finish();
    app.update();

    let world = app.world_mut();
    let mut builder = world.operator(op_id.to_owned());
    for (key, value) in params.0 {
        builder = builder.param(key, value);
    }
    match builder.call() {
        Ok(OperatorResult::Finished | OperatorResult::Running) => ExitCode::from(EXIT_OK),
        Ok(OperatorResult::Cancelled) => ExitCode::from(EXIT_CANCELLED),
        Err(err) => {
            match &err {
                CallOperatorError::UnknownId(id) => {
                    eprintln!("error: unknown operator id: {id}");
                }
                other => {
                    eprintln!("error: operator dispatch failed: {other}");
                }
            }
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

/// Build the headless app shell: `DefaultPlugins` with no GPU backend
/// and no winit, plus `EditorPlugins` with a fully-disabled dylib
/// loader. Matches the shape of the integration tests' `headless_app`
/// so operators that rely on shared editor resources resolve.
fn build_headless_app() -> App {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(WgpuSettings {
                    backends: None,
                    ..default()
                }),
                ..default()
            })
            .disable::<WinitPlugin>(),
    )
    .add_plugins(EditorPlugins::default().set(DylibLoaderPlugin {
        extra_paths: Vec::new(),
        include_user_dir: false,
        include_env_dir: false,
    }));
    app
}

/// Parse a JSON object into [`OperatorParameters`]. Empty strings
/// produce an empty parameter map. Top-level value must be an object
/// whose values are scalars (`bool` / number / string) or 2 / 3 / 4
/// element numeric arrays (decoded as `Vec2` / `Vec3` / `Color`).
///
/// The mapping is intentionally narrow: this is the CLI / IPC entry
/// point, not a full JSN deserialiser. `Entity` parameters cannot be
/// expressed because their values are runtime-only handles.
fn parse_params_json(json: &str) -> Result<OperatorParameters, String> {
    let trimmed = json.trim();
    if trimmed.is_empty() {
        return Ok(OperatorParameters::default());
    }
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| format!("invalid JSON: {e}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "expected a JSON object at the top level".to_owned())?;
    let mut params = OperatorParameters::default();
    for (key, raw) in object {
        let property = json_value_to_property(raw)
            .ok_or_else(|| format!("unsupported value for key '{key}': {raw}"))?;
        params.insert(key.clone(), property);
    }
    Ok(params)
}

/// Map a single `serde_json::Value` to a [`PropertyValue`]. Returns
/// `None` for shapes the operator parameter system can't represent
/// (nested objects, mixed arrays, null, etc.).
fn json_value_to_property(value: &serde_json::Value) -> Option<PropertyValue> {
    match value {
        serde_json::Value::Bool(b) => Some(PropertyValue::Bool(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(PropertyValue::Int(i))
            } else {
                n.as_f64().map(PropertyValue::Float)
            }
        }
        serde_json::Value::String(s) => Some(PropertyValue::String(s.clone().into())),
        serde_json::Value::Array(items) => json_array_to_property(items),
        serde_json::Value::Null | serde_json::Value::Object(_) => None,
    }
}

/// Decode a numeric array as `Vec2`, `Vec3`, or `Color` (RGBA in `0..=1`).
/// Any other length, or non-numeric elements, returns `None`.
fn json_array_to_property(items: &[serde_json::Value]) -> Option<PropertyValue> {
    let floats: Option<Vec<f32>> = items
        .iter()
        .map(|item| item.as_f64().map(|f| f as f32))
        .collect();
    let floats = floats?;
    match floats.len() {
        2 => Some(PropertyValue::Vec2(Vec2::new(floats[0], floats[1]))),
        3 => Some(PropertyValue::Vec3(Vec3::new(
            floats[0], floats[1], floats[2],
        ))),
        4 => Some(PropertyValue::Color(Color::srgba(
            floats[0], floats[1], floats[2], floats[3],
        ))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_json_is_empty_params() {
        let params = parse_params_json("").expect("empty string should parse");
        assert!(params.0.is_empty());
    }

    #[test]
    fn whitespace_only_is_empty_params() {
        let params = parse_params_json("   \n").expect("whitespace should parse");
        assert!(params.0.is_empty());
    }

    #[test]
    fn parses_scalars() {
        let params =
            parse_params_json(r#"{"flag": true, "count": 7, "ratio": 0.5, "name": "foo"}"#)
                .expect("valid JSON should parse");
        assert_eq!(params.as_bool("flag"), Some(true));
        assert_eq!(params.as_int("count"), Some(7));
        assert_eq!(params.as_float("ratio"), Some(0.5));
        assert_eq!(params.as_str("name"), Some("foo"));
    }

    #[test]
    fn parses_vec2_and_vec3() {
        let params = parse_params_json(r#"{"xy": [1.0, 2.0], "xyz": [3, 4, 5]}"#)
            .expect("vec arrays should parse");
        match params.0.get("xy") {
            Some(PropertyValue::Vec2(v)) => assert_eq!(*v, Vec2::new(1.0, 2.0)),
            other => panic!("expected Vec2, got {other:?}"),
        }
        match params.0.get("xyz") {
            Some(PropertyValue::Vec3(v)) => assert_eq!(*v, Vec3::new(3.0, 4.0, 5.0)),
            other => panic!("expected Vec3, got {other:?}"),
        }
    }

    #[test]
    fn rejects_top_level_array() {
        assert!(parse_params_json("[1,2,3]").is_err());
    }

    #[test]
    fn rejects_invalid_json() {
        assert!(parse_params_json("{not json").is_err());
    }

    #[test]
    fn rejects_null_values() {
        assert!(parse_params_json(r#"{"x": null}"#).is_err());
    }
}
