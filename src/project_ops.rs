//! Operators for project lifecycle (scaffold, open, build).
//!
//! These wrap [`crate::new_project`] / [`crate::project`] /
//! [`crate::ext_build`] in operator form so they're scriptable from
//! the CLI (`jackdaw <op-id> '<json>'`) and exposed to extension
//! authors who want to drive the editor programmatically.

use std::path::PathBuf;

use bevy::prelude::*;
use jackdaw_api::prelude::*;

use crate::new_project::{TemplatePreset, scaffold_project};

/// Scaffold a new project from a template via cargo-generate.
///
/// JSON parameters:
///
/// ```json
/// {
///   "path": "/path/to/parent/dir",
///   "name": "my_game",
///   "kind": "game" | "extension",
///   "linkage": "static" | "dylib"
/// }
/// ```
///
/// `path` is the parent directory; the scaffolder creates
/// `<path>/<name>` and emits the project there. All four parameters
/// are required.
///
/// Blocks the calling thread on `bevy new` invocation. From the UI
/// path (which calls this operator from `project_select`), this is
/// already wrapped in an `AsyncComputeTaskPool` task. From the CLI,
/// this runs in the foreground; the headless app boots, runs this
/// operator, exits.
#[operator(
    id = "project.new",
    label = "New Project",
    description = "Scaffold a new game or extension project.",
    params(
        path(String, doc = "Parent directory the project is created under."),
        name(String, doc = "Project name (used as directory and crate name)."),
        kind(String, default = "game", doc = "Project kind: 'game' or 'extension'."),
        linkage(String, default = "static", doc = "Linkage: 'static' or 'dylib'."),
        branch(
            String,
            default = "",
            doc = "Optional template branch / tag (empty = main)."
        ),
    )
)]
pub fn project_new(In(params): In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let Some(path) = params.as_str("path").map(PathBuf::from) else {
        warn!("project.new: missing required `path` parameter");
        return OperatorResult::Cancelled;
    };
    let Some(name) = params.as_str("name").map(str::to_owned) else {
        warn!("project.new: missing required `name` parameter");
        return OperatorResult::Cancelled;
    };
    let kind = params.as_str("kind").unwrap_or("game").to_owned();
    let linkage = params.as_str("linkage").unwrap_or("static").to_owned();
    let branch = params
        .as_str("branch")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let preset = match kind.as_str() {
        "game" => TemplatePreset::Game,
        "extension" => TemplatePreset::Extension,
        other => {
            warn!("project.new: unknown `kind` '{other}' (want 'game' or 'extension')");
            return OperatorResult::Cancelled;
        }
    };
    let template_linkage = match linkage.as_str() {
        "static" => crate::new_project::TemplateLinkage::Static,
        "dylib" => crate::new_project::TemplateLinkage::Dylib,
        other => {
            warn!("project.new: unknown `linkage` '{other}' (want 'static' or 'dylib')");
            return OperatorResult::Cancelled;
        }
    };
    let template_url = preset.url(template_linkage);

    commands.queue(move |_world: &mut World| {
        match scaffold_project(
            &name,
            &path,
            &template_url,
            branch.as_deref(),
            template_linkage,
        ) {
            Ok(project_path) => {
                info!("project.new: scaffolded {}", project_path.display());
            }
            Err(err) => {
                warn!("project.new: {err}");
            }
        }
    });

    OperatorResult::Finished
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ProjectNewOp>();
}
