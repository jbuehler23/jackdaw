//! The Navmesh terrain tool: an entry in the terrain tool palette whose
//! options bar holds what the agent is, the Bake action, whether the result
//! draws, and what the last bake produced.
//!
//! The agent chips are scene data on the terrain, so they commit through
//! `component.set` on release, undoable. The overlay toggle is view state and
//! leaves no history entry. There is no voxel size field; the voxel follows
//! the terrain's own cell, and a second size to keep in step with it could
//! produce a navmesh finer than the heights behind it.

use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::ValueChange;
use jackdaw_api::keymap::PresetInput;
use jackdaw_api::prelude::*;
use jackdaw_api::terrain::{TerrainEditMode, TerrainToolOptions};
use jackdaw_api::ui::{
    ButtonProps, ButtonVariant, FieldKind, spawn_checkbox, spawn_hint, spawn_scrub_chip,
};
use jackdaw_terrain::navmesh::BakeParams;

use crate::bake::{
    ShowNavmesh, TerrainNavmeshBakeOp, TerrainNavmeshState, TerrainNavmeshToggleOverlayOp,
    has_selected_terrain, summary,
};

pub(crate) fn register(ctx: &mut ExtensionRegistrar) {
    ctx.register_operator::<TerrainToolNavmeshOp>()
        .register_terrain_tool::<TerrainToolNavmeshOp>(Icon::Waypoints)
        .bind_operator_host::<TerrainToolNavmeshOp>([PresetInput::key("Digit8").alt()]);
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        fill_options
            .run_if(in_state(AppState::Editor))
            .run_if(resource_exists::<TerrainNavmeshState>),
    )
    .add_observer(commit_agent_field)
    .add_observer(toggle_overlay_from_checkbox);
}

/// Pick the Navmesh tool. Claims no viewport input.
#[operator(
    id = "terrain.tool.navmesh",
    label = "Navmesh",
    description = "Bake a navigation mesh from this terrain.",
    is_available = has_selected_terrain
)]
pub fn terrain_tool_navmesh(
    _: In<OperatorParameters>,
    mut mode: ResMut<TerrainEditMode>,
) -> OperatorResult {
    mode.toggle(TerrainEditMode::Extension(TerrainToolNavmeshOp::ID));
    OperatorResult::Finished
}

/// Tags the agent chips. They describe the character a bake is for, which is
/// authored on the terrain, so they write the scene document rather than a
/// resource.
#[derive(Component, Clone, Copy)]
enum AgentField {
    Radius,
    Height,
    MaxSlope,
    Climb,
    MinObstacle,
}

impl AgentField {
    /// The field of `Terrain` the chip edits.
    fn path(self) -> &'static str {
        match self {
            Self::Radius => "navmesh.agent_radius",
            Self::Height => "navmesh.agent_height",
            Self::MaxSlope => "navmesh.max_slope_degrees",
            Self::Climb => "navmesh.climb",
            Self::MinObstacle => "navmesh.min_obstacle_size",
        }
    }

    fn clamp(self, value: f32) -> f32 {
        match self {
            Self::Radius => value.clamp(0.05, 3.0),
            Self::Height => value.clamp(0.5, 6.0),
            Self::MaxSlope => value.clamp(0.0, 85.0),
            Self::Climb => value.clamp(0.0, BakeParams::MAX_CLIMB),
            Self::MinObstacle => value.clamp(0.0, 2.0),
        }
    }
}

/// Tags the overlay checkbox.
#[derive(Component)]
struct OverlayCheckbox;

/// What the options last showed, so they rebuild when the bake, the overlay
/// or the agent changes and never while a chip is being dragged.
#[derive(PartialEq, Clone, Debug)]
struct Shown {
    slot: Entity,
    show_overlay: bool,
    summary: String,
    agent: jackdaw_scene_types::TerrainNavmesh,
}

fn fill_options(
    slots: Query<(Entity, &TerrainToolOptions)>,
    selection: Res<Selection>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    state: Res<TerrainNavmeshState>,
    show: Option<Res<ShowNavmesh>>,
    mut shown: Local<Option<Shown>>,
    mut commands: Commands,
) {
    let Some((slot, _)) = slots
        .iter()
        .find(|(_, options)| options.tool == TerrainToolNavmeshOp::ID)
    else {
        *shown = None;
        return;
    };
    let now = Shown {
        slot,
        show_overlay: show.is_some_and(|show| show.0),
        summary: summary(&state),
        agent: selection
            .primary()
            .and_then(|entity| terrains.get(entity).ok())
            .map(|terrain| terrain.navmesh.clone())
            .unwrap_or_default(),
    };
    if shown.as_ref() == Some(&now) {
        return;
    }
    commands.entity(slot).despawn_children();
    spawn_fields(&mut commands, slot, &now);
    *shown = Some(now);
}

fn spawn_fields(commands: &mut Commands, parent: Entity, shown: &Shown) {
    let agent = &shown.agent;
    // Radius reads as continuous and is not: the baker erodes in whole voxels,
    // using `ceil(radius / voxel)`. At the voxel a kilometre of terrain bakes
    // at, the whole chip range lands on about six distinct erosions, so
    // neighbouring values on the chip give the same navmesh.
    spawn_scrub_chip(
        commands,
        parent,
        "Agent Radius",
        "How wide the character is. Walkable ground is pulled back this far from every wall",
        agent.agent_radius,
        0.05..3.0,
        FieldKind::Continuous,
        AgentField::Radius,
    );
    spawn_scrub_chip(
        commands,
        parent,
        "Agent Height",
        "How tall the character is. Ground with less headroom than this is not walkable",
        agent.agent_height,
        0.5..6.0,
        FieldKind::Continuous,
        AgentField::Height,
    );
    spawn_scrub_chip(
        commands,
        parent,
        "Max Slope",
        "Steepest ground the character can stand on, in degrees",
        agent.max_slope_degrees,
        0.0..85.0,
        FieldKind::Continuous,
        AgentField::MaxSlope,
    );
    spawn_scrub_chip(
        commands,
        parent,
        "Step Height",
        "Tallest step the character walks up without leaving the ground. Kerbs and stair \
         treads under this stay one connected surface",
        agent.climb,
        0.0..2.0,
        FieldKind::Continuous,
        AgentField::Climb,
    );
    spawn_scrub_chip(
        commands,
        parent,
        "Min Obstacle",
        "Smallest scene mesh the bake rasterizes. Below this a piece is decoration the bake \
         walks over. 0 filters nothing",
        agent.min_obstacle_size,
        0.0..2.0,
        FieldKind::Continuous,
        AgentField::MinObstacle,
    );
    commands.spawn((
        button(
            ButtonProps::new("Bake Navmesh")
                .with_variant(ButtonVariant::Primary)
                .call_operator(TerrainNavmeshBakeOp::ID),
        ),
        ChildOf(parent),
    ));
    spawn_checkbox(
        commands,
        parent,
        "Show Navmesh",
        shown.show_overlay,
        OverlayCheckbox,
    );
    spawn_hint(commands, parent, &shown.summary);
}

/// Writes an agent chip to the selected terrain once the drag or the typing
/// ends: a commit per drag frame would leave a history entry each frame.
fn commit_agent_field(
    event: On<ValueChange<f32>>,
    fields: Query<&AgentField>,
    selection: Res<Selection>,
    mut commands: Commands,
) {
    let Ok(&field) = fields.get(event.event_target()) else {
        return;
    };
    if !event.is_final {
        return;
    }
    let Some(entity) = selection.primary() else {
        return;
    };
    commands
        .operator("component.set")
        .param("entity", entity)
        .param("type_path", "jackdaw_scene_types::types::Terrain")
        .param("field", field.path())
        .param("value", field.clamp(event.value).to_string())
        .call();
}

/// `FeathersCheckbox` does not keep its own `Checked`, so this reflects the
/// new value onto the checkbox before toggling the overlay.
fn toggle_overlay_from_checkbox(
    event: On<ValueChange<bool>>,
    checkboxes: Query<(), With<OverlayCheckbox>>,
    mut commands: Commands,
) {
    let target = event.event_target();
    if !checkboxes.contains(target) {
        return;
    }
    if event.value {
        commands.entity(target).insert(Checked);
    } else {
        commands.entity(target).remove::<Checked>();
    }
    commands.operator(TerrainNavmeshToggleOverlayOp::ID).call();
}
