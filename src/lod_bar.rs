//! The LOD bar, as Unity's LOD Group shows one: a segment per level from the
//! most detailed down to culled, labelled with the screen height each level
//! shows from. Dragging a divider moves a switch, one undo entry a drag, and
//! clicking a segment draws every group at that level in the viewport.
//!
//! The bar edits a model's import settings on its file card, and a placed
//! model's own [`LodOverride`] on its placement card.

use bevy::picking::events::{Click, Drag, DragEnd, DragStart, Pointer};
use bevy::prelude::*;
use bevy::ui::FocusPolicy;
use jackdaw_api::prelude::*;
use jackdaw_commands::CommandHistory;
use jackdaw_feathers::button::{ButtonOperatorCall, ButtonProps, button};
use jackdaw_feathers::tokens;
use jackdaw_scene_types::LodOverride;
use jackdaw_scene_types::model_import::{ModelLevels, ModelLod, ModelLodIndex};
use jackdaw_scene_types::model_parts::source_path;

use crate::commands::EditorCommand;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<BarDrag>()
        .init_resource::<LodColorView>()
        .add_systems(Update, (follow_camera, follow_target, tint_lod_parts))
        .add_observer(begin_divider_drag)
        .add_observer(drag_divider)
        .add_observer(end_divider_drag)
        .add_observer(force_clicked_level);
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<LodRevertOverrideOp>()
        .register_operator::<OpenModelSettingsOp>()
        .register_operator::<ViewToggleLodColorsOp>()
        .register_operator::<ModelLodRevertOp>()
        .register_operator::<ModelLodSetFadeOp>();
}

/// The colour each level is drawn in, on the bar and in the LOD colour view.
pub fn level_color(level: usize) -> Color {
    const COLORS: [Color; 6] = [
        Color::srgb(0.30, 0.56, 0.27),
        Color::srgb(0.25, 0.44, 0.65),
        Color::srgb(0.50, 0.36, 0.66),
        Color::srgb(0.70, 0.52, 0.24),
        Color::srgb(0.22, 0.58, 0.58),
        Color::srgb(0.62, 0.34, 0.48),
    ];
    COLORS[level % COLORS.len()]
}

/// The colour of the culled end of the bar.
pub const CULLED_COLOR: Color = Color::srgb(0.42, 0.20, 0.20);

/// What a bar edits.
#[derive(Clone, Debug, PartialEq)]
pub enum BarTarget {
    /// A model's import settings, by asset path.
    Model(String),
    /// A placed model's override of its model's settings.
    Placement(Entity),
}

/// A LOD bar and the screen heights it shows.
#[derive(Component, Clone, Debug)]
pub struct LodBar {
    pub target: BarTarget,
    pub heights: Vec<f32>,
}

/// One segment of a bar: a level, or the culled end at `level == count`.
#[derive(Component, Clone, Copy, Debug)]
pub struct LodBarSegment {
    pub bar: Entity,
    pub level: usize,
}

/// The divider after `level` on a bar, which moves that level's screen height.
#[derive(Component, Clone, Copy, Debug)]
pub struct LodBarDivider {
    pub bar: Entity,
    pub level: usize,
}

/// The line on a placement's bar showing how much of the screen the placed
/// model covers from the main viewport.
#[derive(Component, Clone, Copy, Debug)]
pub struct LodBarCamera {
    pub bar: Entity,
}

/// What a divider drag started from, so its end can be one undo entry.
#[derive(Resource, Default)]
struct BarDrag {
    started: Option<DragStartState>,
}

enum DragStartState {
    Model {
        lod: Option<ModelLod>,
        unsaved: bool,
    },
    Placement(Option<LodOverride>),
}

const BAR_HEIGHT: f32 = 30.0;
const DIVIDER_WIDTH: f32 = 6.0;
const MIN_GAP: f32 = 0.001;

/// The screen heights and fade a target shows now.
fn current(world: &World, target: &BarTarget) -> Option<(Vec<f32>, f32)> {
    match target {
        BarTarget::Model(path) => {
            let lod = model_levels(world, path)?;
            Some((
                lod.levels.iter().map(|level| level.screen_height).collect(),
                lod.fade.width(),
            ))
        }
        BarTarget::Placement(entity) => {
            let levels = world.get::<ModelLevels>(*entity)?;
            Some((
                levels
                    .group
                    .levels
                    .iter()
                    .map(|level| level.screen_height)
                    .collect(),
                levels.group.fade,
            ))
        }
    }
}

/// A model's levels as the editor holds them: in memory once read, or as its
/// meta has them.
pub fn model_levels(world: &World, path: &str) -> Option<ModelLod> {
    let index = world.resource::<ModelLodIndex>();
    if index.is_known(path) {
        return index.get(path).map(|lod| (**lod).clone());
    }
    let assets = world
        .get_resource::<crate::project::ProjectRoot>()?
        .assets_dir();
    crate::model_lod::levels_on_disk(&assets.join(path))
}

/// The left edge of the point on a bar where the screen height is `height`,
/// as a share of the bar: 100% of the screen at the left, nothing at the right.
fn left_of(height: f32) -> f32 {
    (1.0 - height.clamp(0.0, 1.0)) * 100.0
}

fn percent(share: f32) -> String {
    let value = share * 100.0;
    if (value - value.round()).abs() < 0.05 {
        format!("{value:.0}%")
    } else {
        format!("{value:.1}%")
    }
}

/// Spawn a bar for `target` under `parent`.
pub fn spawn_bar(world: &mut World, parent: Entity, target: BarTarget) -> Option<Entity> {
    let (heights, _) = current(world, &target)?;
    let count = heights.len();
    let bar = world
        .spawn((
            LodBar {
                target: target.clone(),
                heights: heights.clone(),
            },
            Node {
                flex_direction: FlexDirection::Row,
                width: Val::Percent(100.0),
                height: Val::Px(BAR_HEIGHT),
                margin: UiRect::vertical(Val::Px(tokens::SPACING_XS)),
                border_radius: BorderRadius::all(tokens::CORNER_RADIUS_LG),
                overflow: Overflow::clip(),
                ..default()
            },
            ChildOf(parent),
        ))
        .id();
    for level in 0..=count {
        let color = if level == count {
            CULLED_COLOR
        } else {
            level_color(level)
        };
        let segment = world
            .spawn((
                LodBarSegment { bar, level },
                Node {
                    height: Val::Percent(100.0),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    overflow: Overflow::clip(),
                    ..default()
                },
                BackgroundColor(color),
                ChildOf(bar),
            ))
            .id();
        world.spawn((
            Text::new(""),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(Color::WHITE),
            TextLayout {
                linebreak: bevy::text::LineBreak::NoWrap,
                ..default()
            },
            FocusPolicy::Pass,
            Pickable::IGNORE,
            ChildOf(segment),
        ));
    }
    for level in 0..count {
        world.spawn((
            LodBarDivider { bar, level },
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(DIVIDER_WIDTH),
                height: Val::Percent(100.0),
                margin: UiRect::left(Val::Px(-DIVIDER_WIDTH / 2.0)),
                ..default()
            },
            BackgroundColor(tokens::PANEL_BG.with_alpha(0.9)),
            ChildOf(bar),
        ));
    }
    if matches!(target, BarTarget::Placement(_)) {
        world.spawn((
            LodBarCamera { bar },
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(2.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(Color::WHITE),
            Pickable::IGNORE,
            ChildOf(bar),
        ));
    }
    lay_out_bar(world, bar);
    Some(bar)
}

/// Size each segment, place each divider and label each level from the
/// bar's heights.
fn lay_out_bar(world: &mut World, bar: Entity) {
    let Some(heights) = world.get::<LodBar>(bar).map(|bar| bar.heights.clone()) else {
        return;
    };
    let children: Vec<Entity> = world
        .get::<Children>(bar)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    let count = heights.len();
    let above = |level: usize| {
        level
            .checked_sub(1)
            .and_then(|before| heights.get(before))
            .copied()
            .unwrap_or(1.0)
    };
    for child in children {
        if let Some(segment) = world.get::<LodBarSegment>(child).copied() {
            let top = above(segment.level);
            let bottom = heights.get(segment.level).copied().unwrap_or(0.0);
            let share = (top - bottom).max(0.0);
            let label = if segment.level == count {
                format!("Culled {}", percent(bottom.max(0.0)))
            } else {
                format!("LOD{} {}", segment.level, percent(top))
            };
            if let Some(mut node) = world.get_mut::<Node>(child) {
                node.width = Val::Percent(share * 100.0);
            }
            let text = world
                .get::<Children>(child)
                .and_then(|children| children.first().copied());
            if let Some(text) = text
                && let Some(mut shown) = world.get_mut::<Text>(text)
                && shown.0 != label
            {
                shown.0 = label;
            }
        } else if let Some(divider) = world.get::<LodBarDivider>(child).copied() {
            let at = heights.get(divider.level).copied().unwrap_or(0.0);
            if let Some(mut node) = world.get_mut::<Node>(child) {
                node.left = Val::Percent(left_of(at));
            }
        }
    }
}

/// How much of the main viewport's height a placed model covers.
fn screen_share(world: &mut World, entity: Entity) -> Option<f32> {
    let size = world.get::<jackdaw_runtime::LodSwitches>(entity)?.size;
    let at = world.get::<GlobalTransform>(entity)?.translation();
    let mut cameras = world.query_filtered::<(&GlobalTransform, &Projection), With<crate::viewport::MainViewportCamera>>();
    let (eye, projection) = cameras.iter(world).next()?;
    let Projection::Perspective(perspective) = projection else {
        return None;
    };
    let distance = eye.translation().distance(at).max(1e-3);
    Some(size / (2.0 * distance * (perspective.fov / 2.0).tan()))
}

/// Keep each placement bar's camera line where the main viewport sees the
/// placed model from.
pub(crate) fn follow_camera(world: &mut World) {
    let mut markers = world.query::<(Entity, &LodBarCamera)>();
    let markers: Vec<(Entity, Entity)> = markers
        .iter(world)
        .map(|(entity, marker)| (entity, marker.bar))
        .collect();
    for (marker, bar) in markers {
        let Some(BarTarget::Placement(entity)) =
            world.get::<LodBar>(bar).map(|bar| bar.target.clone())
        else {
            continue;
        };
        let left = screen_share(world, entity).map(left_of);
        if let Some(mut node) = world.get_mut::<Node>(marker) {
            let wanted = left.map_or(Display::None, |_| Display::Flex);
            if node.display != wanted {
                node.display = wanted;
            }
            if let Some(left) = left
                && node.left != Val::Percent(left)
            {
                node.left = Val::Percent(left);
            }
        }
    }
}

/// Keep each bar on what its target holds now, as undo, redo and edits made
/// elsewhere change it. A model whose settings were never read is left alone.
fn follow_target(world: &mut World) {
    if world.resource::<BarDrag>().started.is_some() {
        return;
    }
    let mut bars = world.query::<(Entity, &LodBar)>();
    let bars: Vec<(Entity, LodBar)> = bars
        .iter(world)
        .map(|(entity, bar)| (entity, bar.clone()))
        .collect();
    for (entity, bar) in bars {
        if let BarTarget::Model(path) = &bar.target
            && !world.resource::<ModelLodIndex>().is_known(path)
        {
            continue;
        }
        let Some((heights, _)) = current(world, &bar.target) else {
            continue;
        };
        if heights.len() != bar.heights.len() || heights == bar.heights {
            continue;
        }
        if let Some(mut held) = world.get_mut::<LodBar>(entity) {
            held.heights = heights;
        }
        lay_out_bar(world, entity);
    }
}

fn begin_divider_drag(
    start: On<Pointer<DragStart>>,
    dividers: Query<&LodBarDivider>,
    mut commands: Commands,
) {
    let Ok(divider) = dividers.get(start.event_target()).copied() else {
        return;
    };
    commands.queue(move |world: &mut World| start_drag(world, divider.bar));
}

/// Remember what a bar's target holds as a divider drag begins.
pub fn start_drag(world: &mut World, bar: Entity) {
    let Some(target) = world.get::<LodBar>(bar).map(|bar| bar.target.clone()) else {
        return;
    };
    let started = match &target {
        BarTarget::Model(path) => DragStartState::Model {
            lod: model_levels(world, path),
            unsaved: world
                .resource::<crate::model_lod::UnsavedModelSettings>()
                .0
                .contains(path),
        },
        BarTarget::Placement(entity) => {
            DragStartState::Placement(world.get::<LodOverride>(*entity).cloned())
        }
    };
    world.resource_mut::<BarDrag>().started = Some(started);
}

fn drag_divider(
    drag: On<Pointer<Drag>>,
    dividers: Query<&LodBarDivider>,
    computed: Query<&ComputedNode>,
    mut commands: Commands,
) {
    let Ok(divider) = dividers.get(drag.event_target()).copied() else {
        return;
    };
    let Ok(node) = computed.get(divider.bar) else {
        return;
    };
    let width = node.size().x * node.inverse_scale_factor();
    if width <= 0.0 {
        return;
    }
    let moved = drag.delta.x / width;
    commands.queue(move |world: &mut World| move_divider(world, divider, moved));
}

/// Move a divider by `moved` of the bar's width, keeping it between its
/// neighbours, and show the new switch at once.
pub fn move_divider(world: &mut World, divider: LodBarDivider, moved: f32) {
    let Some(mut bar) = world.get::<LodBar>(divider.bar).cloned() else {
        return;
    };
    let level = divider.level;
    let Some(height) = bar.heights.get(level).copied() else {
        return;
    };
    let upper = level
        .checked_sub(1)
        .and_then(|before| bar.heights.get(before))
        .copied()
        .unwrap_or(1.0)
        - MIN_GAP;
    let lower = bar.heights.get(level + 1).copied().unwrap_or(0.0) + MIN_GAP;
    let wanted = (height - moved).clamp(lower.min(upper), upper.max(lower));
    bar.heights[level] = wanted;
    preview(world, &bar.target, &bar.heights);
    if let Some(mut held) = world.get_mut::<LodBar>(divider.bar) {
        held.heights = bar.heights;
    }
    lay_out_bar(world, divider.bar);
}

/// Show `heights` live, without an undo entry.
fn preview(world: &mut World, target: &BarTarget, heights: &[f32]) {
    match target {
        BarTarget::Model(path) => {
            let Some(mut lod) = model_levels(world, path) else {
                return;
            };
            for (level, height) in lod.levels.iter_mut().zip(heights) {
                level.screen_height = *height;
            }
            world.resource_mut::<ModelLodIndex>().set(path, Some(lod));
        }
        BarTarget::Placement(entity) => {
            if let Some(mut levels) = world.get_mut::<ModelLevels>(*entity) {
                for (level, height) in levels.group.levels.iter_mut().zip(heights) {
                    level.screen_height = *height;
                }
            }
        }
    }
}

fn end_divider_drag(
    end: On<Pointer<DragEnd>>,
    dividers: Query<&LodBarDivider>,
    mut commands: Commands,
) {
    let Ok(divider) = dividers.get(end.event_target()).copied() else {
        return;
    };
    commands.queue(move |world: &mut World| finish_drag(world, divider.bar));
}

/// End a divider drag as one undo entry from where it started to where the
/// bar stands now.
pub fn finish_drag(world: &mut World, bar: Entity) {
    let Some(started) = world.resource_mut::<BarDrag>().started.take() else {
        return;
    };
    let Some(LodBar { target, heights }) = world.get::<LodBar>(bar).cloned() else {
        return;
    };
    match (target, started) {
        (BarTarget::Model(path), DragStartState::Model { lod, unsaved }) => {
            let after = model_levels(world, &path);
            if after == lod {
                return;
            }
            world
                .resource_mut::<crate::model_lod::UnsavedModelSettings>()
                .0
                .insert(path.clone());
            world
                .resource_mut::<CommandHistory>()
                .push_executed(Box::new(crate::model_lod::SetModelLod {
                    path,
                    before: lod,
                    after,
                    was_unsaved: unsaved,
                }));
            crate::inspector::file_card::refresh_file_card(world);
        }
        (BarTarget::Placement(entity), DragStartState::Placement(before)) => {
            let mut held = before.clone().unwrap_or_default();
            held.screen_heights = Some(heights);
            let after = Some(held);
            if after == before {
                return;
            }
            let mut command = SetLodOverride {
                entity,
                before,
                after,
            };
            command.execute(world);
            world
                .resource_mut::<CommandHistory>()
                .push_executed(Box::new(command));
        }
        _ => {}
    }
}

fn force_clicked_level(
    click: On<Pointer<Click>>,
    segments: Query<&LodBarSegment>,
    bars: Query<&LodBar>,
    mut forced: ResMut<jackdaw_runtime::ForcedLod>,
) {
    let Ok(segment) = segments.get(click.event_target()) else {
        return;
    };
    let count = bars.get(segment.bar).map_or(0, |bar| bar.heights.len());
    if segment.level >= count {
        return;
    }
    force_level(&mut forced, segment.level);
}

/// Draw every group at `level`, or let the distance choose again when it
/// already is.
pub fn force_level(forced: &mut jackdaw_runtime::ForcedLod, level: usize) {
    forced.0 = if forced.0 == Some(level) {
        None
    } else {
        Some(level)
    };
}

/// Set or clear a placed model's [`LodOverride`], as one undo entry.
pub struct SetLodOverride {
    pub entity: Entity,
    pub before: Option<LodOverride>,
    pub after: Option<LodOverride>,
}

impl SetLodOverride {
    fn put(world: &mut World, entity: Entity, value: Option<&LodOverride>) {
        let entity = crate::scene_nodes::live_entity(world, entity);
        let Ok(mut placed) = world.get_entity_mut(entity) else {
            return;
        };
        match value {
            Some(value) => {
                placed.insert(value.clone());
                crate::commands::sync_component_to_bsn_doc(world, entity, value);
            }
            None => {
                placed.remove::<LodOverride>();
                let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
                if let Some(node) = ast.ast_for(entity) {
                    ast.remove_component_patch(node, LodOverride::type_path());
                }
            }
        }
        world
            .resource_mut::<crate::inspector::PendingInspectorRebuild>()
            .0 = Some(entity);
    }
}

impl EditorCommand for SetLodOverride {
    fn execute(&mut self, world: &mut World) {
        Self::put(world, self.entity, self.after.as_ref());
    }

    fn undo(&mut self, world: &mut World) {
        Self::put(world, self.entity, self.before.as_ref());
    }

    fn description(&self) -> &str {
        "Change LOD override"
    }
}

/// Put back a placed model's screen heights or fade from its model's
/// settings.
#[operator(
    id = "lod.revert_override",
    label = "Revert to Model",
    description = "Put a placed model's screen heights or fade back to its model's settings.",
    allows_undo = false,
    params(
        entity(Entity, doc = "The placed model."),
        field(String, doc = "\"screen_heights\", \"fade\", or \"all\" for both."),
    )
)]
pub(crate) fn lod_revert_override(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = params.as_entity("entity") else {
        return OperatorResult::Cancelled;
    };
    let field = params.as_str("field").unwrap_or("all").to_string();
    commands.queue(move |world: &mut World| {
        let entity = crate::scene_nodes::live_entity(world, entity);
        let before = world.get::<LodOverride>(entity).cloned();
        let Some(mut after) = before.clone() else {
            return;
        };
        match field.as_str() {
            "screen_heights" => after.screen_heights = None,
            "fade" => after.fade = None,
            _ => after = LodOverride::default(),
        }
        let after = (after != LodOverride::default()).then_some(after);
        let mut command = SetLodOverride {
            entity,
            before,
            after,
        };
        command.execute(world);
        world
            .resource_mut::<CommandHistory>()
            .push_executed(Box::new(command));
    });
    OperatorResult::Finished
}

/// Drop a model's unsaved settings, going back to what its meta holds.
#[operator(
    id = "model.lod.revert",
    label = "Revert Import Settings",
    description = "Go back to the import settings a model's .meta holds, dropping edits not yet \
                   written.",
    allows_undo = false,
    params(path(String, doc = "The model, as a path under the project's assets."))
)]
pub(crate) fn model_lod_revert(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(path) = params.as_str("path").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let Some(assets) = world
            .get_resource::<crate::project::ProjectRoot>()
            .map(crate::project::ProjectRoot::assets_dir)
        else {
            return;
        };
        let on_disk = crate::model_lod::levels_on_disk(&assets.join(&path));
        crate::model_lod::edit_model_levels(world, &path, on_disk);
        world
            .resource_mut::<crate::model_lod::UnsavedModelSettings>()
            .0
            .remove(&path);
        crate::inspector::file_card::refresh_file_card(world);
    });
    OperatorResult::Finished
}

/// Set how a model's levels hand over to each other.
#[operator(
    id = "model.lod.set_fade",
    label = "Set LOD Fade",
    description = "Set whether a model's levels snap or cross-fade into each other, and over how \
                   much of each switch distance.",
    allows_undo = false,
    params(
        path(String, doc = "The model, as a path under the project's assets."),
        width(
            f64,
            default = 0.0,
            doc = "Share of each switch distance that cross-fades; 0 snaps."
        ),
    )
)]
pub(crate) fn model_lod_set_fade(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(path) = params.as_str("path").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    let width = params.as_float("width").unwrap_or(0.0) as f32;
    commands.queue(move |world: &mut World| {
        let Some(mut lod) = model_levels(world, &path) else {
            return;
        };
        lod.fade = if width > 0.0 {
            jackdaw_scene_types::LodFade::CrossFade { width }
        } else {
            jackdaw_scene_types::LodFade::Snap
        };
        crate::model_lod::edit_model_levels(world, &path, Some(lod));
    });
    OperatorResult::Finished
}

fn small_text(world: &mut World, parent: Entity, text: String, color: Color) {
    world.spawn((
        Text::new(text),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(color),
        ChildOf(parent),
    ));
}

fn row(world: &mut World, parent: Entity) -> Entity {
    world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                column_gap: Val::Px(tokens::SPACING_SM),
                margin: UiRect::top(Val::Px(tokens::SPACING_XS)),
                ..default()
            },
            ChildOf(parent),
        ))
        .id()
}

fn action(world: &mut World, parent: Entity, label: &str, call: ButtonOperatorCall) {
    let button = world
        .spawn((button(ButtonProps::new(label.to_string())), call))
        .id();
    world.entity_mut(button).insert(ChildOf(parent));
}

/// The LOD Group card of a placed model drawing the levels its model's
/// settings give it: the bar, which fields the placement overrides, and the
/// way to the model's settings.
pub fn spawn_placement_card(world: &mut World, inspector: Entity, entity: Entity) {
    use jackdaw_feathers::panel_card::{PanelCardCollapseState, PanelCardProps, spawn_panel_card};

    let Some(model) = world
        .get::<jackdaw_scene_types::GltfSource>(entity)
        .map(source_path)
    else {
        return;
    };
    let icon_font = world
        .resource::<jackdaw_feathers::icons::IconFont>()
        .0
        .clone();
    let body = {
        let mut state: bevy::ecs::system::SystemState<Commands> =
            bevy::ecs::system::SystemState::new(world);
        let Ok(mut commands) = state.get_mut(world) else {
            return;
        };
        let card = spawn_panel_card(
            &mut commands,
            inspector,
            PanelCardProps::new("LOD Group"),
            &icon_font,
            &PanelCardCollapseState::default(),
        );
        commands
            .entity(card.section)
            .insert(crate::inspector::ComponentDisplay);
        let body = card.body;
        state.apply(world);
        body
    };
    spawn_bar(world, body, BarTarget::Placement(entity));
    let held = world
        .get::<LodOverride>(entity)
        .cloned()
        .unwrap_or_default();
    let fields = [
        (
            "Screen heights",
            "screen_heights",
            held.screen_heights.is_some(),
        ),
        ("Fade", "fade", held.fade.is_some()),
    ];
    for (label, field, overridden) in fields {
        let line = row(world, body);
        let (text, color) = if overridden {
            (
                format!("{label}: set on this placement"),
                tokens::ACCENT_BLUE,
            )
        } else {
            (format!("{label}: from the model"), tokens::TEXT_SECONDARY)
        };
        small_text(world, line, text, color);
        if overridden {
            action(
                world,
                line,
                "Revert to Model",
                ButtonOperatorCall::new(LodRevertOverrideOp::ID)
                    .with_param("entity", entity)
                    .with_param("field", field.to_string()),
            );
        }
    }
    let line = row(world, body);
    action(
        world,
        line,
        "Edit Model Settings",
        ButtonOperatorCall::new(OpenModelSettingsOp::ID).with_param("path", model),
    );
}

/// Show a model's file card, where its import settings are edited.
#[operator(
    id = "model.lod.open_settings",
    label = "Edit Model Settings",
    description = "Show a model's import settings on its file card.",
    allows_undo = false,
    params(path(String, doc = "The model, as a path under the project's assets."))
)]
pub(crate) fn open_model_settings(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(path) = params.as_str("path").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        if let Some(assets) = world
            .get_resource::<crate::project::ProjectRoot>()
            .map(crate::project::ProjectRoot::assets_dir)
        {
            crate::inspector::file_card::show_file(world, &assets.join(&path));
        }
    });
    OperatorResult::Finished
}

/// The bar and fade controls on a model's file card.
pub fn spawn_model_controls(world: &mut World, body: Entity, path: &str) {
    spawn_bar(world, body, BarTarget::Model(path.to_string()));
    let line = row(world, body);
    small_text(world, line, "Fade".to_string(), tokens::TEXT_SECONDARY);
    let buttons = row(world, line);
    action(
        world,
        buttons,
        "Snap",
        ButtonOperatorCall::new(ModelLodSetFadeOp::ID)
            .with_param("path", path.to_string())
            .with_param("width", 0.0),
    );
    action(
        world,
        buttons,
        "Cross-fade",
        ButtonOperatorCall::new(ModelLodSetFadeOp::ID)
            .with_param("path", path.to_string())
            .with_param("width", 0.1),
    );
}

/// Whether the viewport tints each live LOD part in its level's colour.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct LodColorView(pub bool);

/// A part's own material while the LOD colour view tints it.
#[derive(Component)]
pub(crate) struct LodTint(Handle<StandardMaterial>);

/// Tint each level's parts in its colour, or put their own materials back.
#[operator(
    id = "view.toggle_lod_colors",
    label = "Toggle LOD Colors",
    description = "Tint every live level of detail in its LOD bar colour, or show the scene's own \
                   materials again. A view setting: nothing is saved.",
    allows_undo = false
)]
pub(crate) fn view_toggle_lod_colors(
    _: In<OperatorParameters>,
    mut view: ResMut<LodColorView>,
) -> OperatorResult {
    view.0 = !view.0;
    OperatorResult::Finished
}

/// Keep the parts of live levels tinted while the LOD colour view is on.
fn tint_lod_parts(
    view: Res<LodColorView>,
    mut commands: Commands,
    untinted: Query<
        (
            Entity,
            &jackdaw_runtime::LodPartLevel,
            &MeshMaterial3d<StandardMaterial>,
        ),
        (Without<LodTint>, Without<crate::view_modes::ShadedBy>),
    >,
    tinted: Query<(Entity, &LodTint)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut palette: Local<Vec<Handle<StandardMaterial>>>,
) {
    if !view.0 {
        if view.is_changed() {
            for (part, tint) in &tinted {
                commands
                    .entity(part)
                    .try_insert(MeshMaterial3d(tint.0.clone()))
                    .try_remove::<LodTint>();
            }
        }
        return;
    }
    for (part, level, own) in &untinted {
        while palette.len() <= level.0 {
            let color = level_color(palette.len());
            palette.push(materials.add(StandardMaterial {
                base_color: color,
                unlit: true,
                ..default()
            }));
        }
        commands.entity(part).try_insert((
            LodTint(own.0.clone()),
            MeshMaterial3d(palette[level.0].clone()),
        ));
    }
}
