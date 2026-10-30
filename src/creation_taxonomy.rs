//! The creation vocabulary read by the Add menu, the Add Entity picker and the
//! command palette. Groups nest one level deep.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::menu_bar::{SECTION_ACTION_PREFIX, SEPARATOR_ACTION, submenu_row};

#[cfg(feature = "camera_rig")]
use crate::entity_ops::EntityAddCameraRigOp;
use crate::entity_ops::{
    EntityAddAnimationPlayerOp, EntityAddAudioSourceOp, EntityAddCameraOp, EntityAddConeOp,
    EntityAddCubeOp, EntityAddCylinderOp, EntityAddDirectionalLightOp, EntityAddEmptyOp,
    EntityAddFogVolumeOp, EntityAddPlaneOp, EntityAddPointLightOp, EntityAddPrefabOp,
    EntityAddPyramidOp, EntityAddReflectionProbeOp, EntityAddSphereOp, EntityAddSpotLightOp,
    EntityAddTerrainOp, EntityAddWedgeOp,
};
#[cfg(feature = "multiplayer")]
use crate::entity_ops::{EntityAddNetworkRoomOp, EntityAddSpawnPointOp, EntityAddZoneTransitionOp};
use crate::sprite_ops::SpriteAddOp;

/// Action prefix of an entry that creates a registered UI widget.
pub const WIDGET_ACTION_PREFIX: &str = "widget:";

/// Action prefix of an entry that creates an asset file; the kind follows it.
pub const ASSET_ACTION_PREFIX: &str = "asset:";

/// The group holding entity kinds that belong to no other group.
pub const GENERAL_GROUP: &str = "general";

/// The group the registered UI widgets sit under; its children are the widget
/// categories.
pub const UI_GROUP: &str = "ui";

/// Label of [`GENERAL_GROUP`].
pub const GENERAL_SECTION: &str = "General";

/// Joins a parent group's label to a child's.
pub const QUALIFIED_LABEL_SEPARATOR: &str = ": ";

/// Widget categories in the order their groups appear; unknown categories sort
/// after these.
const UI_CATEGORY_ORDER: [&str; 3] = ["Layout", "Display", "Controls"];

/// Sort order of the UI group and of the first of its children.
const UI_GROUP_ORDER: i32 = -40;

/// Sort order of a widget group whose category is not in `UI_CATEGORY_ORDER`.
const UI_UNKNOWN_CATEGORY_ORDER: i32 = UI_GROUP_ORDER - UI_CATEGORY_ORDER.len() as i32;

/// The group holding the 2D world entity kinds. Always present; its entries are
/// unavailable until a 2D world scene is open.
pub const TWO_D_GROUP: &str = "2d";

/// The group holding the 3D entity kinds; its children are the 3D categories.
pub const THREE_D_GROUP: &str = "3d";

/// Sort order of [`TWO_D_GROUP`], after General.
const TWO_D_GROUP_ORDER: i32 = -10;

/// Sort order of [`THREE_D_GROUP`], after the 2D group.
const THREE_D_GROUP_ORDER: i32 = -20;

/// Sort order of the multiplayer group, after the 3D group and before UI.
#[cfg(feature = "multiplayer")]
const MULTIPLAYER_GROUP_ORDER: i32 = -30;

/// The group holding one entry per asset kind.
pub const ASSETS_GROUP: &str = "assets";

/// Label of [`ASSETS_GROUP`].
pub const ASSETS_SECTION: &str = "Assets";

/// Sort order of the assets group, after the entity kinds.
const ASSETS_GROUP_ORDER: i32 = UI_UNKNOWN_CATEGORY_ORDER - 1;

/// Sort order of the per-extension groups, last in the menu.
const EXTENSION_GROUP_ORDER: i32 = ASSETS_GROUP_ORDER - 1;

/// Heading for entries whose owning extension cannot be named.
pub const EXTENSIONS_SECTION: &str = "Extensions";

/// A group with no more entries than this renders inline in the Add menu rather
/// than as a row that expands, so a single entry appears among the groups.
const INLINE_GROUP_MAX: usize = 1;

/// Who a group came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CreationSource {
    /// Part of the editor's own vocabulary.
    #[default]
    BuiltIn,
    /// Contributed by an extension.
    Extension,
}

/// One group of things that can be created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreationGroup {
    /// Stable key, referenced by [`CreationEntry::group`] and
    /// [`CreationGroup::parent`].
    pub id: String,
    /// The name shown in a menu or a picker heading.
    pub label: String,
    /// Where the group sits; greater comes first.
    pub order: i32,
    /// The group this one belongs to, if any.
    pub parent: Option<String>,
    /// Who contributed the group.
    pub source: CreationSource,
}

/// One thing that can be created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreationEntry {
    /// The [`CreationGroup::id`] this entry belongs to.
    pub group: String,
    /// The name shown for the entry.
    pub label: String,
    /// What activating it does: an `op:` operator call, a
    /// [`WIDGET_ACTION_PREFIX`] widget id, or an [`ASSET_ACTION_PREFIX`] kind.
    pub action: String,
}

/// The whole creation vocabulary, groups and entries both.
#[derive(Clone, Debug, Default)]
pub struct CreationTaxonomy {
    groups: Vec<CreationGroup>,
    entries: Vec<CreationEntry>,
}

/// Build an `op:` action string for the given operator type.
fn op_action<O: Operator>() -> String {
    format!("op:{}", O::ID)
}

fn group(id: &str, label: &str, order: i32) -> CreationGroup {
    CreationGroup {
        id: String::from(id),
        label: String::from(label),
        order,
        parent: None,
        source: CreationSource::BuiltIn,
    }
}

fn child_group(id: &str, label: &str, order: i32) -> CreationGroup {
    CreationGroup {
        parent: Some(String::from(THREE_D_GROUP)),
        ..group(id, label, order)
    }
}

fn extension_group(id: &str, label: &str) -> CreationGroup {
    CreationGroup {
        source: CreationSource::Extension,
        ..group(id, label, EXTENSION_GROUP_ORDER)
    }
}

fn entry(group: &str, label: &str, action: String) -> CreationEntry {
    CreationEntry {
        group: String::from(group),
        label: String::from(label),
        action,
    }
}

/// The built-in entity vocabulary: the groups, then the entries in them.
fn builtin(taxonomy: &mut CreationTaxonomy) {
    taxonomy.groups.extend([
        group(GENERAL_GROUP, GENERAL_SECTION, 0),
        group(THREE_D_GROUP, "3D", THREE_D_GROUP_ORDER),
        child_group("geometry", "Objects", THREE_D_GROUP_ORDER - 1),
        child_group("lights", "Lights", THREE_D_GROUP_ORDER - 2),
        child_group("cameras", "Cameras", THREE_D_GROUP_ORDER - 3),
        child_group("environment", "Environment", THREE_D_GROUP_ORDER - 4),
        child_group("terrain", "Terrain", THREE_D_GROUP_ORDER - 5),
    ]);

    taxonomy.entries.extend([
        entry(GENERAL_GROUP, "Empty", op_action::<EntityAddEmptyOp>()),
        entry(GENERAL_GROUP, "Prefab", op_action::<EntityAddPrefabOp>()),
        entry(
            GENERAL_GROUP,
            "Audio Source",
            op_action::<EntityAddAudioSourceOp>(),
        ),
        entry(
            GENERAL_GROUP,
            "Animation Player",
            op_action::<EntityAddAnimationPlayerOp>(),
        ),
        entry("geometry", "Cube", op_action::<EntityAddCubeOp>()),
        entry("geometry", "Sphere", op_action::<EntityAddSphereOp>()),
        entry("geometry", "Plane", op_action::<EntityAddPlaneOp>()),
        entry("geometry", "Cylinder", op_action::<EntityAddCylinderOp>()),
        entry("geometry", "Wedge", op_action::<EntityAddWedgeOp>()),
        entry("geometry", "Cone", op_action::<EntityAddConeOp>()),
        entry("geometry", "Pyramid", op_action::<EntityAddPyramidOp>()),
        entry("terrain", "Terrain", op_action::<EntityAddTerrainOp>()),
        entry(
            "lights",
            "Point Light",
            op_action::<EntityAddPointLightOp>(),
        ),
        entry(
            "lights",
            "Directional Light",
            op_action::<EntityAddDirectionalLightOp>(),
        ),
        entry("lights", "Spot Light", op_action::<EntityAddSpotLightOp>()),
        entry("cameras", "Camera", op_action::<EntityAddCameraOp>()),
        #[cfg(feature = "camera_rig")]
        entry("cameras", "Camera Rig", op_action::<EntityAddCameraRigOp>()),
        entry(
            "environment",
            "Fog Volume",
            op_action::<EntityAddFogVolumeOp>(),
        ),
        entry(
            "environment",
            "Reflection Probe",
            op_action::<EntityAddReflectionProbeOp>(),
        ),
    ]);

    #[cfg(feature = "multiplayer")]
    {
        taxonomy
            .groups
            .push(group("multiplayer", "Multiplayer", MULTIPLAYER_GROUP_ORDER));
        taxonomy.entries.extend([
            entry(
                "multiplayer",
                "Spawn Point",
                op_action::<EntityAddSpawnPointOp>(),
            ),
            entry(
                "multiplayer",
                "Zone Transition",
                op_action::<EntityAddZoneTransitionOp>(),
            ),
            entry(
                "multiplayer",
                "Network Room",
                op_action::<EntityAddNetworkRoomOp>(),
            ),
        ]);
    }
}

/// The 2D world entity kinds. Their operators are unavailable until a 2D world
/// scene is open.
fn two_d(taxonomy: &mut CreationTaxonomy) {
    taxonomy
        .groups
        .push(group(TWO_D_GROUP, "2D", TWO_D_GROUP_ORDER));
    taxonomy.entries.extend([
        entry(TWO_D_GROUP, SpriteAddOp::LABEL, op_action::<SpriteAddOp>()),
        entry(
            TWO_D_GROUP,
            "Empty 2D",
            op_action::<crate::entity_ops::EntityAddEmpty2dOp>(),
        ),
        entry(
            TWO_D_GROUP,
            "Camera",
            op_action::<crate::entity_ops::EntityAddCamera2dOp>(),
        ),
    ]);
}

/// The registered UI widgets, as one child group of [`UI_GROUP`] per category.
fn widgets(taxonomy: &mut CreationTaxonomy, world: &mut World) {
    let mut definitions = world
        .resource::<WidgetRegistry>()
        .iter()
        .map(|definition| {
            (
                definition.category.to_string(),
                definition.name.to_string(),
                definition.id.to_string(),
            )
        })
        .collect::<Vec<_>>();
    definitions.sort();
    if definitions.is_empty() {
        return;
    }

    taxonomy.groups.push(group(UI_GROUP, "UI", UI_GROUP_ORDER));
    for (category, label, id) in definitions {
        let child = ui_group_id(&category);
        if !taxonomy.groups.iter().any(|group| group.id == child) {
            taxonomy.groups.push(CreationGroup {
                id: child.clone(),
                label: category.clone(),
                order: ui_group_order(&category),
                parent: Some(String::from(UI_GROUP)),
                source: CreationSource::BuiltIn,
            });
        }
        taxonomy
            .entries
            .push(entry(&child, &label, format!("{WIDGET_ACTION_PREFIX}{id}")));
    }
}

/// One entry per asset kind the editor can write a file of. An asset needs a
/// project to land in, so the group is absent until one is open.
fn assets(taxonomy: &mut CreationTaxonomy, world: &mut World) {
    if world
        .get_resource::<crate::project::ProjectRoot>()
        .is_none()
    {
        return;
    }
    let kinds = crate::new_asset::creatable_kinds(world);
    if kinds.is_empty() {
        return;
    }
    taxonomy
        .groups
        .push(group(ASSETS_GROUP, ASSETS_SECTION, ASSETS_GROUP_ORDER));
    for kind in kinds {
        taxonomy.entries.push(entry(
            ASSETS_GROUP,
            &kind.label,
            format!("{ASSET_ACTION_PREFIX}{}", kind.kind),
        ));
    }
}

fn ui_group_id(category: &str) -> String {
    format!("{UI_GROUP}.{}", category.to_lowercase())
}

fn ui_group_order(category: &str) -> i32 {
    UI_CATEGORY_ORDER
        .iter()
        .position(|known| *known == category)
        .map_or(UI_UNKNOWN_CATEGORY_ORDER, |index| {
            UI_GROUP_ORDER - index as i32
        })
}

/// The Add entries extensions contribute, one group per extension.
fn extensions(taxonomy: &mut CreationTaxonomy, world: &mut World) {
    let mut query = world.query::<(
        &jackdaw_api_internal::lifecycle::RegisteredMenuEntry,
        Option<&ChildOf>,
    )>();
    let mut registered: Vec<(Entity, String, String)> = Vec::new();
    for (entry, parent) in query.iter(world) {
        if entry.menu != TopLevelMenu::Add {
            continue;
        }
        registered.push((
            parent.map(ChildOf::parent).unwrap_or(Entity::PLACEHOLDER),
            format!("op:{}", entry.operator_id),
            entry.label.clone(),
        ));
    }
    if registered.is_empty() {
        return;
    }

    // An extension id like `jackdaw.core` names a package, not a menu heading.
    let labels = world
        .resource::<jackdaw_api_internal::lifecycle::ExtensionCatalog>()
        .iter_with_content()
        .map(|(id, label, ..)| (id, label))
        .collect::<std::collections::HashMap<_, _>>();
    for (owner, action, label) in registered {
        let extension = world
            .get::<jackdaw_api_internal::lifecycle::Extension>(owner)
            .map(|extension| extension.id.clone());
        let heading = extension
            .as_ref()
            .and_then(|id| labels.get(id).cloned())
            .unwrap_or_else(|| String::from(EXTENSIONS_SECTION));
        let id = extension.unwrap_or_else(|| String::from(EXTENSIONS_SECTION));
        if !taxonomy.groups.iter().any(|group| group.id == id) {
            taxonomy.groups.push(extension_group(&id, &heading));
        }
        taxonomy.entries.push(entry(&id, &label, action));
    }
}

impl CreationTaxonomy {
    /// Read the whole vocabulary out of the world.
    pub fn collect(world: &mut World) -> Self {
        let mut taxonomy = Self::default();
        builtin(&mut taxonomy);
        two_d(&mut taxonomy);
        widgets(&mut taxonomy, world);
        assets(&mut taxonomy, world);
        extensions(&mut taxonomy, world);
        taxonomy.sort_groups();
        taxonomy
    }

    /// Order the groups greatest first, each group's children right after it
    /// and sorted among themselves, so a flat walk keeps a parent's children
    /// together.
    fn sort_groups(&mut self) {
        let orders = self
            .groups
            .iter()
            .map(|group| (group.id.clone(), group.order))
            .collect::<std::collections::HashMap<_, _>>();
        self.groups.sort_by_key(|group| {
            let top_order = group
                .parent
                .as_ref()
                .and_then(|parent| orders.get(parent))
                .copied()
                .unwrap_or(group.order);
            (
                std::cmp::Reverse(top_order),
                group.parent.is_some(),
                std::cmp::Reverse(group.order),
                group.label.clone(),
            )
        });
    }

    /// Every group, in the order surfaces show them.
    pub fn groups(&self) -> &[CreationGroup] {
        &self.groups
    }

    /// Every entry, in registration order.
    pub fn entries(&self) -> &[CreationEntry] {
        &self.entries
    }

    /// The group with this id, if the vocabulary has one.
    pub fn group(&self, id: &str) -> Option<&CreationGroup> {
        self.groups.iter().find(|group| group.id == id)
    }

    /// The entries of one group, in order.
    pub fn entries_of<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a CreationEntry> {
        self.entries.iter().filter(move |entry| entry.group == id)
    }

    /// The groups nested inside one group, in order.
    pub fn children_of<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a CreationGroup> {
        self.groups
            .iter()
            .filter(move |group| group.parent.as_deref() == Some(id))
    }

    /// The groups with no parent, in order.
    pub fn top_level(&self) -> impl Iterator<Item = &CreationGroup> {
        self.groups.iter().filter(|group| group.parent.is_none())
    }

    /// A group's label shown away from its parent, as in "UI: Controls".
    pub fn qualified_label(&self, id: &str) -> Option<String> {
        let group = self.group(id)?;
        let Some(parent) = group.parent.as_ref().and_then(|id| self.group(id)) else {
            return Some(group.label.clone());
        };
        Some(format!(
            "{}{QUALIFIED_LABEL_SEPARATOR}{}",
            parent.label, group.label
        ))
    }

    /// Whether a group's entries sit directly in the Add menu.
    fn renders_inline(&self, group: &CreationGroup) -> bool {
        if group.id == GENERAL_GROUP {
            return true;
        }
        // A lone asset kind reads as an entity kind without the heading that
        // says it writes a file.
        if group.id == ASSETS_GROUP {
            return false;
        }
        // An extension's group name credits the extension; inlining would make
        // its entries look built-in.
        if group.source == CreationSource::Extension {
            return false;
        }
        if self.children_of(&group.id).next().is_some() {
            return false;
        }
        self.entries_of(&group.id).count() <= INLINE_GROUP_MAX
    }

    /// The Add menu's rows: the General entries first, then one row per group.
    pub fn menu_rows(&self) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> = Vec::new();
        let mut extensions_opened = false;
        let groups = self.top_level().collect::<Vec<_>>();
        for (index, group) in groups.iter().enumerate() {
            if group.source == CreationSource::Extension && !extensions_opened {
                extensions_opened = true;
                push_separator(&mut rows);
            }
            if self.renders_inline(group) {
                rows.extend(self.entry_rows(&group.id));
                if group.id == GENERAL_GROUP && index + 1 < groups.len() {
                    push_separator(&mut rows);
                }
                continue;
            }
            rows.extend(submenu_row(&group.label, self.group_rows(&group.id)));
        }
        rows
    }

    /// One group's dropdown rows: its entries, or its children as sections.
    fn group_rows(&self, id: &str) -> Vec<(String, String)> {
        let children = self.children_of(id).collect::<Vec<_>>();
        if children.is_empty() {
            return self.entry_rows(id);
        }
        let mut rows = Vec::new();
        for (index, child) in children.iter().enumerate() {
            if index > 0 {
                rows.push(separator());
            }
            rows.push(section_label(&child.label));
            rows.extend(self.entry_rows(&child.id));
        }
        rows
    }

    fn entry_rows(&self, id: &str) -> Vec<(String, String)> {
        self.entries_of(id)
            .map(|entry| (entry.action.clone(), entry.label.clone()))
            .collect()
    }
}

fn separator() -> (String, String) {
    (String::from(SEPARATOR_ACTION), String::new())
}

/// Push a divider, unless one is already last or the rows are empty.
fn push_separator(rows: &mut Vec<(String, String)>) {
    if rows.last().map(|(action, _)| action.as_str()) == Some(SEPARATOR_ACTION) {
        return;
    }
    if rows.is_empty() {
        return;
    }
    rows.push(separator());
}

fn section_label(name: &str) -> (String, String) {
    (format!("{SECTION_ACTION_PREFIX}{name}"), String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A taxonomy holding the built-in vocabulary alone.
    fn builtin_taxonomy() -> CreationTaxonomy {
        let mut taxonomy = CreationTaxonomy::default();
        builtin(&mut taxonomy);
        two_d(&mut taxonomy);
        taxonomy.sort_groups();
        taxonomy
    }

    /// The built-in vocabulary plus one extension contributing one entry.
    fn with_extension() -> CreationTaxonomy {
        let mut taxonomy = builtin_taxonomy();
        taxonomy.groups.push(extension_group(
            "jackdaw.core",
            "Jackdaw Core Functionality",
        ));
        taxonomy.entries.push(entry(
            "jackdaw.core",
            "Draw Brush",
            String::from("op:viewport.draw_brush_modal"),
        ));
        taxonomy
    }

    fn with_widgets() -> CreationTaxonomy {
        let mut taxonomy = builtin_taxonomy();
        taxonomy.groups.push(group(UI_GROUP, "UI", UI_GROUP_ORDER));
        for (category, entries) in [
            ("Layout", vec!["Panel", "Row"]),
            ("Display", vec!["Label"]),
            ("Controls", vec!["Button", "Slider"]),
        ] {
            let id = ui_group_id(category);
            taxonomy.groups.push(CreationGroup {
                id: id.clone(),
                label: String::from(category),
                order: ui_group_order(category),
                parent: Some(String::from(UI_GROUP)),
                source: CreationSource::BuiltIn,
            });
            for label in entries {
                taxonomy.entries.push(entry(
                    &id,
                    label,
                    format!("{WIDGET_ACTION_PREFIX}ui.{}", label.to_lowercase()),
                ));
            }
        }
        taxonomy.sort_groups();
        taxonomy
    }

    #[test]
    fn every_entry_names_a_group_that_exists() {
        let taxonomy = with_widgets();
        for entry in taxonomy.entries() {
            assert!(
                taxonomy.group(&entry.group).is_some(),
                "{} names group `{}`, which is not registered",
                entry.label,
                entry.group,
            );
        }
    }

    #[test]
    fn groups_come_in_their_declared_order() {
        let taxonomy = builtin_taxonomy();
        let labels = taxonomy
            .top_level()
            .map(|group| group.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            labels.first(),
            Some(&GENERAL_SECTION),
            "the general vocabulary opens the menu: {labels:?}",
        );
        let orders = taxonomy
            .top_level()
            .map(|group| group.order)
            .collect::<Vec<_>>();
        let mut sorted = orders.clone();
        sorted.sort_by_key(|order| std::cmp::Reverse(*order));
        assert_eq!(orders, sorted, "groups are ordered, greatest first");
    }

    #[test]
    fn a_nested_group_reads_with_its_parents_name() {
        let taxonomy = with_widgets();
        assert_eq!(
            taxonomy.qualified_label(&ui_group_id("Controls")),
            Some(String::from("UI: Controls")),
        );
        assert_eq!(
            taxonomy.qualified_label(UI_GROUP),
            Some(String::from("UI")),
            "a top-level group is already named in full",
        );
    }

    #[test]
    fn the_general_group_stays_in_the_menu_and_the_rest_expand() {
        let taxonomy = with_widgets();
        let rows = taxonomy.menu_rows();
        let actions = rows.iter().map(|(action, _)| action.as_str());

        assert!(
            actions
                .clone()
                .take_while(|action| *action != SEPARATOR_ACTION)
                .all(|action| action.starts_with("op:")),
            "the menu opens on the general entries themselves: {rows:?}",
        );
        for label in ["2D", "3D", "UI"] {
            assert!(
                rows.iter().any(|(action, _)| *action
                    == format!(
                        "{}{label}",
                        jackdaw_feathers::menu_bar::SUBMENU_ACTION_PREFIX
                    )),
                "`{label}` is a group the menu expands: {rows:?}",
            );
        }
    }

    #[test]
    fn the_three_d_group_expands_into_its_categories() {
        let taxonomy = builtin_taxonomy();
        let children = taxonomy
            .children_of(THREE_D_GROUP)
            .map(|group| group.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            children,
            ["Objects", "Lights", "Cameras", "Environment", "Terrain"],
        );
        assert!(
            taxonomy
                .entries_of("terrain")
                .any(|entry| entry.label == "Terrain")
        );
    }

    #[test]
    fn the_general_group_holds_audio_and_animation_and_no_reference_image() {
        let taxonomy = builtin_taxonomy();
        let labels = taxonomy
            .entries_of(GENERAL_GROUP)
            .map(|entry| entry.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            ["Empty", "Prefab", "Audio Source", "Animation Player"],
        );
        assert!(
            !taxonomy
                .entries()
                .iter()
                .any(|entry| entry.action == "op:entity.add.image")
        );
    }

    #[test]
    fn a_group_of_one_stands_in_for_its_entry() {
        let mut taxonomy = builtin_taxonomy();
        taxonomy.groups.push(group("solo", "Solo", -15));
        taxonomy
            .entries
            .push(entry("solo", "Lone Thing", String::from("op:lone.thing")));
        taxonomy.sort_groups();
        let rows = taxonomy.menu_rows();
        assert!(
            rows.iter().any(|(action, _)| action == "op:lone.thing"),
            "the single entry sits in the menu: {rows:?}",
        );
        assert!(
            !rows.iter().any(|(action, _)| *action
                == format!("{}Solo", jackdaw_feathers::menu_bar::SUBMENU_ACTION_PREFIX)),
            "a group of one is not worth expanding: {rows:?}",
        );
    }

    #[test]
    fn children_follow_their_parent_in_the_flat_order() {
        let taxonomy = with_widgets();
        let ids = taxonomy
            .groups()
            .iter()
            .map(|group| group.id.as_str())
            .collect::<Vec<_>>();
        let position = |id: &str| ids.iter().position(|found| *found == id).unwrap();
        let three_d = position(THREE_D_GROUP);
        let children = ["geometry", "lights", "cameras", "environment", "terrain"];
        for (offset, child) in children.iter().enumerate() {
            assert_eq!(position(child), three_d + 1 + offset, "{ids:?}");
        }
        assert!(position(UI_GROUP) > three_d + children.len(), "{ids:?}");
        let orders = taxonomy
            .groups()
            .iter()
            .map(|group| group.order)
            .collect::<Vec<_>>();
        assert_eq!(
            orders
                .iter()
                .filter(|order| **order == THREE_D_GROUP_ORDER)
                .count(),
            1,
            "no child shares the parent's order: {orders:?}",
        );
    }

    #[test]
    fn an_extensions_single_entry_keeps_its_byline() {
        let taxonomy = with_extension();
        let rows = taxonomy.menu_rows();
        assert!(
            rows.iter().any(|(action, _)| *action
                == format!(
                    "{}Jackdaw Core Functionality",
                    jackdaw_feathers::menu_bar::SUBMENU_ACTION_PREFIX
                )),
            "the extension's group is a row that expands: {rows:?}",
        );
        let divider = rows
            .iter()
            .position(|(action, _)| action == SEPARATOR_ACTION)
            .expect("the menu has a divider");
        let extension = rows
            .iter()
            .position(|(action, _)| action.contains("Jackdaw Core"))
            .expect("the extension group is in the menu");
        assert!(
            divider < extension,
            "a divider opens the contributed block: {rows:?}",
        );
    }

    #[test]
    fn the_ui_group_expands_into_its_categories() {
        let taxonomy = with_widgets();
        let rows = taxonomy.menu_rows();
        let start = rows
            .iter()
            .position(|(action, _)| {
                *action == format!("{}UI", jackdaw_feathers::menu_bar::SUBMENU_ACTION_PREFIX)
            })
            .expect("the UI group is in the menu");
        let end = rows[start..]
            .iter()
            .position(|(action, _)| action == jackdaw_feathers::menu_bar::SUBMENU_END_ACTION)
            .expect("the UI group is closed")
            + start;
        let inside = &rows[start + 1..end];

        for category in ["Layout", "Display", "Controls"] {
            assert!(
                inside
                    .iter()
                    .any(|(action, _)| *action == format!("{SECTION_ACTION_PREFIX}{category}")),
                "`{category}` is a section of the UI group: {inside:?}",
            );
        }
        assert!(
            inside
                .iter()
                .any(|(action, _)| action.starts_with(WIDGET_ACTION_PREFIX)),
            "the widgets are in there with their sections: {inside:?}",
        );
    }
}
