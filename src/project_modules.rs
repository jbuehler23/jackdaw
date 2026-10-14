//! File > Modules: change which optional modules the open project uses.
//!
//! A change rewrites `[modules]` in `jackdaw.toml` and the runtime features
//! and crates in the game's Cargo.toml together, through the same plan the
//! import review shows. The game picks the change up the next time it is
//! built.

use std::path::PathBuf;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::dialog::{
    DialogActionEvent, DialogChildrenSlot, EditorDialog, OpenDialogEvent,
};
use jackdaw_project_build::modules::{ModuleCatalog, ModuleSet};
use jackdaw_project_build::project_manifest::ProjectManifest;

use crate::module_picker::{ModulePicker, missing_modules, missing_modules_notice};

pub struct ProjectModulesPlugin;

impl Plugin for ProjectModulesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                warn_about_missing_modules,
                populate_dialog,
                close_with_dialog,
            )
                .run_if(in_state(crate::AppState::Editor)),
        )
        .add_observer(open_dialog)
        .add_observer(save_from_dialog);
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ProjectModulesOp>()
        .register_operator::<ProjectSetModuleOp>();
}

/// Show the Modules dialog.
#[derive(Event)]
pub struct OpenProjectModulesEvent;

/// Present while the Modules dialog is open.
#[derive(Resource)]
struct ProjectModulesOpen;

/// Marks the Modules dialog's picker.
#[derive(Component)]
pub struct ProjectModulesPicker;

/// Show the open project's modules for changing.
#[operator(
    id = "project.modules",
    label = "Modules...",
    description = "Choose the optional modules this project uses.",
    allows_undo = false
)]
pub fn project_modules(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.trigger(OpenProjectModulesEvent);
    OperatorResult::Finished
}

/// Turn one module on or off for the open project.
#[operator(
    id = "project.set_module",
    label = "Set Module",
    description = "Turn one of the project's optional modules on or off. Edits jackdaw.toml \
                   and the game's Cargo.toml; the game rebuilds the next time it is built.",
    allows_undo = false,
    params(
        id(String, doc = "The module: physics, navmesh or multiplayer."),
        enabled(bool, doc = "On or off.")
    )
)]
pub fn project_set_module(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (Some(id), Some(enabled)) = (params.as_str("id"), params.as_bool("enabled")) else {
        warn!("project.set_module: name a module and whether it is enabled");
        return OperatorResult::Cancelled;
    };
    let catalog = ModuleCatalog::builtin();
    if catalog.get(id).is_none() {
        warn!("project.set_module: there is no module `{id}`");
        return OperatorResult::Cancelled;
    }
    let id = id.to_string();
    commands.queue(move |world: &mut World| {
        let Some(root) = project_root(world) else {
            return;
        };
        let mut modules = match crate::scaffold::project_modules(&root) {
            Ok(modules) => modules,
            Err(error) => {
                crate::status_bar::notify_error(world, format!("Modules not changed: {error}"));
                return;
            }
        };
        if enabled {
            catalog.enable(&mut modules, &id);
        } else {
            catalog.disable(&mut modules, &id);
        }
        apply_modules(world, root, &modules);
    });
    OperatorResult::Finished
}

fn project_root(world: &World) -> Option<PathBuf> {
    world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|project| project.root.clone())
}

/// Write `modules` into the project at `root` and report the result.
fn apply_modules(world: &mut World, root: PathBuf, modules: &ModuleSet) {
    let plan = match crate::scaffold::plan_module_change(&root, modules) {
        Ok(plan) => plan,
        Err(error) => {
            crate::status_bar::notify_error(world, format!("Modules not changed: {error}"));
            return;
        }
    };
    if plan.is_empty() {
        return;
    }
    if let Err(error) = crate::scaffold::apply_import_plan(&plan) {
        crate::status_bar::notify_error(world, format!("Modules not changed: {error}"));
        return;
    }
    for note in &plan.notes {
        info!("Modules: {note}");
    }
    let names =
        match ModuleCatalog::builtin().names(&plan.modules.iter().cloned().collect::<Vec<_>>()) {
            names if names.is_empty() => "none".to_string(),
            names => names,
        };
    crate::status_bar::notify_info(
        world,
        format!("Modules: {names}. The game rebuilds on its next build."),
    );
}

fn open_dialog(
    _: On<OpenProjectModulesEvent>,
    mut commands: Commands,
    open: Option<Res<ProjectModulesOpen>>,
    project: Option<Res<crate::project::ProjectRoot>>,
) {
    if open.is_some() || project.is_none() {
        return;
    }
    commands.insert_resource(ProjectModulesOpen);
    commands.trigger(
        OpenDialogEvent::new("Modules", "Save")
            .with_description(
                "Optional modules this project uses. Saving updates jackdaw.toml and the game's \
                 Cargo.toml; the game rebuilds the next time it is built.",
            )
            .with_max_width(px(560)),
    );
}

fn populate_dialog(
    slots: Query<Entity, Added<DialogChildrenSlot>>,
    open: Option<Res<ProjectModulesOpen>>,
    project: Option<Res<crate::project::ProjectRoot>>,
    mut commands: Commands,
) {
    let (Some(_), Some(project)) = (open, project) else {
        return;
    };
    for slot in slots {
        let modules = crate::scaffold::project_modules(&project.root).unwrap_or_else(|error| {
            warn!("Modules: {error}");
            ProjectManifest::read(&project.root).modules(&project.root)
        });
        let picker = crate::module_picker::spawn_module_picker(
            &mut commands,
            slot,
            modules,
            ModuleSet::new(),
            "",
        );
        commands.entity(picker).insert(ProjectModulesPicker);
    }
}

fn save_from_dialog(
    _: On<DialogActionEvent>,
    open: Option<Res<ProjectModulesOpen>>,
    pickers: Query<&ModulePicker, With<ProjectModulesPicker>>,
    project: Option<Res<crate::project::ProjectRoot>>,
    mut commands: Commands,
) {
    let (Some(_), Some(project)) = (open, project) else {
        return;
    };
    let Some(picker) = pickers.iter().next() else {
        return;
    };
    let modules = picker.modules.clone();
    let root = project.root.clone();
    commands.queue(move |world: &mut World| apply_modules(world, root, &modules));
}

fn close_with_dialog(
    mut commands: Commands,
    open: Option<Res<ProjectModulesOpen>>,
    dialogs: Query<(), With<EditorDialog>>,
) {
    if open.is_some() && dialogs.is_empty() {
        commands.remove_resource::<ProjectModulesOpen>();
    }
}

/// Say once per opened project when it uses a module this editor lacks.
fn warn_about_missing_modules(
    project: Option<Res<crate::project::ProjectRoot>>,
    mut warned_for: Local<Option<PathBuf>>,
    mut commands: Commands,
) {
    let Some(project) = project else {
        return;
    };
    if warned_for.as_ref() == Some(&project.root) {
        return;
    }
    *warned_for = Some(project.root.clone());
    let root = project.root.clone();
    commands.queue(move |world: &mut World| {
        let missing = match crate::scaffold::project_modules(&root) {
            Ok(modules) => modules
                .into_iter()
                .filter(|id| !crate::module_picker::editor_has_module(world, id))
                .collect(),
            Err(_) => missing_modules(world, &root),
        };
        let Some(notice) = missing_modules_notice(&missing) else {
            return;
        };
        warn!("{notice}");
        crate::status_bar::notify_warn(world, notice);
    });
}
