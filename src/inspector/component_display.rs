use crate::EditorEntity;
use crate::custom_properties::CustomProperties;
use crate::default_style;
use crate::prelude::*;
use crate::selection::Selection;
use std::any::TypeId;

use bevy::ecs::component::ComponentInfo;
use bevy::{
    ecs::{
        archetype::{Archetype, ArchetypeId},
        component::{ComponentId, Components},
        reflect::{AppTypeRegistry, ReflectComponent},
    },
    feathers::containers::{pane, pane_body, pane_header},
    feathers::controls::{ButtonVariant, FeathersDisclosureToggle, FeathersToolButton},
    prelude::*,
    reflect::serde::TypedReflectSerializer,
    ui::Checked,
    ui_widgets::ToggleChecked,
};
use jackdaw_feathers::{
    button::ButtonOperatorCall,
    icons::{EditorFont, Icon, IconFont, icon_scene},
    tokens,
};
use jackdaw_localization::LocalizedText;
use jackdaw_project_build::modules::ModuleCatalog;
use jackdaw_widgets::collapsible::{CollapsibleBody, CollapsibleHeader, CollapsibleSection};

use jackdaw_feathers::text_edit::TextEditValue;
use std::collections::{HashMap, HashSet};

use super::{
    ComponentDisplay, ComponentDisplayBody, ComponentDisplayTypePath, ComponentName,
    ComponentPicker, Inspector, InspectorDirty, InspectorGroupSection, InspectorSearch,
    InspectorTarget, ReflectDisplayable, animation_graph_card, bindings_card, brush_display,
    category_strip::ActiveInspectorCategory, component_tooltip::ReflectedTypeTooltip,
    custom_props_display, material_display, modifier_display, node_card, reflect_fields,
};
use crate::inspector::prefab_field_dots::{PrefabInstanceCtx, inspector_type_paths_for};
use crate::prefab::PrefabAstCache;
use crate::type_metadata::{TypeChrome, TypeMetadata};
use bevy::picking::hover::Hovered;

/// What the inspector reads to place its target: the entity's parents, the
/// prefab it instances, and whether it edits a definition asset rather than a
/// scene entity. Bundled into one param so the systems that read it stay under
/// the system param-count limit.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct InspectorLineage<'w, 's> {
    pub(crate) child_of: Query<'w, 's, &'static bevy::ecs::hierarchy::ChildOf>,
    pub(crate) is_a: Query<'w, 's, &'static crate::prefab::IsA>,
    pub(crate) definitions: Query<'w, 's, (), With<crate::definition_assets::DefinitionAssetEdit>>,
    pub(crate) files: Query<'w, 's, (), With<super::file_card::SelectedFile>>,
}

/// The live scene-document resource bundled into one param so the systems
/// that read it stay under the system param-count limit.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SceneAsts<'w> {
    pub(crate) bsn: Res<'w, jackdaw_bsn::SceneBsnAst>,
    pub(crate) project_types: Res<'w, crate::project_types::ProjectTypes>,
    pub(crate) type_metadata: Res<'w, crate::type_metadata::TypeMetadata>,
}

/// What a panel builds its cards from, bundled into one param so the systems
/// that build them stay under the system param-count limit.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CardBuildInputs<'w, 's> {
    components: &'w Components,
    type_registry: Res<'w, AppTypeRegistry>,
    names: Query<'w, 's, &'static Name>,
    icon_font: Res<'w, IconFont>,
    editor_font: Res<'w, EditorFont>,
    materials: Res<'w, Assets<StandardMaterial>>,
    asts: SceneAsts<'w>,
    prefab_cache: Res<'w, PrefabAstCache>,
    collapse_state: Res<'w, super::InspectorCollapseState>,
}

/// What a panel's cards were last built for: how many entities were selected,
/// for the multi-selection header, the components of the entity shown that
/// have a card, so a component that gets one brings the cards in line while
/// one that never shows does not, and the keys of the cards in panel order.
#[derive(Component)]
pub(crate) struct InspectorShown {
    selection_count: usize,
    shape: Option<(ArchetypeId, Vec<ComponentId>)>,
    cards: Vec<String>,
}

impl InspectorShown {
    fn new(
        selection_count: usize,
        shape: Option<(ArchetypeId, Vec<ComponentId>)>,
        cards: Vec<String>,
    ) -> Self {
        Self {
            selection_count,
            shape,
            cards,
        }
    }

    /// Whether the entity has left the archetype the cards were built from,
    /// which is the only time its carded components can differ.
    fn moved(&self, archetype: &Archetype) -> bool {
        self.shape
            .as_ref()
            .is_some_and(|(id, _)| *id != archetype.id())
    }

    /// Whether `carded` differs from the components the cards show. When it
    /// does not, the new archetype is taken as the one shown, so the comparison
    /// is not repeated every frame.
    fn reshaped(&mut self, archetype: &Archetype, carded: Vec<ComponentId>) -> bool {
        let Some((id, components)) = self.shape.as_mut() else {
            return false;
        };
        if *components != carded {
            return true;
        }
        *id = archetype.id();
        false
    }
}

/// Keep each inspector's cards pointed at [`Selection::primary`], and bring
/// them in line when what they show has changed: another entity or selection
/// size, an [`InspectorDirty`] marker on the entity shown, or a component added
/// to or taken from it. Another entity or selection size puts the whole panel
/// up again. On the same entity only the cards that came or went are put up or
/// taken down, and the ones that stay keep their place in the panel and their
/// state. A marker that changes no card puts the panel up again, since its
/// callers mark the entity when a card's own layout has to follow its
/// component. However many of those land in a frame, a panel is rebuilt at
/// most once. A reselect of the entity already shown leaves the panel
/// standing. An empty selection takes an open file or definition card down
/// with it.
pub(crate) fn sync_inspector_to_selection(
    mut commands: Commands,
    selection: Res<Selection>,
    entity_query: Query<(&Archetype, EntityRef), (Without<EditorEntity>, Without<Inspector>)>,
    mut inspectors: Query<
        (
            Entity,
            Option<&InspectorTarget>,
            Option<&mut InspectorShown>,
        ),
        With<Inspector>,
    >,
    dirty: Query<Entity, With<InspectorDirty>>,
    lineage: InspectorLineage,
    inputs: CardBuildInputs,
) {
    let desired = selection.primary();
    if desired.is_none() && selection.is_changed() {
        commands.queue(|world: &mut World| {
            super::file_card::clear_selected_file(world);
            crate::definition_assets::close_open_definition(world);
        });
    }

    let sel_count = selection.entities.len();

    for (inspector, target, mut shown) in &mut inspectors {
        let current = target.map(|t| t.0);
        let retarget = current != desired
            || shown
                .as_ref()
                .is_some_and(|shown| shown.selection_count != sel_count);
        let marked = current.is_some_and(|current| dirty.contains(current));
        let reshaped = current.zip(shown.as_mut()).is_some_and(|(current, shown)| {
            entity_query
                .get(current)
                .is_ok_and(|(archetype, entity_ref)| {
                    shown.moved(archetype) && {
                        let authored = inspector_type_paths_for(
                            &inputs.asts.bsn,
                            &inputs.prefab_cache,
                            current,
                            entity_ref,
                            &lineage.child_of,
                            &lineage.is_a,
                        );
                        shown.reshaped(archetype, carded_components(archetype, &inputs, &authored))
                    }
                })
        });
        if !(retarget || marked || reshaped) {
            continue;
        }

        if !retarget
            && let Some(current) = current
            && let Some(shown) = shown.as_mut().filter(|shown| shown.shape.is_some())
            && let Ok((archetype, entity_ref)) = entity_query.get(current)
        {
            let authored_type_paths = inspector_type_paths_for(
                &inputs.asts.bsn,
                &inputs.prefab_cache,
                current,
                entity_ref,
                &lineage.child_of,
                &lineage.is_a,
            );
            let cards = EntityCards {
                inputs: &inputs,
                source: current,
                archetype,
                entity_ref,
                inspector,
                selection_count: sel_count,
                authored_type_paths: &authored_type_paths,
            };
            let rebuild = marked && cards.build(&mut commands, &|_| true) == shown.cards;
            if !rebuild {
                let standing: HashSet<&str> = shown.cards.iter().map(String::as_str).collect();
                let planned = cards.build(&mut commands, &|key| standing.contains(key));
                arrange_cards(&mut commands, inspector, planned.clone());
                shown.shape = Some((
                    archetype.id(),
                    carded_components(archetype, &inputs, &authored_type_paths),
                ));
                shown.cards = planned;
                continue;
            }
        }

        if let Some(primary) = desired
            && current.is_none()
            && entity_query.get(primary).is_err()
            && !lineage.definitions.contains(primary)
            && !lineage.files.contains(primary)
        {
            continue;
        }

        commands
            .entity(inspector)
            .remove::<(InspectorTarget, InspectorShown)>();
        despawn_inspector_display_children(&mut commands, inspector);

        let Some(primary) = desired else {
            continue;
        };
        if lineage.files.contains(primary) {
            commands.queue(move |world: &mut World| {
                super::file_card::fill_file_card(world, inspector, primary);
            });
            commands.entity(inspector).insert((
                InspectorTarget(primary),
                InspectorShown::new(sel_count, None, Vec::new()),
            ));
            continue;
        }
        if lineage.definitions.contains(primary) {
            commands.queue(move |world: &mut World| {
                super::definition_card::fill_definition_card(world, inspector, primary);
            });
            commands.entity(inspector).insert((
                InspectorTarget(primary),
                InspectorShown::new(sel_count, None, Vec::new()),
            ));
            continue;
        }
        let Ok((archetype, entity_ref)) = entity_query.get(primary) else {
            continue;
        };

        let source_entity = entity_ref.entity();
        let authored_type_paths = inspector_type_paths_for(
            &inputs.asts.bsn,
            &inputs.prefab_cache,
            source_entity,
            entity_ref,
            &lineage.child_of,
            &lineage.is_a,
        );

        let cards = EntityCards {
            inputs: &inputs,
            source: source_entity,
            archetype,
            entity_ref,
            inspector,
            selection_count: sel_count,
            authored_type_paths: &authored_type_paths,
        }
        .build(&mut commands, &|_| false);

        commands.entity(inspector).insert((
            InspectorTarget(primary),
            InspectorShown::new(
                sel_count,
                Some((
                    archetype.id(),
                    carded_components(archetype, &inputs, &authored_type_paths),
                )),
                cards,
            ),
        ));
    }

    for marked in &dirty {
        commands.entity(marked).try_remove::<InspectorDirty>();
    }
}

/// The entity a panel shows its cards for, with everything they are built
/// from.
struct EntityCards<'a, 'w, 's> {
    inputs: &'a CardBuildInputs<'w, 's>,
    source: Entity,
    archetype: &'a Archetype,
    entity_ref: EntityRef<'a>,
    inspector: Entity,
    selection_count: usize,
    authored_type_paths: &'a HashSet<String>,
}

impl EntityCards<'_, '_, '_> {
    /// Put up the cards `standing` does not name, and return the keys of all
    /// of them in panel order. See [`build_inspector_cards`].
    fn build(&self, commands: &mut Commands, standing: &dyn Fn(&str) -> bool) -> Vec<String> {
        let inputs = self.inputs;
        build_inspector_cards(
            commands,
            inputs.components,
            &inputs.type_registry,
            self.source,
            self.archetype,
            self.entity_ref,
            self.inspector,
            self.selection_count,
            &inputs.names,
            &inputs.icon_font,
            &inputs.editor_font,
            &inputs.materials,
            self.authored_type_paths,
            Some(&inputs.asts.bsn),
            Some(&inputs.prefab_cache),
            &inputs.collapse_state,
            &inputs.asts.project_types,
            &inputs.asts.type_metadata,
            standing,
        )
    }
}

/// Names a card directly under the inspector, so a card that stays when the
/// cards around it change is told apart from one that has to be put up again.
/// Two cards share a key only when they would be built the same.
#[derive(Component)]
struct InspectorCardKey(String);

/// The order of a panel's cards by key, and which of them already stand.
struct CardPlan<'a> {
    keys: Vec<String>,
    repeats: HashMap<String, usize>,
    standing: &'a dyn Fn(&str) -> bool,
}

impl<'a> CardPlan<'a> {
    fn new(standing: &'a dyn Fn(&str) -> bool) -> Self {
        Self {
            keys: Vec::new(),
            repeats: HashMap::new(),
            standing,
        }
    }

    /// Place the card `key` next in the panel. Returns its key, numbered when
    /// it repeats an earlier one, when the card has to be put up, and `None`
    /// when it stands already.
    fn place(&mut self, key: String) -> Option<String> {
        let repeat = self.repeats.entry(key.clone()).or_default();
        let key = if *repeat == 0 {
            key
        } else {
            format!("{key}#{repeat}")
        };
        *repeat += 1;
        self.keys.push(key.clone());
        (!(self.standing)(&key)).then_some(key)
    }
}

/// The key of a component's card: its type path, and whatever about the
/// component that the card draws once, when it is put up.
fn component_card_key(
    type_path: &str,
    is_overridden: bool,
    revert_through_prefab: bool,
    is_derived: bool,
) -> String {
    let mut key = type_path.to_string();
    if is_overridden {
        key.push_str(if revert_through_prefab {
            " overridden in prefab"
        } else {
            " overridden"
        });
    }
    if is_derived {
        key.push_str(" derived");
    }
    key
}

/// Key the cards a spawner just put under `inspector` without one, for
/// spawners that do not hand their cards back.
fn key_new_cards(commands: &mut Commands, inspector: Entity, key: String) {
    commands.queue(move |world: &mut World| {
        let Some(children) = world.get::<Children>(inspector) else {
            return;
        };
        let new: Vec<Entity> = children
            .iter()
            .filter(|&child| {
                world.get::<ComponentDisplay>(child).is_some()
                    && world.get::<InspectorCardKey>(child).is_none()
            })
            .collect();
        for card in new {
            world.entity_mut(card).insert(InspectorCardKey(key.clone()));
        }
    });
}

/// Take down the cards under `inspector` that `planned` does not name, and put
/// the rest in its order. Cards that stay are moved, not put up again.
fn arrange_cards(commands: &mut Commands, inspector: Entity, planned: Vec<String>) {
    commands.queue(move |world: &mut World| {
        let Some(children) = world.get::<Children>(inspector) else {
            return;
        };
        let position: HashMap<&str, usize> = planned
            .iter()
            .enumerate()
            .map(|(index, key)| (key.as_str(), index))
            .collect();
        let mut gone = Vec::new();
        let mut places = HashMap::new();
        for child in children.iter() {
            if world.get::<ComponentDisplay>(child).is_none() {
                continue;
            }
            match world
                .get::<InspectorCardKey>(child)
                .and_then(|key| position.get(key.0.as_str()))
            {
                Some(&place) => {
                    places.insert(child, place);
                }
                None => gone.push(child),
            }
        }
        for card in gone {
            if let Ok(card) = world.get_entity_mut(card) {
                card.despawn();
            }
        }
        let place = |child: &Entity| places.get(child).copied().unwrap_or(usize::MAX);
        let Some(mut children) = world.get_mut::<Children>(inspector) else {
            return;
        };
        if !children.is_sorted_by_key(place) {
            children.sort_by_key(place);
        }
    });
}

/// Whether the editor has a registration for the type the document names, or
/// for the type an authored enum variant belongs to.
fn known_to_the_editor(registry: &bevy::reflect::TypeRegistry, type_path: &str) -> bool {
    if registry.get_with_type_path(type_path).is_some() {
        return true;
    }
    enclosing_type(type_path).is_some_and(|base| registry.get_with_type_path(base).is_some())
}

/// The type an authored enum variant belongs to, whose schema is the one that
/// describes it.
fn enclosing_type(type_path: &str) -> Option<&str> {
    type_path.rsplit_once("::").map(|(base, _)| base)
}

/// The last segment of a type path, for a card naming a type nothing
/// describes.
fn short_type_name(type_path: &str) -> &str {
    type_path.rsplit("::").next().unwrap_or(type_path)
}

/// Scene-document components that live under `jackdaw_scene_types` and
/// belong in the inspector: `Brush` mounts the mesh card (`brush_display`,
/// and with it the whole Mesh tab), `Terrain` mounts the scatter /
/// quantization / channel / generation sections; `CanvasGuides` is where
/// a canvas guide's exact position is typed. `HiddenInGame` and
/// `NavmeshExclude` are marker tags an author adds by hand, so they need
/// a card even though they have no fields, as is `YSort`, which an author
/// should see on a sprite and be able to take off.
///
/// [`hidden_by_namespace`] exists to keep jackdaw's own bookkeeping
/// components out of the generic list. These are not bookkeeping -- they
/// are the scene data the user selected the entity to edit -- so culling
/// them takes their inspector surface with them.
const SCENE_TYPES_WITH_INSPECTOR_CARDS: [&str; 6] = [
    "jackdaw_scene_types::types::Brush",
    "jackdaw_scene_types::types::Terrain",
    "jackdaw_scene_types::CanvasGuides",
    jackdaw_scene_types::HIDDEN_IN_GAME_TYPE_PATH,
    jackdaw_scene_types::NAVMESH_EXCLUDE_TYPE_PATH,
    jackdaw_scene_types::Y_SORT_TYPE_PATH,
];

/// Whether a `jackdaw*` type is editor bookkeeping rather than something
/// the inspector should offer as a card.
///
/// A namespace cull with two kinds of hole punched in it: the crates whose
/// components are user-facing wholesale, modules' crates among them, and the
/// individual scene-data types in [`SCENE_TYPES_WITH_INSPECTOR_CARDS`].
fn hidden_by_namespace(full_path: &str) -> bool {
    full_path.starts_with("jackdaw")
        && !full_path.starts_with("jackdaw_jsn")
        && !full_path.starts_with("jackdaw_geometry")
        && !full_path.starts_with("jackdaw::reference_image")
        && !full_path.starts_with("jackdaw_animation")
        && !full_path.starts_with("jackdaw_bind::")
        && ModuleCatalog::builtin()
            .component_module(full_path)
            .is_none()
        && !SCENE_TYPES_WITH_INSPECTOR_CARDS.contains(&full_path)
        && !authored_widget_components().contains(&full_path)
}

/// Whether a reflected component gets a card.
///
/// Everything the namespace cull passes does, and inside it so does any type
/// that names an editor category without hiding itself: that is how scene
/// data such as `Wind` or `MaterialOverrides` says it is authored rather than
/// bookkeeping.
fn offered_as_card(full_path: &str, chrome: &TypeChrome) -> bool {
    !hidden_by_namespace(full_path) || (!chrome.category.is_empty() && !chrome.hidden)
}

/// The reflected components a widget's author edits, exempted from the
/// `jackdaw_widgets_runtime` cull that hides the crate's chrome bookkeeping.
fn authored_widget_components() -> [&'static str; 10] {
    use bevy::reflect::TypePath;
    use jackdaw_widgets_runtime as widgets;
    [
        widgets::TextValue::type_path(),
        widgets::ToggleSwitch::type_path(),
        widgets::Separator::type_path(),
        widgets::Spacer::type_path(),
        widgets::Progress::type_path(),
        widgets::ProgressFill::type_path(),
        widgets::Dropdown::type_path(),
        widgets::RadioOptions::type_path(),
        widgets::TabStrip::type_path(),
        widgets::NineSlice::type_path(),
    ]
}

/// Cards another card shows inside itself, as (absorbed, host) type paths.
/// An absorbed card is left out whenever its host's card is listed.
fn folded_cards() -> [(&'static str, &'static str); 1] {
    use bevy::reflect::TypePath;
    [(bevy::sprite::Anchor::type_path(), Sprite::type_path())]
}

struct ListedComponent {
    name: String,
    group: String,
    component_id: ComponentId,
    type_path: String,
    chrome: TypeChrome,
}

/// A file as the project names it: under the assets folder when it is there,
/// and as it stands when it is not.
fn named_in_assets(path: &std::path::Path) -> String {
    crate::project::open_project_assets_dir()
        .and_then(|assets| {
            path.strip_prefix(assets)
                .ok()
                .map(std::path::Path::to_path_buf)
        })
        .unwrap_or_else(|| path.to_path_buf())
        .display()
        .to_string()
}

/// What the card says about a prefab source nothing was inherited from.
fn source_note(path: &std::path::Path) -> String {
    format!(
        "Source {}: {}",
        crate::prefab::save_load::missing_source_reason(path),
        named_in_assets(path)
    )
}

/// The components of `archetype` that get a card of their own, before any is
/// folded into the card of another.
fn listed_components(
    archetype: &Archetype,
    components: &Components,
    registry: &bevy::reflect::TypeRegistry,
    type_metadata: &TypeMetadata,
    project_types: &crate::project_types::ProjectTypes,
    authored_type_paths: &HashSet<String>,
) -> Vec<ListedComponent> {
    archetype
        .iter_components()
        .filter_map(|component_id| {
            listed_component(
                component_id,
                components,
                registry,
                type_metadata,
                project_types,
                authored_type_paths,
            )
        })
        .collect()
}

/// The card [`listed_components`] gives `component_id`, or `None` when it
/// gets none.
fn listed_component(
    component_id: ComponentId,
    components: &Components,
    registry: &bevy::reflect::TypeRegistry,
    type_metadata: &TypeMetadata,
    project_types: &crate::project_types::ProjectTypes,
    authored_type_paths: &HashSet<String>,
) -> Option<ListedComponent> {
    let info = components.get_info(component_id)?;
    let type_id = info.type_id();

    // Try TypeRegistry first for proper names
    if let Some(type_id) = type_id
        && let Some(registration) = registry.get(type_id)
    {
        let table = registration.type_info().type_path_table();
        let full_path = table.path();
        let chrome = type_metadata.resolve(full_path, registry, project_types);
        if !offered_as_card(full_path, &chrome) {
            return None;
        }
        // AST filter: hide Bevy-internal components that
        // aren't tracked in the scene file. User-defined
        // components (anything outside the `bevy::*`,
        // `core::*`, `std::*`, and `jackdaw_*` namespaces)
        // are always shown so the inspector reflects the
        // actual ECS state. Without this exception, a user
        // component newly added via the picker would be
        // invisible if `AddComponent::execute`'s AST
        // serialization failed silently (e.g., a struct
        // field whose `Reflect` impl can't round-trip
        // through `TypedReflectSerializer`), leaving the
        // user wondering whether the click registered.
        let is_user_type = !full_path.starts_with("bevy")
            && !full_path.starts_with("core")
            && !full_path.starts_with("std")
            && (!full_path.starts_with("jackdaw")
                || ModuleCatalog::builtin()
                    .component_module(full_path)
                    .is_some());
        if !is_user_type
            && !crate::worn_material::material_component_paths().contains(&full_path)
            && !authored_type_paths.is_empty()
            && !jackdaw_bsn::type_paths_include(
                authored_type_paths.iter().map(String::as_str),
                full_path,
            )
        {
            return None;
        }
        let group = chrome.group(full_path);
        return Some(ListedComponent {
            name: table.short_path().to_string(),
            group,
            component_id,
            type_path: full_path.to_string(),
            chrome,
        });
    }

    // Unreflected components fall back to the `Components` name.
    let name = components.get_name(component_id)?;
    if hidden_by_namespace(&name) {
        return None;
    }
    Some(ListedComponent {
        name: name.shortname().to_string(),
        group: "Other".to_string(),
        component_id,
        type_path: name.to_string(),
        chrome: TypeChrome::default(),
    })
}

/// The components a panel's cards are built from, as compared between frames.
fn carded_components(
    archetype: &Archetype,
    inputs: &CardBuildInputs,
    authored_type_paths: &HashSet<String>,
) -> Vec<ComponentId> {
    let registry = inputs.type_registry.read();
    let mut carded: Vec<ComponentId> = listed_components(
        archetype,
        inputs.components,
        &registry,
        &inputs.asts.type_metadata,
        &inputs.asts.project_types,
        authored_type_paths,
    )
    .into_iter()
    .map(|listed| listed.component_id)
    .collect();
    carded.sort_unstable();
    carded
}

pub(crate) fn build_inspector_displays(
    commands: &mut Commands,
    components: &Components,
    type_registry: &Res<AppTypeRegistry>,
    source_entity: Entity,
    archetype: &Archetype,
    entity_ref: EntityRef,
    inspector_entity: Entity,
    selection_count: usize,
    names: &Query<&Name>,
    icon_font: &IconFont,
    editor_font: &EditorFont,
    _read_only: bool,
    materials: &Assets<StandardMaterial>,
    authored_type_paths: &HashSet<String>,
    scene_ast: Option<&jackdaw_bsn::SceneBsnAst>,
    prefab_cache: Option<&PrefabAstCache>,
    collapse_state: &super::InspectorCollapseState,
    project_types: &crate::project_types::ProjectTypes,
    type_metadata: &TypeMetadata,
) {
    build_inspector_cards(
        commands,
        components,
        type_registry,
        source_entity,
        archetype,
        entity_ref,
        inspector_entity,
        selection_count,
        names,
        icon_font,
        editor_font,
        materials,
        authored_type_paths,
        scene_ast,
        prefab_cache,
        collapse_state,
        project_types,
        type_metadata,
        &|_| false,
    );
}

/// Put up the cards for `source_entity` under `inspector_entity`, leaving out
/// those whose key `standing` names, and return the keys of every card in the
/// order the panel shows them.
fn build_inspector_cards(
    commands: &mut Commands,
    components: &Components,
    type_registry: &Res<AppTypeRegistry>,
    source_entity: Entity,
    archetype: &Archetype,
    entity_ref: EntityRef,
    inspector_entity: Entity,
    selection_count: usize,
    names: &Query<&Name>,
    icon_font: &IconFont,
    editor_font: &EditorFont,
    materials: &Assets<StandardMaterial>,
    authored_type_paths: &HashSet<String>,
    scene_ast: Option<&jackdaw_bsn::SceneBsnAst>,
    prefab_cache: Option<&PrefabAstCache>,
    collapse_state: &super::InspectorCollapseState,
    project_types: &crate::project_types::ProjectTypes,
    type_metadata: &TypeMetadata,
    standing: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    let mut plan = CardPlan::new(standing);

    // Show multi-selection header when multiple entities are selected
    if selection_count > 1
        && let Some(key) = plan.place(format!("{selection_count} selected"))
    {
        commands.spawn((
            ComponentDisplay,
            InspectorCardKey(key),
            Node {
                padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_SM)),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            BackgroundColor(tokens::SELECTED_BG),
            ChildOf(inspector_entity),
            children![(
                Text::new(format!(
                    "{selection_count} entities selected, edits apply to all"
                )),
                TextFont {
                    font: editor_font.0.clone().into(),
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            )],
        ));
    }

    let registry = type_registry.read();

    // Check for prefab baseline (override tracking)
    let baseline = entity_ref
        .get::<jackdaw_scene_types::PrefabBaseline>()
        .cloned();

    // Prefab-instance context: if this entity sits inside an IsA
    // subtree, override info comes from the prefab AST + cache and the
    // revert / right-click actions route to the new prefab operators.
    let prefab_ctx: Option<PrefabInstanceCtx> = scene_ast.and_then(|ast| {
        let cache = prefab_cache?;
        let node = ast.ast_for(source_entity)?;
        if !crate::prefab::overrides_bsn::is_inside_prefab_instance(ast, node) {
            return None;
        }
        let (path, prefab_entity_id) =
            crate::prefab::overrides_bsn::resolve_inheritance(ast, node)?;
        let instance_entity = ast
            .ancestor_with_component(node, "jackdaw::prefab::components::IsA")
            .and_then(|n| ast.ecs_for_ast(n))?;
        Some(PrefabInstanceCtx {
            instance_entity,
            has_cached_prefab: cache.get(&path).is_some(),
            prefab_path: path,
            prefab_entity_id,
        })
    });

    if let Some(ctx) = prefab_ctx.as_ref().filter(|ctx| !ctx.has_cached_prefab)
        && let Some(key) = plan.place(format!("source note {}", ctx.prefab_path.display()))
    {
        commands.spawn((
            ComponentDisplay,
            InspectorCardKey(key),
            Node {
                padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_SM)),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(inspector_entity),
            children![(
                Text::new(source_note(&ctx.prefab_path)),
                TextFont {
                    font: editor_font.0.clone().into(),
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_ERROR),
            )],
        ));
    }

    let mut comp_list = listed_components(
        archetype,
        components,
        &registry,
        type_metadata,
        project_types,
        authored_type_paths,
    );

    let listed: HashSet<String> = comp_list
        .iter()
        .map(|listed| listed.type_path.clone())
        .collect();
    comp_list.retain(|component| {
        !folded_cards()
            .iter()
            .any(|(absorbed, host)| component.type_path == *absorbed && listed.contains(*host))
    });

    // Sort: game EditorCategory groups, then Game, then engine groups,
    // then by group name, authored before derived, then alphabetical.
    let is_derived_path = |type_path: &str| -> bool {
        !authored_type_paths.is_empty()
            && !jackdaw_bsn::type_paths_include(
                authored_type_paths.iter().map(String::as_str),
                type_path,
            )
    };
    comp_list.sort_by(|a, b| {
        crate::type_metadata::group_order(&b.type_path, &b.chrome.category)
            .cmp(&crate::type_metadata::group_order(
                &a.type_path,
                &a.chrome.category,
            ))
            .then_with(|| a.group.cmp(&b.group))
            .then_with(|| is_derived_path(&a.type_path).cmp(&is_derived_path(&b.type_path)))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    for ListedComponent {
        name,
        component_id,
        type_path,
        chrome,
        ..
    } in &comp_list
    {
        let component_id = *component_id;

        // Detect override: compare current component value vs baseline
        let is_overridden_baseline = baseline.as_ref().is_some_and(|bl| {
            let type_id = components
                .get_info(component_id)
                .and_then(ComponentInfo::type_id);
            if let Some(type_id) = type_id
                && let Some(registration) = registry.get(type_id)
                && let Some(reflect_component) = registration.data::<ReflectComponent>()
                && let Some(component_ref) = reflect_component.reflect(entity_ref)
            {
                let type_path = registration.type_info().type_path_table().path();
                if let Some(baseline_val) = bl.components.get(type_path) {
                    let serializer = TypedReflectSerializer::new(component_ref, &registry);
                    if let Ok(current_val) = serde_json::to_value(&serializer) {
                        return current_val != *baseline_val;
                    }
                }
            }
            false
        });

        let is_overridden_prefab = prefab_ctx.as_ref().is_some_and(|ctx| {
            if !ctx.has_cached_prefab {
                return false;
            }
            let (Some(ast), Some(cache)) = (scene_ast, prefab_cache) else {
                return false;
            };
            let Some(node) = ast.ast_for(source_entity) else {
                return false;
            };
            let get_prefab = |p: &std::path::Path| cache.get(p);
            crate::prefab::overrides_bsn::field_is_overridden(
                ast,
                &get_prefab,
                node,
                type_path,
                None,
            )
        });

        let is_overridden = is_overridden_baseline || is_overridden_prefab;
        let is_derived = !authored_type_paths.is_empty()
            && !jackdaw_bsn::type_paths_include(
                authored_type_paths.iter().map(String::as_str),
                type_path.as_str(),
            );

        // Forward the prefab context whenever the entity sits inside a
        // prefab instance so the right-click menu can offer Revert /
        // Apply on every component. The revert ICON's routing still
        // checks `is_overridden_prefab` below so the legacy
        // `PrefabBaseline` path keeps using its existing operator
        // when both systems coexist.
        let spec_prefab_ctx = prefab_ctx.clone();
        let revert_through_prefab = is_overridden_prefab;

        // ModifierStack gets its own top-level cards (one per modifier entry)
        // rather than a single generic wrapper. Detect it here, before creating
        // the generic card, and emit per-modifier cards directly under the
        // inspector scroll body.
        if *type_path == *<jackdaw_geometry::ModifierStack as bevy::reflect::TypePath>::type_path()
        {
            let type_id = components
                .get_info(component_id)
                .and_then(ComponentInfo::type_id);
            if let Some(type_id) = type_id
                && let Some(registration) = registry.get(type_id)
                && let Some(reflect_component) = registration.data::<ReflectComponent>()
                && let Some(reflected) = reflect_component.reflect(entity_ref)
                && let Some(stack) = reflected.downcast_ref::<jackdaw_geometry::ModifierStack>()
                && let Some(key) = plan.place(
                    std::iter::once(type_path.as_str())
                        .chain(
                            stack
                                .modifiers
                                .iter()
                                .map(|entry| entry.modifier.kind_str()),
                        )
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            {
                modifier_display::spawn_modifier_display(
                    commands,
                    inspector_entity,
                    source_entity,
                    stack,
                    names,
                    type_registry,
                    &editor_font.0,
                    &icon_font.0,
                    collapse_state,
                );
                key_new_cards(commands, inspector_entity, key);
            }
            continue;
        }

        // The component a mesh wears its material on gets four dedicated
        // material cards (Preview, Surface, Textures, Settings) rather than a
        // single generic wrapper. Skip the generic card and inject the four
        // cards directly.
        if crate::worn_material::material_component_paths().contains(&type_path.as_str()) {
            if let Some(key) = plan.place("material".to_string()) {
                material_display::inject_material_cards(
                    commands,
                    source_entity,
                    inspector_entity,
                    &icon_font.0,
                    collapse_state,
                );
                key_new_cards(commands, inspector_entity, key);
            }
            continue;
        }

        let Some(key) = plan.place(component_card_key(
            type_path,
            is_overridden,
            revert_through_prefab,
            is_derived,
        )) else {
            continue;
        };
        let card = spawn_component_display(
            commands,
            ComponentDisplaySpec {
                name,
                type_path,
                entity: source_entity,
                is_overridden,
                is_derived,
                removable: true,
                prefab_ctx: spec_prefab_ctx,
                revert_through_prefab,
                icon_font: &icon_font.0,
                editor_font: &editor_font.0,
                collapse_state,
            },
        );
        commands.entity(card.section).insert(InspectorCardKey(key));
        super::type_metadata_pane::spawn_type_metadata_ui(
            commands,
            &card,
            type_path,
            chrome,
            type_metadata,
        );
        jackdaw_feathers::utils::attach_or_despawn(commands, inspector_entity, card.section);
        let body_entity = card.body;

        // Try Displayable first, then reflection, then fallback
        let type_id = components
            .get_info(component_id)
            .and_then(ComponentInfo::type_id);

        // A camera card leads with what the camera frames: a live
        // render-to-texture strip fed by the editor's mirror camera
        // (`camera_preview`), above the reflected fields.
        if type_id == Some(TypeId::of::<Camera3d>()) {
            crate::camera_preview::spawn_camera_preview_strip(commands, body_entity);
            crate::camera_preview::spawn_look_through_button(commands, body_entity, source_entity);
        }

        // A graph reference leads with the graphs the project holds, and a set
        // of clips with the way to a graph. Both are filled world-exclusive
        // after the flush, then fall through to the reflected fields.
        if type_id == Some(TypeId::of::<jackdaw_animation_runtime::AnimationGraphRef>()) {
            let body = body_entity;
            commands.queue(move |world: &mut World| {
                animation_graph_card::fill_graph_reference_picker(world, source_entity, body);
            });
        }
        if type_id == Some(TypeId::of::<jackdaw_animation_runtime::AnimationSet>()) {
            let body = body_entity;
            commands.queue(move |world: &mut World| {
                animation_graph_card::fill_animation_set_actions(world, source_entity, body);
            });
        }

        if let Some(type_id) = type_id
            && let Some(registration) = registry.get(type_id)
            && let Some(reflect_component) = registration.data::<ReflectComponent>()
            && let Some(reflected) = reflect_component.reflect(entity_ref)
        {
            // Priority 1: Displayable trait override
            if let Some(reflect_displayable) = registration.data::<ReflectDisplayable>()
                && let Some(displayable) = reflect_displayable.get(reflected)
            {
                let mut body_commands = commands.entity(body_entity);
                displayable.display(&mut body_commands, source_entity);
                continue;
            }

            // Priority 2: CustomProperties, specialized property editor
            if type_id == TypeId::of::<CustomProperties>() {
                if let Some(cp) = reflected.downcast_ref::<CustomProperties>() {
                    custom_props_display::spawn_custom_properties_display(
                        commands,
                        body_entity,
                        source_entity,
                        cp,
                        &editor_font.0,
                        &icon_font.0,
                    );
                }
                continue;
            }

            // Priority 3b: Brush, show face/vertex info
            if type_id == TypeId::of::<crate::brush::Brush>() {
                if let Some(brush) = reflected.downcast_ref::<crate::brush::Brush>() {
                    brush_display::spawn_brush_display(commands, body_entity, brush, materials);
                }
                continue;
            }

            // Priority 3c: Terrain, custom inspector sections
            if type_id == TypeId::of::<jackdaw_scene_types::Terrain>() {
                crate::terrain::inspector::spawn_terrain_inspector_container(commands, body_entity);
                continue;
            }

            // Priority 3d: Node, the structured layout card. Filled
            // world-exclusive after the flush, like the material cards.
            if type_id == TypeId::of::<Node>() {
                let body = body_entity;
                commands.queue(move |world: &mut World| {
                    node_card::fill_node_card_body(world, source_entity, body);
                });
                continue;
            }

            if let Some(canvas) = reflected.downcast_ref::<jackdaw_scene_types::PixelCanvas>() {
                crate::pixel_camera::spawn_preset_row(
                    commands,
                    body_entity,
                    source_entity,
                    canvas.resolution,
                );
            }

            if type_id == TypeId::of::<Sprite>() {
                let body = body_entity;
                commands.queue(move |world: &mut World| {
                    super::sprite_card::fill_sprite_card_body(world, source_entity, body);
                });
                continue;
            }

            // Priority 3e: Bindings, the declarative connections card.
            if type_id == TypeId::of::<jackdaw_bind::Bindings>() {
                let body = body_entity;
                commands.queue(move |world: &mut World| {
                    bindings_card::fill_bindings_card_body(world, source_entity, body);
                });
                continue;
            }

            // Priority 3: Generic reflection display
            let full_path = registration.type_info().type_path_table().path();
            reflect_fields::spawn_reflected_fields(
                commands,
                body_entity,
                reflected,
                0,
                String::new(),
                source_entity,
                full_path,
                names,
                type_registry,
                &editor_font.0,
                &icon_font.0,
            );
            continue;
        }

        // Fallback: no reflection data
        commands.spawn((
            LocalizedText::new("read-only"),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(body_entity),
        ));
    }

    // Project (schema-reported) components are not real ECS components in the
    // editor, so the archetype pass above never sees them. Render each one the
    // document authored on this entity from its extracted schema; values come
    // from the document and edits round-trip back through the same field
    // widgets (see `project_component_display`).
    if let Some(ast) = scene_ast
        && let Some(node) = ast.ast_for(source_entity)
    {
        let mut unavailable = Vec::new();
        for type_path in ast.component_type_paths(node) {
            let schema = project_types.component(&type_path);
            if schema.is_none()
                && (known_to_the_editor(&registry, &type_path)
                    || project_types.type_schema(&type_path).is_some())
            {
                continue;
            }
            if super::unavailable::is_unavailable(&registry, project_types, &type_path) {
                unavailable.push(type_path);
                continue;
            }
            let enclosing = enclosing_type(&type_path)
                .and_then(|base| project_types.type_schema(base))
                .filter(|_| schema.is_none());
            let chrome = type_metadata.resolve(&type_path, &registry, project_types);
            let name = match (schema, enclosing) {
                (Some(schema), _) => schema.short_name.clone(),
                (None, Some(enclosing)) => {
                    format!("{}::{}", enclosing.short_name, short_type_name(&type_path))
                }
                (None, None) => short_type_name(&type_path).to_string(),
            };
            let Some(key) = plan.place(format!(
                "{type_path} {}",
                if schema.is_some() {
                    "project"
                } else {
                    "document"
                }
            )) else {
                continue;
            };
            let card = spawn_component_display(
                commands,
                ComponentDisplaySpec {
                    name: &name,
                    type_path: &type_path,
                    entity: source_entity,
                    is_overridden: false,
                    is_derived: false,
                    removable: true,
                    prefab_ctx: None,
                    revert_through_prefab: false,
                    icon_font: &icon_font.0,
                    editor_font: &editor_font.0,
                    collapse_state,
                },
            );
            commands.entity(card.section).insert(InspectorCardKey(key));
            super::type_metadata_pane::spawn_type_metadata_ui(
                commands,
                &card,
                &type_path,
                &chrome,
                type_metadata,
            );
            jackdaw_feathers::utils::attach_or_despawn(commands, inspector_entity, card.section);
            match schema {
                Some(_) => {
                    commands.queue(move |world: &mut World| {
                        super::project_component_display::fill_project_component_fields(
                            world,
                            card.body,
                            source_entity,
                            node,
                            &type_path.clone(),
                        );
                    });
                }
                None => super::project_component_display::spawn_document_component_fields(
                    commands, card.body, ast, node, &type_path,
                ),
            }
        }
        if !unavailable.is_empty()
            && let Some(key) = plan.place(format!("not loaded {}", unavailable.join(" ")))
        {
            spawn_unavailable_group(
                commands,
                inspector_entity,
                source_entity,
                ast,
                node,
                &unavailable,
                &icon_font.0,
                &editor_font.0,
                collapse_state,
            );
            key_new_cards(commands, inspector_entity, key);
        }
    }

    // Add Component button is in the static layout header (layout.rs entity_inspector)
    // so we don't spawn a dynamic one here.

    // If the selected entity is a brush, inject the four material cards into the
    // Material tab. The brush entity itself carries no MeshMaterial3d; its face
    // data carries the handles. Shells are spawned synchronously (same flush as
    // every other card) so the "material" category is present on the rebuild
    // frame before `resolve_active_on_rebuild` runs. Body fills are deferred.
    if entity_ref.contains::<crate::brush::Brush>()
        && let Some(key) = plan.place("material".to_string())
    {
        material_display::inject_material_cards(
            commands,
            source_entity,
            inspector_entity,
            &icon_font.0,
            collapse_state,
        );
        key_new_cards(commands, inspector_entity, key);
    }

    if selection_count == 1
        && entity_ref.contains::<jackdaw_scene_types::model_import::ModelLevels>()
        && let Some(key) = plan.place("lod placement".to_string())
    {
        commands.queue(move |world: &mut World| {
            crate::lod_bar::spawn_placement_card(world, inspector_entity, source_entity);
        });
        key_new_cards(commands, inspector_entity, key);
    }

    plan.keys
}

/// The components on `node` the editor cannot load, as one group of read-only
/// cards: each names its type, says where it comes from, and shows its fields
/// as the document spells them. Removing one is allowed; editing is not.
#[expect(
    clippy::too_many_arguments,
    reason = "the card spawner's inputs, passed through"
)]
fn spawn_unavailable_group(
    commands: &mut Commands,
    inspector_entity: Entity,
    source_entity: Entity,
    ast: &jackdaw_bsn::SceneBsnAst,
    node: Entity,
    type_paths: &[String],
    icon_font: &Handle<Font>,
    editor_font: &Handle<Font>,
    collapse_state: &super::InspectorCollapseState,
) {
    if type_paths.is_empty() {
        return;
    }
    let group = commands
        .spawn((
            // Marked as a display so the next rebuild clears it with the cards.
            ComponentDisplay,
            InspectorGroupSection,
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                row_gap: Val::Px(tokens::SPACING_SM),
                ..Default::default()
            },
        ))
        .id();
    commands.spawn((
        Text::new(format!("Not loaded ({})", type_paths.len())),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            weight: FontWeight::MEDIUM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(group),
    ));
    for type_path in type_paths {
        let card = spawn_component_display(
            commands,
            ComponentDisplaySpec {
                name: super::unavailable::card_name(type_path),
                type_path,
                entity: source_entity,
                is_overridden: false,
                is_derived: false,
                removable: true,
                prefab_ctx: None,
                revert_through_prefab: false,
                icon_font,
                editor_font,
                collapse_state,
            },
        );
        commands.entity(card.section).insert(ChildOf(group));
        commands.spawn((
            Node {
                padding: UiRect::axes(Val::Px(tokens::SPACING_XS), Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
                flex_shrink: 0.0,
                ..Default::default()
            },
            BackgroundColor(tokens::AXIS_LABEL_BG),
            ChildOf(card.title_row),
            children![(
                Text::new("not loaded"),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_MUTED_COLOR.into()),
            )],
        ));
        super::project_component_display::spawn_unavailable_component_fields(
            commands, card.body, ast, node, type_path,
        );
    }
    jackdaw_feathers::utils::attach_or_despawn(commands, inspector_entity, group);
}

/// Despawn inspector card and picker children as one queued world step so
/// lazy combobox/button setup cannot interleave and orphan UI. The children are
/// read when the step runs, so a card another queued step put up since goes too.
fn despawn_inspector_display_children(commands: &mut Commands, inspector: Entity) {
    commands.queue(move |world: &mut World| {
        let Some(children) = world.get::<Children>(inspector) else {
            return;
        };
        let old_children: Vec<Entity> = children
            .iter()
            .filter(|&child| {
                world.get::<ComponentDisplay>(child).is_some()
                    || world.get::<ComponentPicker>(child).is_some()
            })
            .collect();
        for child in old_children {
            if let Ok(ec) = world.get_entity_mut(child) {
                ec.despawn();
            }
        }
    });
}

/// The disclosure link and its handler live with the shared card widget;
/// component cards and material cards both use it.
pub(crate) use jackdaw_feathers::panel_card::DisclosureSection;

/// Inputs to [`spawn_component_display`]. Bundled into a single
/// struct so the call site is readable as a struct literal instead of
/// a long positional argument list.
pub(crate) struct ComponentDisplaySpec<'a> {
    pub name: &'a str,
    pub type_path: &'a str,
    pub entity: Entity,
    pub is_overridden: bool,
    /// True when the component is on the live entity but has no authored
    /// document patch (`#[require]` companions, runtime inserts, etc.).
    pub is_derived: bool,
    /// Show the header X that dispatches `component.remove`.
    pub removable: bool,
    /// When `Some`, the entity sits inside a prefab instance. Drives
    /// the right-click menu for every component on the entity.
    pub prefab_ctx: Option<PrefabInstanceCtx>,
    /// When true, the revert icon (if shown) routes through the new
    /// prefab operators (`prefab::operators::revert_component`) rather
    /// than the legacy `ComponentRevertBaselineOp` path. False forces
    /// the legacy path even if `prefab_ctx` is present, which preserves
    /// pre-existing baseline overrides.
    pub revert_through_prefab: bool,
    pub icon_font: &'a Handle<Font>,
    pub editor_font: &'a Handle<Font>,
    /// Per-session collapsed-state map; used to restore the card's
    /// open/closed state across inspector rebuilds.
    pub collapse_state: &'a super::InspectorCollapseState,
}

/// The width a card's title row takes before its header buttons wrap below
/// it: the disclosure and icon, and about eight characters of the name.
const TITLE_ROW_BASIS: f32 = 96.0;

/// Entities spawned by [`spawn_component_display`]. `section` is the card
/// root; `body` is where field widgets go; `type_settings` sits between
/// header and body so type-settings chrome can span the header width.
pub(crate) struct ComponentDisplayCard {
    pub section: Entity,
    pub body: Entity,
    pub header: Entity,
    /// The row holding the disclosure, icon and name, for chips beside the name.
    pub title_row: Entity,
    pub type_settings: Entity,
}

pub(crate) fn spawn_component_display(
    commands: &mut Commands,
    spec: ComponentDisplaySpec<'_>,
) -> ComponentDisplayCard {
    let ComponentDisplaySpec {
        name,
        type_path,
        entity,
        is_overridden,
        is_derived,
        removable,
        prefab_ctx,
        revert_through_prefab,
        icon_font,
        editor_font,
        collapse_state,
    } = spec;
    let font = icon_font.clone();
    let body_font = editor_font.clone();

    let collapsed = collapse_state.collapsed(name);
    let body_display = if collapsed {
        Display::None
    } else {
        Display::Flex
    };

    // Card frame: a feathers pane holding a header and a body. The card's layout is
    // patched onto the pane's own `Node` rather than replacing it: `pane` carries the
    // stretch alignment that makes its header and body span the card, and `pane_body`
    // carries the padding, row gap and rounded corners that draw the frame.
    let section_entity = commands
        .spawn_scene((
            pane(),
            bsn! {
                Node {
                    flex_direction: FlexDirection::Column,
                    width: percent(100),
                }
            },
        ))
        .insert((
            ComponentDisplay,
            ComponentName(name.to_string()),
            ComponentDisplayTypePath(type_path.to_string()),
            CollapsibleSection { collapsed },
        ))
        .id();

    // The toggle area grows to push the revert / remove buttons right. When the
    // panel is too narrow for both, the buttons wrap to a row of their own
    // rather than leave the title no room.
    let header = commands
        .spawn_scene((
            pane_header(),
            bsn! {
                Node {
                    flex_wrap: FlexWrap::Wrap,
                    justify_content: JustifyContent::FlexEnd,
                }
            },
        ))
        .insert((CollapsibleHeader, ChildOf(section_entity)))
        .id();

    let type_settings = commands
        .spawn((
            CollapsibleBody,
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                display: body_display,
                flex_shrink: 0.0,
                ..Default::default()
            },
            ChildOf(section_entity),
        ))
        .id();

    let body_entity = commands
        .spawn_scene((
            pane_body(),
            bsn! {
                Node {
                    flex_direction: FlexDirection::Column,
                    width: percent(100),
                    display: {body_display},
                }
            },
        ))
        .insert((
            ComponentDisplayBody,
            CollapsibleBody,
            ChildOf(section_entity),
        ))
        .id();

    // Toggle area (chevron + icon + title) -- click to collapse/expand.
    // The hover-tooltip source sits on this row so the popover
    // surface matches the click target; the auto-attach observer in
    // `component_tooltip.rs` resolves the reflected type and inserts
    // a `Tooltip` with the short name, optional `ReflectEditorMeta`
    // description, and full type path.
    let toggle_area = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_SM),
                flex_grow: 1.0,
                // The room the title row asks for before the buttons wrap.
                flex_basis: Val::Px(TITLE_ROW_BASIS),
                // A generic type path is one unbreakable word wider than the
                // sidebar; the full path is on the hover tooltip.
                min_width: Val::Px(0.0),
                overflow: Overflow::clip_x(),
                ..Default::default()
            },
            Hovered::default(),
            ReflectedTypeTooltip::new(type_path.to_string()),
            ChildOf(header),
        ))
        .id();

    // Disclosure toggle. Its checked state maps to expanded (`!collapsed`),
    // and it renders the rotating chevron. Clicking it emits
    // `ValueChange<bool>`, handled by `on_disclosure_change`, which drives the
    // section's collapsed flag and the body visibility.
    let mut disclosure = commands.spawn_scene(bsn! { @FeathersDisclosureToggle });
    disclosure.insert((ChildOf(toggle_area), DisclosureSection(section_entity)));
    if !collapsed {
        disclosure.insert(Checked);
    }
    let disclosure_entity = disclosure.id();

    // Component icon (matching Figma: lucide/move-3d style icon)
    commands.spawn((
        Text::new(String::from(Icon::Move3d.unicode())),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(toggle_area),
    ));

    // Component name (orange if overridden; muted when derived).
    let name_color = if is_overridden {
        default_style::INSPECTOR_OVERRIDE
    } else if is_derived {
        tokens::TEXT_MUTED_COLOR.into()
    } else {
        tokens::TEXT_DISPLAY_COLOR.into()
    };
    commands.spawn((
        Text::new(name.to_string()),
        TextFont {
            font: body_font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            weight: FontWeight::MEDIUM,
            ..Default::default()
        },
        // Cut, never wrapped: a second line would move the card's controls
        // down the panel.
        TextLayout {
            linebreak: bevy::text::LineBreak::NoWrap,
            ..Default::default()
        },
        TextColor(name_color),
        Node {
            min_width: Val::Px(0.0),
            flex_shrink: 1.0,
            ..Default::default()
        },
        ChildOf(toggle_area),
    ));

    // Clicking anywhere on the header row toggles the disclosure, which then
    // emits `ValueChange<bool>` and flows through `on_disclosure_change`.
    commands
        .entity(toggle_area)
        .observe(move |_: On<PointerClick>, mut commands: Commands| {
            commands.trigger(ToggleChecked {
                entity: disclosure_entity,
            });
        });

    if is_overridden {
        let revert_type_path = type_path.to_string();
        let entity_param = entity;

        // Revert button (only shown for overridden prefab components).
        // Two code paths share the visual: the legacy
        // `PrefabBaseline` system dispatches through
        // `ComponentRevertBaselineOp` (and uses `ButtonOperatorCall`
        // for the rich tooltip popover); the new prefab system calls
        // `prefab::operators::revert_component` directly with the
        // entity's AST key, so it skips the tooltip wiring.
        let revert_through_new_prefab = revert_through_prefab && prefab_ctx.is_some();
        if revert_through_new_prefab {
            let prefab_type_path = revert_type_path.clone();
            commands.spawn((
                Text::new(String::from(Icon::RotateCcw.unicode())),
                TextFont {
                    font: font.clone().into(),
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(default_style::INSPECTOR_OVERRIDE),
                Hovered::default(),
                ChildOf(header),
                bevy::ui_widgets::observe(move |_: On<PointerClick>, mut commands: Commands| {
                    let revert_path = prefab_type_path.clone();
                    commands
                        .operator("prefab.revert_component")
                        .settings(CallOperatorSettings {
                            creates_history_entry: true,
                            ..default()
                        })
                        .param("entity", entity_param)
                        .param("type_path", revert_path)
                        .call();
                    commands.queue(move |world: &mut World| {
                        if let Ok(mut ec) = world.get_entity_mut(entity_param) {
                            ec.insert(InspectorDirty);
                        }
                    });
                }),
            ));
        } else {
            let bo_call = ButtonOperatorCall::new(super::ops::ComponentRevertBaselineOp::ID)
                .with_param("entity", entity_param)
                .with_param("type_path", revert_type_path.clone());
            commands.spawn((
                Text::new(String::from(Icon::RotateCcw.unicode())),
                TextFont {
                    font: font.clone().into(),
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(default_style::INSPECTOR_OVERRIDE),
                Hovered::default(),
                bo_call,
                ChildOf(header),
                bevy::ui_widgets::observe(move |_: On<PointerClick>, mut commands: Commands| {
                    commands
                        .operator(super::ops::ComponentRevertBaselineOp::ID)
                        .param("entity", entity_param)
                        .param("type_path", revert_type_path.clone())
                        .call();
                }),
            ));
        }
    }

    // Remove component button (X icon). See revert button for the
    // tooltip-data + manual-dispatch pattern.
    if removable && !is_derived {
        let remove_path = type_path.to_string();
        let entity_param = entity;
        let remove_call = ButtonOperatorCall::new(super::ops::ComponentRemoveOp::ID)
            .with_param("entity", entity_param)
            .with_param("type_path", remove_path.clone());
        commands
            .spawn_scene(bsn! {
                @FeathersToolButton {
                    @caption: bsn! { @icon_scene(Icon::X.unicode(), tokens::TEXT_SIZE_SM_PX) },
                    @variant: {ButtonVariant::Plain}
                }
            })
            .insert((Hovered::default(), remove_call, ChildOf(header)));
    }

    // Right-click context menu on prefab-instance component headers.
    // Wires the "Revert Component" / "Apply Component to Prefab Source"
    // actions; both route through `prefab_menu::on_prefab_menu_action`,
    // which reads the captured target context from
    // `prefab_menu::PrefabMenuTarget`.
    if let Some(menu_ctx) = prefab_ctx.clone() {
        let menu_type_path = type_path.to_string();
        commands.entity(header).observe(
            move |click: On<PointerClick>,
                  mut commands: Commands,
                  windows: Query<&Window>,
                  mut state: ResMut<jackdaw_widgets::context_menu::ContextMenuState>,
                  mut target: ResMut<super::prefab_menu::PrefabMenuTarget>| {
                if click.event().button != PointerButton::Secondary {
                    return;
                }
                let cursor_pos = windows
                    .single()
                    .ok()
                    .and_then(bevy::prelude::Window::cursor_position)
                    .unwrap_or_default();
                if let Some(existing) = state.menu_entity.take()
                    && let Ok(mut ec) = commands.get_entity(existing)
                {
                    ec.despawn();
                }
                target.entity = Some(entity);
                target.instance_entity = Some(menu_ctx.instance_entity);
                target.prefab_entity_id = Some(menu_ctx.prefab_entity_id);
                target.prefab_path = Some(menu_ctx.prefab_path.clone());
                target.type_path = Some(menu_type_path.clone());
                target.field_path = None;
                let items: [(&str, &str); 3] = [
                    (super::prefab_menu::REVERT_COMPONENT, "Revert Component"),
                    (
                        super::prefab_menu::APPLY_TO_SOURCE,
                        "Apply Component to Prefab Source",
                    ),
                    (
                        super::prefab_menu::BULK_APPLY,
                        "Apply to All Instances in Scene",
                    ),
                ];
                let menu = jackdaw_feathers::context_menu::spawn_context_menu(
                    &mut commands,
                    cursor_pos,
                    None,
                    &items,
                );
                state.menu_entity = Some(menu);
            },
        );
    }

    ComponentDisplayCard {
        section: section_entity,
        body: body_entity,
        header,
        title_row: toggle_area,
        type_settings,
    }
}

/// Filter inspector component cards based on both the active category and the
/// search input. A card is visible only when its category matches the active
/// category AND its short name passes the search text predicate (either the
/// search field is empty or the name contains the filter string).
///
/// A definition card is the panel's whole contents rather than one component of
/// an entity, so no category hides it; the search still does.
///
/// The system re-runs whenever the search text changes OR the active category
/// changes. Group-section visibility follows: a group hides when all of its
/// cards are hidden.
pub(crate) fn filter_inspector_components(
    search_query: Query<&TextEditValue, With<InspectorSearch>>,
    active: Res<ActiveInspectorCategory>,
    registry: Res<jackdaw_api_internal::inspector::InspectorRegistry>,
    components: Query<
        (
            Entity,
            &ComponentName,
            &ComponentDisplayTypePath,
            Has<super::definition_card::DefinitionCard>,
        ),
        With<ComponentDisplay>,
    >,
    groups: Query<(Entity, &Children), With<InspectorGroupSection>>,
    mut node_query: Query<&mut Node>,
    changed_search: Query<(), (With<InspectorSearch>, Changed<TextEditValue>)>,
) {
    // Re-run only when the search text or the active category changed.
    // Running every frame is cheap but this avoids unnecessary Node mutations.
    if changed_search.is_empty() && !active.is_changed() {
        return;
    }

    let filter = search_query
        .single()
        .map(|v| v.0.trim().to_lowercase())
        .unwrap_or_default();

    let active_cat = active.0.as_ref();

    // Track which component entities are visible.
    let mut visible_components: HashSet<Entity> = HashSet::new();

    for (entity, comp_name, type_path, is_definition) in &components {
        let category_ok = is_definition || registry.category_for(&type_path.0) == active_cat;
        let search_ok = filter.is_empty() || comp_name.0.to_lowercase().contains(&filter);
        let visible = category_ok && search_ok;

        if let Ok(mut node) = node_query.get_mut(entity) {
            node.display = if visible {
                Display::Flex
            } else {
                Display::None
            };
        }

        if visible {
            visible_components.insert(entity);
        }
    }

    // Hide group sections where all children are hidden.
    for (group_entity, children) in &groups {
        let has_visible_child = children
            .iter()
            .any(|child| visible_components.contains(&child));

        if let Ok(mut node) = node_query.get_mut(group_entity) {
            node.display = if has_visible_child {
                Display::Flex
            } else {
                Display::None
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ComponentDisplaySpec, hidden_by_namespace, source_note, spawn_component_display};
    use bevy::feathers::controls::FeathersToolButton;
    use bevy::prelude::*;
    use jackdaw_api_internal::operator::Operator;
    use jackdaw_feathers::button::ButtonOperatorCall;

    #[test]
    fn a_source_that_is_on_disk_but_did_not_read_says_so_rather_than_reading_as_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let unreadable = dir.path().join("broken.bsn");
        std::fs::write(&unreadable, "this is not a document").expect("write");

        assert!(source_note(&unreadable).starts_with("Source could not be read"));
        assert!(
            source_note(&dir.path().join("absent.bsn")).starts_with("Source is not in the project"),
        );
    }

    /// The card's remove control is a tool button carrying the remove operator.
    #[test]
    fn the_remove_control_is_a_feathers_tool_button() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ))
        .init_asset::<Image>()
        .init_asset::<Font>();

        let spawn = app
            .world_mut()
            .register_system(move |mut commands: Commands| {
                let entity = commands.spawn_empty().id();
                let font: Handle<Font> = Handle::default();
                let collapse_state = crate::inspector::InspectorCollapseState::default();
                spawn_component_display(
                    &mut commands,
                    ComponentDisplaySpec {
                        name: "Transform",
                        type_path: "bevy_transform::components::transform::Transform",
                        entity,
                        is_overridden: false,
                        is_derived: false,
                        removable: true,
                        prefab_ctx: None,
                        revert_through_prefab: false,
                        icon_font: &font,
                        editor_font: &font,
                        collapse_state: &collapse_state,
                    },
                );
            });
        app.world_mut().run_system(spawn).expect("system runs");
        app.world_mut().flush();

        let mut removes = app
            .world_mut()
            .query_filtered::<&ButtonOperatorCall, With<FeathersToolButton>>();
        assert!(
            removes
                .iter(app.world())
                .any(|call| call.id == crate::inspector::ops::ComponentRemoveOp::ID),
            "the remove control is a feathers tool button carrying the remove operator",
        );
    }

    /// A card keyed only by type path still offers remove.
    #[test]
    fn a_project_component_card_has_the_remove_control() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ))
        .init_asset::<Image>()
        .init_asset::<Font>();

        let spawn = app
            .world_mut()
            .register_system(move |mut commands: Commands| {
                let entity = commands.spawn_empty().id();
                let font: Handle<Font> = Handle::default();
                let collapse_state = crate::inspector::InspectorCollapseState::default();
                spawn_component_display(
                    &mut commands,
                    ComponentDisplaySpec {
                        name: "PlayerSpawnPoint",
                        type_path: "web_shooter::player::PlayerSpawnPoint",
                        entity,
                        is_overridden: false,
                        is_derived: false,
                        removable: true,
                        prefab_ctx: None,
                        revert_through_prefab: false,
                        icon_font: &font,
                        editor_font: &font,
                        collapse_state: &collapse_state,
                    },
                );
            });
        app.world_mut().run_system(spawn).expect("system runs");
        app.world_mut().flush();

        let mut removes = app
            .world_mut()
            .query_filtered::<&ButtonOperatorCall, With<FeathersToolButton>>();
        assert!(
            removes
                .iter(app.world())
                .any(|call| call.id == crate::inspector::ops::ComponentRemoveOp::ID),
            "a project component card has the remove control",
        );
    }

    /// The card's layout is patched onto the feathers pane rather than replacing its
    /// `Node`, which would drop the padding, row gap and rounded corners `pane_body`
    /// spawns with.
    #[test]
    fn the_card_layout_is_added_to_the_panes_own_node_not_swapped_for_it() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ));

        let spawn = app.world_mut().register_system(|mut commands: Commands| {
            commands.spawn_scene((
                bevy::feathers::containers::pane_body(),
                bsn! {
                    Node {
                        flex_direction: FlexDirection::Column,
                        width: percent(100),
                        display: {Display::None},
                    }
                },
            ));
        });
        app.world_mut().run_system(spawn).expect("system runs");
        app.world_mut().flush();

        let mut nodes = app.world_mut().query::<&Node>();
        let node = nodes.iter(app.world()).next().expect("the body spawned");
        assert_eq!(node.width, Val::Percent(100.0), "the card's own width");
        assert_eq!(
            node.display,
            Display::None,
            "the card's own collapsed state"
        );
        assert_eq!(
            node.flex_direction,
            FlexDirection::Column,
            "the card's own direction",
        );
        assert_ne!(node.padding, UiRect::ZERO, "the pane's frame padding");
        assert_ne!(node.row_gap, Val::ZERO, "the pane's row gap");
    }

    #[test]
    fn the_scene_data_components_with_their_own_cards_survive_the_namespace_cull() {
        // Each mounts a dedicated inspector surface; culling either one
        // takes a whole tool panel out of the editor.
        assert!(!hidden_by_namespace("jackdaw_scene_types::types::Brush"));
        assert!(!hidden_by_namespace("jackdaw_scene_types::types::Terrain"));
        assert!(
            !hidden_by_namespace(std::any::type_name::<jackdaw_scene_types::CanvasGuides>()),
            "the UI root's guides show as a card, so their positions are typeable",
        );
        assert!(
            !hidden_by_namespace(jackdaw_scene_types::HIDDEN_IN_GAME_TYPE_PATH),
            "HiddenInGame is a tag an author adds by hand, so it needs a card",
        );
        assert!(
            !hidden_by_namespace(jackdaw_scene_types::NAVMESH_EXCLUDE_TYPE_PATH),
            "NavmeshExclude is a tag an author adds by hand, so it needs a card",
        );
    }

    #[test]
    fn scene_data_naming_an_editor_category_gets_a_card_and_bookkeeping_does_not() {
        let chrome = |category: &str, hidden: bool| super::TypeChrome {
            category: category.to_string(),
            hidden,
            ..Default::default()
        };
        assert!(super::offered_as_card(
            "jackdaw_scene_types::types::Wind",
            &chrome("Environment", false)
        ));
        assert!(super::offered_as_card(
            "jackdaw_scene_types::types::MaterialOverrides",
            &chrome("Rendering", false)
        ));
        assert!(!super::offered_as_card(
            "jackdaw_scene_types::types::ScatterInstance",
            &chrome("Terrain", true)
        ));
        assert!(!super::offered_as_card(
            "jackdaw_scene_types::node_id::SceneNodeId",
            &chrome("", true)
        ));
        assert!(super::offered_as_card(
            "jackdaw_scene_types::types::Terrain",
            &chrome("Terrain", true)
        ));
    }

    #[test]
    fn editor_bookkeeping_stays_out_of_the_generic_list() {
        assert!(hidden_by_namespace(
            "jackdaw_scene_types::node_id::SceneNodeId"
        ));
        // The wholesale-allowed crates are untouched by the cull, modules'
        // crates among them.
        assert!(!hidden_by_namespace("jackdaw_multiplayer::Replication"));
        assert!(!hidden_by_namespace(
            "jackdaw_geometry::modifiers::ModifierStack"
        ));
        // Non-jackdaw types never reach this rule's business end.
        assert!(!hidden_by_namespace(
            "bevy_transform::components::transform::Transform"
        ));
    }

    /// Bindings are authored data, not editor bookkeeping.
    #[test]
    fn binding_types_survive_the_namespace_cull() {
        assert!(!hidden_by_namespace("jackdaw_bind::types::Bindings"));
        assert!(!hidden_by_namespace("jackdaw_bind::types::BindContext"));
        // The exemption is that crate, not every crate sharing its prefix.
        assert!(hidden_by_namespace("jackdaw_bindings::Foo"));
    }

    /// `AuthoredWidget` is not `Reflect`, so it reaches the inspector through
    /// the `Components`-name fallback and is culled by its namespace.
    #[test]
    fn the_runtime_widget_marker_stays_out_of_the_generic_list() {
        assert!(hidden_by_namespace(std::any::type_name::<
            jackdaw_widgets_runtime::AuthoredWidget,
        >()));
    }

    /// What a widget's author edits is not bookkeeping, so it survives the cull.
    #[test]
    fn the_authored_widget_components_survive_the_namespace_cull() {
        for path in super::authored_widget_components() {
            assert!(!hidden_by_namespace(path), "{path} is hidden");
        }
    }
}
