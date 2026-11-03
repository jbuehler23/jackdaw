//! Adding sprites to the open 2D world scene. The Add menu's Sprite row, an
//! image dropped on the stage and a remote caller all reach `sprite.add`, so
//! the three make the same entity.

use std::path::Path;

use bevy::{
    ecs::system::{SystemParam, SystemState},
    math::BVec2,
    prelude::*,
    sprite::Anchor,
};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::{
    lifecycle::ActiveModalQuery,
    operator::{CallOperatorSettings, ExecutionContext, warn_caller},
};
use jackdaw_scene_types::{CanvasGuides, Scene2dRoot};

use crate::{
    commands::{CommandHistory, EditorCommand, SpawnEntity},
    entity_ops::{claim_free_name, scene_entity_names, to_asset_path},
    sprite_snap::{SpriteSnapRules, SpriteSnapTargets, land},
    sprite_stage::{Scene2dRoots, in_2d_scene},
};

/// Name of a sprite with no image to name it after.
const DEFAULT_NAME: &str = "Sprite";

/// Side, in world units, of the square a sprite with no image shows.
const PLACEHOLDER_SIZE: f32 = 32.0;

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(flag_menu_on_scene_2d_add)
        .add_observer(flag_menu_on_scene_2d_remove);
}

/// Add a sprite to the open 2D world scene, as the Add menu's Sprite row and
/// an image drop on the stage do.
///
/// A new sprite's z is one above the highest sprite already in the scene, or 0
/// in an empty scene, so it draws over whatever it was dropped on.
///
/// At a project pixels per unit other than 1, an image sprite is sized to its
/// image's pixels over the ppu, as `sprite.set_pixels_per_unit` sizes one.
#[operator(
    id = "sprite.add",
    label = "Sprite",
    description = "Add a sprite to the open 2D world scene.",
    allows_undo = false,
    unavailable_reason = sprite_needs_2d_scene,
    is_available = can_add_sprite,
    params(
        path(
            String,
            doc = "Image file, absolute or relative to the assets folder. A plain square when omitted."
        ),
        x(
            f64,
            doc = "World x of the sprite's centre. Left out, the centre of the 2D view in front, or 0 with none."
        ),
        y(
            f64,
            doc = "World y (up) of the sprite's centre. Left out, the centre of the 2D view in front, or 0 with none."
        ),
        parent(
            Entity,
            doc = "Node of the 2D scene that adopts it. Left out, the sprite goes beside a selection inside the scene, else under the scene root."
        ),
        name(
            String,
            doc = "Name for the sprite. Defaults to the image's file stem, or \"Sprite\"."
        ),
    )
)]
pub(crate) fn sprite_add(
    params: In<OperatorParameters>,
    fronted: crate::viewport_2d::FrontedViewport,
    mut commands: Commands,
) -> OperatorResult {
    let path = params
        .as_str("path")
        .filter(|path| !path.is_empty())
        .map(to_asset_path);
    let name = params
        .as_str("name")
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| default_name(path.as_deref()));
    let centre = fronted
        .two_d_host()
        .map_or(Vec2::ZERO, |host| host.view.pan);
    let at = Vec2::new(
        params.as_float("x").map_or(centre.x, |x| x as f32),
        params.as_float("y").map_or(centre.y, |y| y as f32),
    );
    let parent = params.as_entity("parent");
    commands.queue(move |world: &mut World| add_sprite(world, path, name, at, parent));
    OperatorResult::Finished
}

/// Whether `sprite.add` may run: a 2D world scene is open and everything
/// `can_act_on_entities` asks holds. A `SystemParam` so the stage's image drop
/// asks the same question before it claims a drag.
#[derive(SystemParam)]
pub(crate) struct SpriteAddGate<'w, 's> {
    scene_roots: Scene2dRoots<'w, 's>,
    keybind_focus: crate::keybind_focus::KeybindFocus<'w, 's>,
    active: ActiveModalQuery<'w, 's>,
    modal: Res<'w, crate::modal_transform::ModalTransformState>,
    draw_state: Res<'w, crate::draw_brush::DrawBrushState>,
    edit_mode: Res<'w, crate::brush::EditMode>,
    panel_focus: crate::panel_focus::PanelFocus<'w, 's>,
}

impl<'w, 's> SpriteAddGate<'w, 's> {
    pub(crate) fn scene_roots(&self) -> &Scene2dRoots<'w, 's> {
        &self.scene_roots
    }

    pub(crate) fn allows(self) -> bool {
        !self.scene_roots.is_empty()
            && crate::entity_ops::can_act_on_entities(
                self.keybind_focus,
                self.active,
                self.modal,
                self.draw_state,
                self.edit_mode,
                self.panel_focus,
            )
    }
}

fn can_add_sprite(gate: SpriteAddGate) -> bool {
    gate.allows()
}

fn open_2d_scene_exists(world: &World) -> bool {
    let Some(root) = world.component_id::<Scene2dRoot>() else {
        return false;
    };
    let child = world.component_id::<ChildOf>();
    world.archetypes().iter().any(|archetype| {
        !archetype.is_empty()
            && archetype.contains(root)
            && !child.is_some_and(|child| archetype.contains(child))
    })
}

pub(crate) fn needs_2d_scene(world: &World, what: &str) -> Option<String> {
    (!open_2d_scene_exists(world)).then(|| format!("Open a 2D scene to add {what}."))
}

pub(crate) fn sprite_needs_2d_scene(world: &World) -> Option<String> {
    needs_2d_scene(world, "sprites")
}

pub(crate) fn camera_needs_2d_scene(world: &World) -> Option<String> {
    needs_2d_scene(world, "a 2D camera")
}

pub(crate) fn empty_needs_2d_scene(world: &World) -> Option<String> {
    needs_2d_scene(world, "an empty 2D entity")
}

fn default_name(path: Option<&str>) -> String {
    path.and_then(|path| Path::new(path).file_stem())
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| DEFAULT_NAME.to_owned())
}

fn add_sprite(
    world: &mut World,
    path: Option<String>,
    name: String,
    at: Vec2,
    explicit: Option<Entity>,
) {
    let mut state = SystemState::<OpenScene2d>::new(world);
    let placed = state.get(world).ok().and_then(|scene| {
        let parent = scene.parent(explicit)?;
        Some((parent, scene.top_z(parent)))
    });
    let Some((parent, z)) = placed else {
        warn_caller(world, "sprite.add: no 2D world scene is open");
        return;
    };
    if let Some(explicit) = explicit.filter(|&explicit| explicit != parent) {
        warn_caller(
            world,
            format!("sprite.add: {explicit} is not in the open 2D scene; added under {parent}"),
        );
    }
    let name = claim_free_name(&mut scene_entity_names(world), &name).unwrap_or(name);
    let label = format!("Add Sprite {name}");
    let at = at.extend(z);
    let mut spawn = SpawnEntity::new(
        move |world: &mut World| {
            let parent = crate::scene_nodes::live_entity(world, parent);
            spawn_sprite(world, path.as_deref(), &name, at, parent)
        },
        label,
    );
    spawn.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(spawn));
}

fn spawn_sprite(
    world: &mut World,
    path: Option<&str>,
    name: &str,
    at: Vec3,
    parent: Entity,
) -> Entity {
    let sprite = match path {
        Some(path) => {
            let mut sprite =
                Sprite::from_image(world.resource::<AssetServer>().load(path.to_owned()));
            sprite.custom_size = crate::sprite_props::project_size(world, &sprite);
            sprite
        }
        None => Sprite::from_color(Color::WHITE, Vec2::splat(PLACEHOLDER_SIZE)),
    };
    let entity = world
        .spawn((
            Name::new(name.to_owned()),
            sprite,
            Anchor::default(),
            Transform::from_translation(at),
            Visibility::default(),
            ChildOf(parent),
        ))
        .id();
    crate::scene_io::register_entity_in_ast(world, entity);
    crate::selection::select_only(world, entity);
    entity
}

/// The open 2D world scene, read for placing a new entity in it. Its root is
/// the lowest open root, the one the canvas guides take.
#[derive(SystemParam)]
struct OpenScene2d<'w, 's> {
    parents: Query<'w, 's, &'static ChildOf>,
    roots: Scene2dRoots<'w, 's>,
    selection: Option<Res<'w, crate::selection::Selection>>,
    sprites: Query<'w, 's, Entity, (With<Sprite>, Without<crate::EditorEntity>)>,
    transforms: Query<'w, 's, &'static Transform>,
}

impl OpenScene2d<'_, '_> {
    fn contains(&self, entity: Entity) -> bool {
        self.roots.contains(entity) || in_2d_scene(entity, &self.parents, &self.roots)
    }

    /// `explicit` when it is in the scene, else beside the selection when that
    /// is inside the scene, else the root.
    fn parent(&self, explicit: Option<Entity>) -> Option<Entity> {
        let root = self.roots.iter().min()?;
        if let Some(explicit) = explicit.filter(|&explicit| self.contains(explicit)) {
            return Some(explicit);
        }
        let beside = self
            .selection
            .as_deref()
            .and_then(crate::selection::Selection::primary)
            .filter(|&primary| !self.roots.contains(primary) && self.contains(primary))
            .and_then(|primary| self.parents.get(primary).ok())
            .map(ChildOf::parent);
        Some(beside.unwrap_or(root))
    }

    /// The sum of the local z of `entity` and its ancestors: its depth in the
    /// scene while nothing on the way rotates or scales it out of the plane.
    fn depth(&self, entity: Entity) -> f32 {
        std::iter::once(entity)
            .chain(self.parents.iter_ancestors(entity))
            .filter_map(|entity| self.transforms.get(entity).ok())
            .map(|transform| transform.translation.z)
            .sum()
    }

    /// The local z under `parent` that puts a new sprite one above the
    /// scene's highest sprite, or 0 when it has none.
    fn top_z(&self, parent: Entity) -> f32 {
        self.sprites
            .iter()
            .filter(|&sprite| self.contains(sprite))
            .map(|sprite| self.depth(sprite))
            .reduce(f32::max)
            .map_or(0.0, |top| top + 1.0 - self.depth(parent))
    }
}

/// Where a new entity of the open 2D world scene goes: `explicit` when it is
/// in the scene, else beside the selection when that is inside the scene,
/// else under the scene root. `None` when no 2D world scene is open.
pub(crate) fn scene_2d_parent(world: &mut World, explicit: Option<Entity>) -> Option<Entity> {
    let mut state = SystemState::<OpenScene2d>::new(world);
    state.get(world).ok()?.parent(explicit)
}

/// `point` landed by the canvas snap `rules`: grid, pixel and `guides`. A
/// drop has no size yet, so the other sprites' edges are not offered.
pub(crate) fn snap_point(
    point: Vec2,
    rules: &SpriteSnapRules,
    guides: Option<&CanvasGuides>,
) -> Vec2 {
    let mut targets = SpriteSnapTargets::default();
    if let Some(guides) = guides {
        targets.add_guides(guides);
    }
    let moving = Rect::from_corners(point, point);
    point + land(moving, BVec2::TRUE, &targets, &rules.without_edges()).nudge
}
/// Dispatch `sprite.add` for an image dropped at `point` of the 2D world, as
/// a gesture: one history entry.
pub(crate) fn dispatch_drop(commands: &mut Commands, path: String, point: Vec2) {
    commands
        .operator(SpriteAddOp::ID)
        .param("path", path)
        .param("x", f64::from(point.x))
        .param("y", f64::from(point.y))
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .call();
}

fn flag_menu_on_scene_2d_add(_: On<Add<Scene2dRoot>>, mut dirty: ResMut<crate::MenuBarDirty>) {
    dirty.0 = true;
}

fn flag_menu_on_scene_2d_remove(
    _: On<Remove<Scene2dRoot>>,
    mut dirty: ResMut<crate::MenuBarDirty>,
) {
    dirty.0 = true;
}
