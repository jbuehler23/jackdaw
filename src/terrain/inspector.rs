//! The Components inspector's Terrain section: authored component data only
//! (resolution, size, max height).
//!
//! Tool state (brush, quantization) and generation and scatter parameters live
//! in the contextual options bar and the dockable Terrain panel instead (see
//! `options_bar.rs` and `panel.rs`).

use bevy::prelude::*;
use jackdaw_feathers::tokens;

use super::TerrainDataStore;
use super::ui_fields::spawn_error_hint;
use crate::selection::Selection;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        update_terrain_inspector.run_if(in_state(crate::AppState::Editor)),
    );
}

/// Marker for the terrain inspector container.
#[derive(Component)]
pub struct TerrainInspectorContainer;

/// Spawns the terrain inspector container. Called from the component display system.
pub fn spawn_terrain_inspector_container(commands: &mut Commands, parent: Entity) {
    commands.spawn((
        TerrainInspectorContainer,
        Node {
            flex_direction: FlexDirection::Column,
            width: Val::Percent(100.0),
            row_gap: px(tokens::SPACING_SM),
            ..Default::default()
        },
        ChildOf(parent),
    ));
}

/// What was last rendered, so an unchanged frame does not rebuild.
#[derive(Default, PartialEq)]
struct InspectorState {
    terrain_entity: Option<Entity>,
    /// Alongside `terrain_entity` rather than replacing it: a quarantine can
    /// land after the terrain is selected (a reload that keeps selection), and
    /// the row has to appear without a reselect.
    quarantine_reason: Option<String>,
}

fn update_terrain_inspector(
    mut commands: Commands,
    selection: Res<Selection>,
    terrains: Query<(), With<jackdaw_scene_types::Terrain>>,
    container_query: Query<(Entity, Option<&Children>), With<TerrainInspectorContainer>>,
    mut local_state: Local<InspectorState>,
    icon_font: Res<jackdaw_feathers::icons::IconFont>,
    terrain_data: Query<&jackdaw_scene_types::Terrain>,
    terrain_store: Res<TerrainDataStore>,
) {
    let terrain_entity = selection.primary().filter(|&e| terrains.contains(e));
    let quarantine_reason = terrain_entity
        .and_then(|e| terrain_data.get(e).ok())
        .and_then(|terrain| terrain_store.load_failed_reason(&terrain.data_path))
        .map(str::to_string);

    let state = InspectorState {
        terrain_entity,
        quarantine_reason,
    };
    if *local_state == state {
        return;
    }

    // A container is spawned a frame after selection, so this retries next
    // frame rather than marking the render done before one exists.
    if container_query.is_empty() {
        return;
    }

    *local_state = state;

    // Multi-instance dock layouts host more than one
    // TerrainInspectorContainer (see component_display.rs), each with its own
    // subtree.
    for (container, children) in &container_query {
        if let Some(children) = children {
            for child in children.iter() {
                commands.entity(child).despawn();
            }
        }

        let Some(terrain_entity_id) = terrain_entity else {
            continue;
        };

        // Read off the `Terrain` component rather than `TerrainDataStore`, so
        // it renders for a terrain that has never been sculpted or generated.
        // Read-only: resolution and size drive chunk layout, and no operator
        // resizes those.
        let (_section, body) = jackdaw_feathers::collapsible::collapsible_section(
            &mut commands,
            "Terrain",
            &icon_font.0,
            container,
        );
        if let Ok(terrain) = terrain_data.get(terrain_entity_id) {
            // Cell size is authored; extent is whatever the terrain has been
            // sculpted into, so it is reported rather than offered.
            let shape = terrain_store.grid_shape(terrain);
            spawn_readonly_field(
                &mut commands,
                body,
                "Cell Size",
                &format!("{:.2} m", terrain.cell_size),
            );
            spawn_readonly_field(
                &mut commands,
                body,
                "Cells",
                &format!("{} x {}", shape.resolution, shape.resolution),
            );
            spawn_readonly_field(
                &mut commands,
                body,
                "Ground",
                &format!("{:.1} x {:.1} m", shape.size.x, shape.size.y),
            );
            spawn_readonly_field(
                &mut commands,
                body,
                "Max Height",
                &format!("{:.1}", terrain.max_height),
            );
            if let Some(reason) = terrain_store.load_failed_reason(&terrain.data_path) {
                spawn_error_hint(&mut commands, body, &format!("read-only: {reason}"));
            }
        }
    }
}

/// A label and a read-only value on one row.
///
/// For authored values the panel shows but has no operator to change, such as a
/// terrain's resolution and size.
fn spawn_readonly_field(commands: &mut Commands, parent: Entity, label: &str, value: &str) {
    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    commands.spawn((
        Text::new(label),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(row),
    ));
    commands.spawn((
        Text::new(value.to_string()),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_BODY_COLOR.into()),
        ChildOf(row),
    ));
}

#[cfg(test)]
mod update_terrain_inspector_tests {
    use jackdaw_feathers::icons::IconFont;

    use super::*;
    use crate::selection::Selection;

    fn base_world() -> World {
        let mut world = World::new();
        world.init_resource::<Selection>();
        world.init_resource::<TerrainDataStore>();
        world.insert_resource(IconFont(Handle::default()));
        world
    }

    fn select_a_terrain(world: &mut World) {
        let terrain = world.spawn(jackdaw_scene_types::Terrain::default()).id();
        world.resource_mut::<Selection>().entities = vec![terrain];
    }

    /// A frame where the container does not exist yet must not mark the render
    /// done, or the inspector never retries.
    #[test]
    fn a_container_that_appears_a_frame_late_still_gets_rendered() {
        let mut world = base_world();
        select_a_terrain(&mut world);

        // Frame 1: selection changed, no container yet.
        world
            .run_system_cached(update_terrain_inspector)
            .expect("system runs");
        world.flush();

        // Frame 2: the container shows up.
        let container = world
            .spawn((TerrainInspectorContainer, Node::default()))
            .id();
        world
            .run_system_cached(update_terrain_inspector)
            .expect("system runs");
        world.flush();

        let children = world.get::<Children>(container);
        assert!(
            children.is_some_and(|c| !c.is_empty()),
            "the container must be populated once it exists, even a frame late",
        );
    }

    /// A multi-instance dock layout spawns one `TerrainInspectorContainer` per
    /// docked inspector panel, and every one is populated.
    #[test]
    fn every_docked_container_gets_rendered() {
        let mut world = base_world();
        select_a_terrain(&mut world);

        let a = world
            .spawn((TerrainInspectorContainer, Node::default()))
            .id();
        let b = world
            .spawn((TerrainInspectorContainer, Node::default()))
            .id();

        world
            .run_system_cached(update_terrain_inspector)
            .expect("system runs");
        world.flush();

        for container in [a, b] {
            let children = world.get::<Children>(container);
            assert!(
                children.is_some_and(|c| !c.is_empty()),
                "container {container:?} must be populated",
            );
        }
    }

    /// Depth-first collection of every `Text` string under `entity`, in
    /// document order.
    fn collect_texts(world: &World, entity: Entity, out: &mut Vec<String>) {
        if let Some(text) = world.get::<Text>(entity) {
            out.push(text.0.clone());
        }
        if let Some(children) = world.get::<Children>(entity) {
            for child in children.iter() {
                collect_texts(world, child, out);
            }
        }
    }

    /// A terrain that has never been sculpted, painted or generated still has
    /// authored `resolution`, `size` and `max_height` on its component, and the
    /// panel shows them from there rather than waiting for a
    /// `TerrainDataStore` entry or a Generate or Erode run.
    #[test]
    fn terrain_properties_render_for_a_never_generated_terrain() {
        let mut world = base_world();
        let terrain = jackdaw_scene_types::Terrain {
            cell_size: 2.5,
            max_height: 20.0,
            ..Default::default()
        };
        let entity = world.spawn(terrain).id();
        world.resource_mut::<Selection>().entities = vec![entity];

        let container = world
            .spawn((TerrainInspectorContainer, Node::default()))
            .id();
        world
            .run_system_cached(update_terrain_inspector)
            .expect("system runs");
        world.flush();

        let mut texts = Vec::new();
        collect_texts(&world, container, &mut texts);

        assert!(
            texts.iter().any(|t| t == "2.50 m"),
            "cell size must render from the component: {texts:?}",
        );
        // A terrain nobody has sculpted holds no cells, and says so rather than
        // reporting a rectangle it never declared.
        assert!(
            texts.iter().any(|t| t == "0 x 0"),
            "the stored extent must render: {texts:?}",
        );
        assert!(
            texts.iter().any(|t| t == "20.0"),
            "max height must render from the component: {texts:?}",
        );
    }

    /// A terrain whose sidecar failed to load shows a "read-only: reason" row
    /// in the Components inspector as well as in the Textures tab.
    #[test]
    fn a_quarantined_terrains_read_only_reason_surfaces_in_the_inspector() {
        let mut world = base_world();
        let terrain = jackdaw_scene_types::Terrain {
            data_path: "zone1.jdterrain".to_string(),
            ..Default::default()
        };
        let entity = world.spawn(terrain).id();
        world.resource_mut::<Selection>().entities = vec![entity];
        world
            .resource_mut::<TerrainDataStore>()
            .mark_load_failed("zone1.jdterrain", "unsupported version 9");

        let container = world
            .spawn((TerrainInspectorContainer, Node::default()))
            .id();
        world
            .run_system_cached(update_terrain_inspector)
            .expect("system runs");
        world.flush();

        let mut texts = Vec::new();
        collect_texts(&world, container, &mut texts);

        assert!(
            texts
                .iter()
                .any(|t| t == "read-only: unsupported version 9"),
            "the quarantine reason must surface verbatim: {texts:?}",
        );
    }

    /// Tool and parameter blocks (Paint Channels, Brush, Scatter,
    /// Quantization, Generation, Erosion) belong to the options bar and the
    /// Terrain panel, not the Components inspector.
    #[test]
    fn tool_and_parameter_sections_stay_out_of_the_components_inspector() {
        let mut world = base_world();
        select_a_terrain(&mut world);

        let container = world
            .spawn((TerrainInspectorContainer, Node::default()))
            .id();
        world
            .run_system_cached(update_terrain_inspector)
            .expect("system runs");
        world.flush();

        let mut texts = Vec::new();
        collect_texts(&world, container, &mut texts);

        for removed in [
            "Paint Channels",
            "Brush",
            "Scatter",
            "Quantization",
            "Terrain Generation",
            "Hydraulic Erosion",
        ] {
            assert!(
                !texts.iter().any(|t| t == removed),
                "'{removed}' must no longer appear in the Components inspector: {texts:?}",
            );
        }
    }
}
