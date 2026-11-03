//! A checkbox list of the optional project modules, shared by the New
//! Project dialog, the import review and File > Modules.
//!
//! Turning a module on turns on what it requires, and turning one off turns
//! off what requires it; a note under the list says when that happened. The
//! picker's [`ModulePicker`] component holds the choice for whoever owns it.

use std::path::Path;

use bevy::{
    feathers::{controls::FeathersCheckbox, theme::ThemedText},
    prelude::*,
    ui::{Checked, InteractionDisabled},
    ui_widgets::ValueChange,
};
use jackdaw_feathers::tokens;
use jackdaw_project_build::modules::{ModuleCatalog, ModuleSet};
use jackdaw_project_build::project_manifest::ProjectManifest;

pub struct ModulePickerPlugin;

impl Plugin for ModulePickerPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_module_toggled)
            .add_systems(Update, sync_module_pickers);
    }
}

/// Whether this editor has the tooling for module `id`: the extension the
/// module names is installed, or the tooling is built in.
pub fn editor_has_module(world: &World, id: &str) -> bool {
    catalog_has_module(
        world.get_resource::<jackdaw_api_internal::ExtensionCatalog>(),
        id,
    )
}

/// [`editor_has_module`] given the installed extensions.
pub fn catalog_has_module(
    catalog: Option<&jackdaw_api_internal::ExtensionCatalog>,
    id: &str,
) -> bool {
    if let Some(extension) = ModuleCatalog::builtin()
        .get(id)
        .and_then(|module| module.extension.as_deref())
    {
        return catalog.is_some_and(|catalog| catalog.contains(extension));
    }
    id == "multiplayer" && cfg!(feature = "multiplayer")
}

/// The modules the project at `root` uses that this editor does not include.
pub fn missing_modules(world: &World, root: &Path) -> Vec<String> {
    ProjectManifest::read(root)
        .modules(root)
        .into_iter()
        .filter(|id| !editor_has_module(world, id))
        .collect()
}

/// What to tell someone opening a project that uses `missing` modules.
pub fn missing_modules_notice(missing: &[String]) -> Option<String> {
    if missing.is_empty() {
        return None;
    }
    let names = ModuleCatalog::builtin().names(missing);
    let noun = if missing.len() == 1 {
        "module"
    } else {
        "modules"
    };
    Some(format!(
        "This project uses the {names} {noun}, which this editor does not include. Scenes keep \
         those components, but they cannot be edited here."
    ))
}

/// The modules chosen in one picker.
#[derive(Component, Clone, Debug, Default)]
pub struct ModulePicker {
    /// The modules turned on.
    pub modules: ModuleSet,
    /// Modules shown on that cannot be turned off here.
    pub locked: ModuleSet,
    /// What the last toggle also changed, shown under the list.
    pub note: String,
}

/// One module's checkbox inside a [`ModulePicker`].
#[derive(Component, Clone, Debug)]
pub struct ModuleToggle {
    /// The picker this checkbox belongs to.
    pub picker: Entity,
    /// The module's id.
    pub id: String,
}

/// Marks the note line of a picker.
#[derive(Component)]
struct ModulePickerNote(Entity);

/// Spawn a picker under `parent`, starting with `modules` on. `locked`
/// modules, and what they require, show as on and cannot be changed;
/// `locked_reason` says why.
pub fn spawn_module_picker(
    commands: &mut Commands,
    parent: Entity,
    modules: ModuleSet,
    locked: ModuleSet,
    locked_reason: &str,
) -> Entity {
    let catalog = ModuleCatalog::builtin();
    let locked = catalog.complete(locked);
    let modules = catalog.complete(modules.union(&locked).cloned().collect());
    let picker = commands
        .spawn((
            ModulePicker {
                modules: modules.clone(),
                locked: locked.clone(),
                note: String::new(),
            },
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(tokens::SPACING_SM),
                width: percent(100),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();
    for module in catalog.modules() {
        let row = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(2.0),
                    ..Default::default()
                },
                ChildOf(picker),
            ))
            .id();
        let mut checkbox = commands.spawn_scene(module_checkbox(module.name.clone()));
        checkbox.insert((
            ModuleToggle {
                picker,
                id: module.id.clone(),
            },
            ChildOf(row),
        ));
        if modules.contains(&module.id) {
            checkbox.insert(Checked);
        }
        let unavailable = module
            .unavailable
            .as_deref()
            .filter(|_| !modules.contains(&module.id));
        if locked.contains(&module.id) || unavailable.is_some() {
            checkbox.insert(InteractionDisabled);
        }
        let mut detail = module.description.clone();
        if locked.contains(&module.id) && !locked_reason.is_empty() {
            detail = format!("{detail} {locked_reason}");
        }
        if let Some(reason) = unavailable {
            detail = format!("{detail} Not compatible: {reason}.");
        }
        commands.spawn((
            Text::new(detail),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            Node {
                margin: UiRect::left(px(26.0)),
                ..Default::default()
            },
            ChildOf(row),
        ));
        let id = module.id.clone();
        commands.queue(move |world: &mut World| {
            if editor_has_module(world, &id) || world.get_entity(row).is_err() {
                return;
            }
            world.spawn((
                Text::new("This editor does not include this module."),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_WARNING),
                Node {
                    margin: UiRect::left(px(26.0)),
                    ..Default::default()
                },
                ChildOf(row),
            ));
        });
    }
    commands.spawn((
        ModulePickerNote(picker),
        Text::new(String::new()),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_ACCENT),
        Node {
            display: Display::None,
            ..Default::default()
        },
        ChildOf(picker),
    ));
    picker
}

fn module_checkbox(name: String) -> impl Scene {
    bsn! {
        @FeathersCheckbox {
            @caption: bsn! { Text(name) ThemedText }
        }
    }
}

/// Apply a checkbox change to its picker, following the module requirements.
fn on_module_toggled(
    change: On<ValueChange<bool>>,
    toggles: Query<&ModuleToggle>,
    mut pickers: Query<&mut ModulePicker>,
) {
    let Ok(toggle) = toggles.get(change.source) else {
        return;
    };
    if pickers
        .get(toggle.picker)
        .is_ok_and(|picker| picker.locked.contains(&toggle.id))
    {
        return;
    }
    let Ok(mut picker) = pickers.get_mut(toggle.picker) else {
        return;
    };
    let picker = &mut *picker;
    let catalog = ModuleCatalog::builtin();
    if change.value && catalog.unavailable(&toggle.id).is_some() {
        return;
    }
    let name = |id: &str| catalog.get(id).map_or(id.to_string(), |m| m.name.clone());
    picker.note = if change.value {
        let also = catalog.enable(&mut picker.modules, &toggle.id);
        match also.as_slice() {
            [] => String::new(),
            _ => format!(
                "{} needs {}, so it is on too.",
                name(&toggle.id),
                catalog.names(&also)
            ),
        }
    } else {
        let also = catalog.disable(&mut picker.modules, &toggle.id);
        match also.as_slice() {
            [] => String::new(),
            _ => format!(
                "{} needs {}, so it is off too.",
                catalog.names(&also),
                name(&toggle.id)
            ),
        }
    };
}

/// Keep each picker's checkboxes and note in step with its choice.
fn sync_module_pickers(
    pickers: Query<(Entity, &ModulePicker), Changed<ModulePicker>>,
    toggles: Query<(Entity, &ModuleToggle, Has<Checked>)>,
    mut notes: Query<(&ModulePickerNote, &mut Text, &mut Node)>,
    mut commands: Commands,
) {
    for (picker_entity, picker) in &pickers {
        for (entity, toggle, checked) in &toggles {
            if toggle.picker != picker_entity {
                continue;
            }
            let on = picker.modules.contains(&toggle.id);
            if on && !checked {
                commands.entity(entity).insert(Checked);
            } else if !on && checked {
                commands.entity(entity).remove::<Checked>();
            }
        }
        for (note, mut text, mut node) in &mut notes {
            if note.0 != picker_entity {
                continue;
            }
            if text.0 != picker.note {
                text.0.clone_from(&picker.note);
            }
            let display = if picker.note.is_empty() {
                Display::None
            } else {
                Display::Flex
            };
            if node.display != display {
                node.display = display;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_using_a_module_this_editor_lacks_is_told_so() {
        assert!(missing_modules_notice(&[]).is_none());
        let notice = missing_modules_notice(&["physics".to_string()]).expect("a notice");
        assert!(
            notice.starts_with("This project uses the Physics module, which this editor"),
            "{notice}"
        );
        let notice = missing_modules_notice(&["navmesh".to_string(), "physics".to_string()])
            .expect("a notice");
        assert!(notice.contains("Physics and Navmesh modules"), "{notice}");
    }
}
