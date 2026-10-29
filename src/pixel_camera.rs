use bevy::{
    camera::{CameraProjection, visibility::RenderLayers},
    feathers::controls::FeathersCheckbox,
    prelude::*,
    ui::Checked,
    ui_widgets::ValueChange,
};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::warn_caller;
use jackdaw_feathers::{
    combobox::{ComboBoxChangeEvent, ComboBoxSelectedIndex, combobox_with_selected},
    field_row::{FieldRowProps, spawn_field_row},
    tokens,
};
use jackdaw_scene_types::{PIXEL_CANVAS_TYPE_PATH, PixelCanvas};

use crate::canvas_snap::CanvasSnap;
use crate::commands::{EditorCommand, FieldWritten, push_executed_edits};
use crate::sprite_stage::{Scene2dRoots, shows_2d_world};
use crate::viewport::ReservedViewportLayer;
use crate::viewport_2d::Viewport2dPanelHost;

/// The base resolutions the inspector offers, smallest first.
pub const CANVAS_PRESETS: [UVec2; 4] = [
    UVec2::new(320, 180),
    UVec2::new(384, 216),
    UVec2::new(480, 270),
    UVec2::new(640, 360),
];

/// Combo index of the Custom option, after the presets.
pub const CUSTOM_PRESET: usize = CANVAS_PRESETS.len();

/// Height of the game-frame label, in the panel's target pixels.
const LABEL_SIZE: f32 = 11.0;

/// The game frames drawn into one 2D panel, on that panel's own render layer.
#[derive(Component)]
pub struct GameFrameOverlay {
    pub host: Entity,
}

/// The resolution preset combo of the card showing `source`.
#[derive(Component)]
pub struct CanvasPresetCombo {
    pub source: Entity,
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        PostUpdate,
        draw_game_frames.after(bevy::transform::TransformSystems::Propagate),
    )
    .add_observer(on_preset_pick)
    .add_observer(follow_canvas_write)
    .add_observer(on_canvas_toggle)
    .add_observer(on_canvas_added)
    .add_observer(on_canvas_removed);
}

/// The corners of the world area a camera shows through `canvas`, counter
/// clockwise from the bottom left: its projection laid over the canvas's base
/// resolution and placed by `transform`, as the running game draws it.
/// `None` while the resolution is unset.
pub fn game_frame(
    canvas: &PixelCanvas,
    projection: &OrthographicProjection,
    transform: &GlobalTransform,
) -> Option<[Vec2; 4]> {
    if !canvas.is_set() {
        return None;
    }
    let mut projection = projection.clone();
    let size = canvas.resolution.as_vec2();
    projection.update(size.x, size.y);
    let Rect { min, max } = projection.area;
    Some(
        [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)]
            .map(|corner| transform.transform_point(corner.extend(0.0)).truncate()),
    )
}

/// Outline every canvas camera of the open 2D scene, labelled with its
/// resolution, in each 2D panel showing the world: one retained gizmo per
/// panel, on the layer only that panel's camera draws, its label sized to the
/// panel's zoom.
fn draw_game_frames(
    snap: Res<CanvasSnap>,
    hosts: Query<(Entity, &Viewport2dPanelHost)>,
    layers: Query<&ReservedViewportLayer>,
    ui_roots: Query<&UiTargetCamera, crate::prefab::AuthoredUiSceneRoot>,
    scene_roots: Scene2dRoots,
    cameras: Query<
        (Entity, &PixelCanvas, &Projection, &GlobalTransform),
        (With<Camera2d>, Without<crate::EditorEntity>),
    >,
    parents: Query<&ChildOf>,
    overlays: Query<(Entity, &GameFrameOverlay, &Gizmo)>,
    mut assets: ResMut<Assets<GizmoAsset>>,
    mut commands: Commands,
) -> Result {
    let frames: Vec<([Vec2; 4], UVec2)> = cameras
        .iter()
        .filter(|_| snap.show_game_frame)
        .filter(|(camera, ..)| {
            parents
                .iter_ancestors(*camera)
                .any(|ancestor| scene_roots.contains(ancestor))
        })
        .filter_map(|(_, canvas, projection, transform)| match projection {
            Projection::Orthographic(ortho) => {
                game_frame(canvas, ortho, transform).map(|corners| (corners, canvas.resolution))
            }
            _ => None,
        })
        .collect();
    for (overlay, owner, _) in &overlays {
        let shown = hosts.get(owner.host).is_ok_and(|(_, host)| {
            !frames.is_empty() && shows_2d_world(host, ui_roots.iter(), &scene_roots)
        });
        if !shown {
            commands.entity(overlay).despawn();
        }
    }
    if frames.is_empty() {
        return Ok(());
    }
    for (entity, host) in &hosts {
        let Ok(layer) = layers.get(host.camera) else {
            continue;
        };
        if !shows_2d_world(host, ui_roots.iter(), &scene_roots) {
            continue;
        }
        let mut gizmo = GizmoAsset::default();
        let size = LABEL_SIZE / host.view.zoom;
        for (corners, resolution) in &frames {
            gizmo.lineloop_2d(*corners, tokens::GUIDE_LINE);
            gizmo.text_2d(
                Isometry2d::from_translation(corners[3] + Vec2::new(0.0, size * 0.5)),
                &format!("{} x {}", resolution.x, resolution.y),
                size,
                Vec2::new(-0.5, -0.5),
                tokens::GUIDE_LINE,
            );
        }
        match overlays.iter().find(|(_, owner, _)| owner.host == entity) {
            Some((_, _, drawn)) => assets.insert(&drawn.handle, gizmo)?,
            None => {
                commands.spawn((
                    GameFrameOverlay { host: entity },
                    crate::EditorEntity,
                    Gizmo {
                        handle: assets.add(gizmo),
                        ..default()
                    },
                    RenderLayers::layer(layer.get_layer()),
                ));
            }
        }
    }
    Ok(())
}

/// Give a 2D camera a pixel canvas, change its settings, or take it off, as
/// one history entry. A side left out keeps its size; a new canvas starts at
/// the first preset.
#[operator(
    id = "camera.pixel_canvas",
    label = "Pixel Canvas",
    description = "Draw a 2D camera at a fixed low resolution scaled up to its window, for pixel art.",
    allows_undo = false,
    params(
        entity(
            Entity,
            doc = "2D camera to set. Left out, the selected 2D camera, else the open 2D scene's only one."
        ),
        width(i64, doc = "Base width in pixels, above 0."),
        height(i64, doc = "Base height in pixels, above 0."),
        scale(String, doc = "integer, fit or stretch."),
        letterbox(String, doc = "Colour of the bars: hex, or r,g,b[,a] in 0..1."),
        pixel_snap(bool, doc = "Draw the camera at whole canvas pixels."),
        remove(bool, doc = "Take the canvas off the camera."),
    )
)]
pub(crate) fn camera_pixel_canvas(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let id = CameraPixelCanvasOp::ID;
    let refuse = |world: &mut World, why: String| {
        warn_caller(world, format!("{id}: {why}"));
        OperatorResult::Cancelled
    };
    let camera = match target_camera(world, &params) {
        Ok(camera) => camera,
        Err(why) => return refuse(world, why),
    };
    if crate::preview_context::preview_writes_type_path(world, camera, PIXEL_CANVAS_TYPE_PATH) {
        return refuse(world, format!("the running preview drives {camera}"));
    }
    let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
    if ast.ast_for(camera).is_none() && !crate::sprite_props::goes_to_the_game(world, camera) {
        return refuse(world, format!("{camera} is not in the scene document"));
    }
    let current = world.get::<PixelCanvas>(camera).copied();
    let path = PIXEL_CANVAS_TYPE_PATH.to_owned();
    if params.as_bool("remove") == Some(true) {
        if current.is_none() {
            return refuse(world, format!("{camera} has no pixel canvas"));
        }
        let edit = crate::inspector::ops::remove_component_edit(world, camera, path);
        push_executed_edits(
            world,
            edit.into_iter().collect(),
            Vec::new(),
            "Remove Pixel Canvas",
        );
        return OperatorResult::Finished;
    }
    let writes = match canvas_writes(&params, current) {
        Ok(writes) => writes,
        Err(why) => return refuse(world, why),
    };
    let mut edits: Vec<Box<dyn EditorCommand>> = Vec::new();
    if current.is_none() {
        edits.extend(crate::inspector::ops::add_component_edit(
            world, camera, path,
        ));
    }
    let writes = writes
        .into_iter()
        .map(|(field, json)| (camera, field, json))
        .collect();
    let Some(mut fields) =
        crate::sprite_props::field_edits(world, id, PIXEL_CANVAS_TYPE_PATH, writes)
    else {
        for edit in edits.iter_mut().rev() {
            edit.undo(world);
        }
        return OperatorResult::Cancelled;
    };
    if current.is_none() {
        fields.extend(zoom_edit(world, id, camera));
    }
    push_executed_edits(world, edits, fields, "Set Pixel Canvas");
    OperatorResult::Finished
}

const PROJECTION_TYPE_PATH: &str = "bevy_camera::projection::Projection";

/// The orthographic scale a new 2D camera or a new canvas takes. The game
/// does not know the project's pixels per unit, so a camera at the default
/// scale of 1 zooms to `1 / ppu`, one sprite pixel to one canvas pixel.
/// `None` at a ppu of 1, or when the scale is anything else: its author chose
/// it.
fn canvas_zoom(world: &World, camera: Entity) -> Option<f32> {
    let ppu = world
        .get_resource::<crate::world_units::WorldUnits>()?
        .ppu();
    let Some(Projection::Orthographic(ortho)) = world.get::<Projection>(camera) else {
        return None;
    };
    (ppu != 1.0 && ortho.scale == 1.0).then_some(1.0 / ppu)
}

/// Zoom a 2D camera the Add menu just spawned by the [`canvas_zoom`] rule,
/// before the document records it, so Play frames it as the editor does.
pub(crate) fn zoom_new_camera(world: &mut World, camera: Entity) {
    if let Some(scale) = canvas_zoom(world, camera)
        && let Some(mut projection) = world.get_mut::<Projection>(camera)
        && let Projection::Orthographic(ortho) = projection.as_mut()
    {
        ortho.scale = scale;
    }
}

/// The edit that zooms `camera` as its canvas comes, if
/// [`canvas_zoom`] calls for one. The zoom is a convenience, so one that
/// cannot be authored leaves the canvas edit to land alone and tells the
/// caller.
fn zoom_edit(world: &mut World, id: &str, camera: Entity) -> Option<crate::commands::SetBsnField> {
    let scale = canvas_zoom(world, camera)?;
    world
        .resource::<jackdaw_bsn::SceneBsnAst>()
        .ast_for(camera)?;
    let json = serde_json::json!(scale);
    let edit = crate::commands::field_edit_to_bsn_value(
        world,
        camera,
        PROJECTION_TYPE_PATH,
        "0.scale",
        &json,
    )
    .ok()
    .and_then(|(path, value)| {
        crate::commands::set_bsn_field_command(world, camera, PROJECTION_TYPE_PATH, &path, value)
            .ok()
    });
    if edit.is_none() {
        warn_caller(
            world,
            format!("{id}: could not set the camera's projection scale to {scale}"),
        );
    }
    edit
}

/// The camera `entity=` names, else the selected 2D camera, else the open 2D
/// scene's only one.
fn target_camera(world: &mut World, params: &OperatorParameters) -> Result<Entity, String> {
    let is_camera = |world: &World, entity: Entity| {
        world.get_entity(entity).is_ok_and(|held| {
            held.contains::<Camera2d>() && !held.contains::<crate::EditorEntity>()
        })
    };
    if let Some(entity) = params.as_entity("entity") {
        return if is_camera(world, entity) {
            Ok(entity)
        } else {
            Err(format!("{entity} is not a 2D camera"))
        };
    }
    let primary = world
        .get_resource::<crate::selection::Selection>()
        .and_then(crate::selection::Selection::primary);
    if let Some(primary) = primary.filter(|&primary| is_camera(world, primary)) {
        return Ok(primary);
    }
    let cameras: Vec<Entity> = world
        .query_filtered::<Entity, (With<Camera2d>, Without<crate::EditorEntity>)>()
        .iter(world)
        .filter(|&camera| crate::sprite_gizmo::in_open_2d_scene(world, camera))
        .collect();
    match cameras.as_slice() {
        [camera] => Ok(*camera),
        [] => Err("the open 2D scene has no 2D camera".to_owned()),
        _ => Err("select one of the scene's 2D cameras".to_owned()),
    }
}

/// The field writes the parameters ask for. A new or unset canvas starts
/// from the first preset.
fn canvas_writes(
    params: &OperatorParameters,
    current: Option<PixelCanvas>,
) -> Result<Vec<(&'static str, serde_json::Value)>, String> {
    let base = current
        .filter(PixelCanvas::is_set)
        .map_or(CANVAS_PRESETS[0], |canvas| canvas.resolution);
    let side = |key: &str, fallback: u32| match params.get(key) {
        None => Ok(fallback),
        Some(_) => params
            .as_int(key)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or(format!("{key} is a whole number above 0")),
    };
    let resolution = UVec2::new(side("width", base.x)?, side("height", base.y)?);
    let mut writes = Vec::new();
    if current.is_none_or(|canvas| canvas.resolution != resolution) {
        writes.push((
            "resolution",
            serde_json::json!([resolution.x, resolution.y]),
        ));
    }
    if let Some(scale) = params.as_str("scale") {
        let variant = ["Integer", "Fit", "Stretch"]
            .into_iter()
            .find(|variant| variant.eq_ignore_ascii_case(scale.trim()))
            .ok_or(format!("no scale {scale}; integer, fit or stretch"))?;
        writes.push(("scale", serde_json::json!(variant)));
    }
    if let Some(letterbox) = params.as_str("letterbox") {
        let colour =
            parse_colour(letterbox).ok_or(format!("letterbox {letterbox} is not hex or r,g,b"))?;
        writes.push((
            "letterbox",
            serde_json::json!({"Srgba": {
                "red": colour.red, "green": colour.green, "blue": colour.blue, "alpha": colour.alpha
            }}),
        ));
    }
    if let Some(snap) = params.as_bool("pixel_snap") {
        writes.push(("pixel_snap", serde_json::json!(snap)));
    }
    Ok(writes)
}

/// `#rrggbb` style hex, or three or four comma separated channels in 0..=1.
fn parse_colour(text: &str) -> Option<Srgba> {
    if let Ok(colour) = Srgba::hex(text.trim()) {
        return Some(colour);
    }
    let channels: Vec<f32> = text
        .split(',')
        .map(|channel| {
            let value = channel.trim().parse::<f32>().ok()?;
            (0.0..=1.0).contains(&value).then_some(value)
        })
        .collect::<Option<_>>()?;
    match channels.as_slice() {
        [red, green, blue] => Some(Srgba::new(*red, *green, *blue, 1.0)),
        [red, green, blue, alpha] => Some(Srgba::new(*red, *green, *blue, *alpha)),
        _ => None,
    }
}

/// The combo index showing `resolution`: its preset, or Custom.
pub fn preset_index(resolution: UVec2) -> usize {
    CANVAS_PRESETS
        .iter()
        .position(|preset| *preset == resolution)
        .unwrap_or(CUSTOM_PRESET)
}

/// The 2D camera card's pixel canvas on/off checkbox for `source`.
#[derive(Component)]
pub struct PixelCanvasToggle {
    pub source: Entity,
}

/// The "Pixel canvas" row of a 2D camera's card, checked while `source`
/// carries a canvas.
pub(crate) fn spawn_canvas_toggle(commands: &mut Commands, body: Entity, source: Entity, on: bool) {
    let row = spawn_field_row(commands, body, FieldRowProps::new("Pixel canvas"));
    let mut checkbox = commands.spawn_scene(bsn! { @FeathersCheckbox });
    checkbox.insert((PixelCanvasToggle { source }, ChildOf(row.control)));
    if on {
        checkbox.insert(Checked);
    }
}

/// Add or remove the canvas through `camera.pixel_canvas` when the request
/// differs from what the camera holds. The checkbox follows the component.
fn on_canvas_toggle(
    change: On<ValueChange<bool>>,
    toggles: Query<&PixelCanvasToggle>,
    canvases: Query<(), With<PixelCanvas>>,
    mut commands: Commands,
) {
    let Ok(toggle) = toggles.get(change.source) else {
        return;
    };
    if canvases.contains(toggle.source) == change.value {
        return;
    }
    let mut call = commands
        .operator(CameraPixelCanvasOp::ID)
        .param("entity", toggle.source);
    if !change.value {
        call = call.param("remove", true);
    }
    call.call();
}

fn check_canvas_toggles(
    commands: &mut Commands,
    toggles: &Query<(Entity, &PixelCanvasToggle)>,
    source: Entity,
    on: bool,
) {
    for (checkbox, _) in toggles.iter().filter(|(_, toggle)| toggle.source == source) {
        jackdaw_feathers::utils::set_marker_if_alive::<Checked>(commands, checkbox, on);
    }
}

fn on_canvas_added(
    add: On<Add<PixelCanvas>>,
    toggles: Query<(Entity, &PixelCanvasToggle)>,
    mut commands: Commands,
) {
    check_canvas_toggles(&mut commands, &toggles, add.entity, true);
}

fn on_canvas_removed(
    remove: On<Remove<PixelCanvas>>,
    toggles: Query<(Entity, &PixelCanvasToggle)>,
    mut commands: Commands,
) {
    check_canvas_toggles(&mut commands, &toggles, remove.entity, false);
}

/// The resolution preset row heading the `PixelCanvas` card; the reflected
/// fields below it are the custom size and the other settings.
pub(crate) fn spawn_preset_row(
    commands: &mut Commands,
    body: Entity,
    source: Entity,
    resolution: UVec2,
) {
    let row = spawn_field_row(commands, body, FieldRowProps::new("preset"));
    let options: Vec<String> = CANVAS_PRESETS
        .iter()
        .map(|preset| format!("{} x {}", preset.x, preset.y))
        .chain(["Custom".to_owned()])
        .collect();
    commands.spawn((
        combobox_with_selected(options, preset_index(resolution)),
        CanvasPresetCombo { source },
        ChildOf(row.control),
    ));
}

/// A preset sets the resolution through `camera.pixel_canvas`. Custom only
/// names a size off the presets, so whatever the pick, the combo then shows
/// what the canvas holds.
fn on_preset_pick(
    pick: On<ComboBoxChangeEvent>,
    combos: Query<&CanvasPresetCombo>,
    mut commands: Commands,
) {
    let Ok(combo) = combos.get(pick.entity) else {
        return;
    };
    let source = combo.source;
    if let Some(preset) = CANVAS_PRESETS.get(pick.selected) {
        commands
            .operator(CameraPixelCanvasOp::ID)
            .param("entity", source)
            .param("width", i64::from(preset.x))
            .param("height", i64::from(preset.y))
            .call();
    }
    commands.queue(move |world: &mut World| show_canvas_preset(world, source));
}

/// Show the preset any write left on the canvas: an undo, a typed size.
fn follow_canvas_write(written: On<FieldWritten>, mut commands: Commands) {
    if written.type_path == PIXEL_CANVAS_TYPE_PATH {
        let source = written.entity;
        commands.queue(move |world: &mut World| show_canvas_preset(world, source));
    }
}

/// Point the preset combos of `source`'s card at its canvas's resolution.
fn show_canvas_preset(world: &mut World, source: Entity) {
    let Some(canvas) = world.get::<PixelCanvas>(source) else {
        return;
    };
    let index = preset_index(canvas.resolution);
    let combos: Vec<Entity> = world
        .query::<(Entity, &CanvasPresetCombo)>()
        .iter(world)
        .filter(|(_, combo)| combo.source == source)
        .map(|(entity, _)| entity)
        .collect();
    for combo in combos {
        world.entity_mut(combo).insert(ComboBoxSelectedIndex(index));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(scale: f32, at: Vec3, resolution: UVec2) -> [Vec2; 4] {
        let projection = OrthographicProjection {
            scale,
            ..OrthographicProjection::default_2d()
        };
        let canvas = PixelCanvas::new(resolution.x, resolution.y);
        game_frame(
            &canvas,
            &projection,
            &GlobalTransform::from(Transform::from_translation(at)),
        )
        .expect("a set canvas has a frame")
    }

    #[test]
    fn the_frame_is_the_base_resolution_around_the_camera_at_any_zoom() {
        for (scale, at) in [
            (1.0, Vec3::ZERO),
            (0.5, Vec3::new(10.4, -3.6, 0.0)),
            (2.0, Vec3::new(-200.0, 75.0, 5.0)),
        ] {
            let corners = frame(scale, at, UVec2::new(320, 180));
            let half = Vec2::new(160.0, 90.0) * scale;
            let centre = at.truncate();
            assert_eq!(corners[0], centre - half, "scale {scale} at {at}");
            assert_eq!(corners[2], centre + half, "scale {scale} at {at}");
            assert_eq!(corners[1], Vec2::new(centre.x + half.x, centre.y - half.y));
            assert_eq!(corners[3], Vec2::new(centre.x - half.x, centre.y + half.y));
        }
    }

    #[test]
    fn the_frame_follows_a_fixed_projection_and_a_rotated_camera() {
        let projection = OrthographicProjection {
            scaling_mode: bevy::camera::ScalingMode::FixedVertical {
                viewport_height: 10.0,
            },
            ..OrthographicProjection::default_2d()
        };
        let canvas = PixelCanvas::new(320, 180);
        let quarter = Transform::from_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
        let corners =
            game_frame(&canvas, &projection, &GlobalTransform::from(quarter)).expect("a frame");
        let width = 10.0 * 320.0 / 180.0;
        assert!((corners[0] - Vec2::new(5.0, -width / 2.0)).length() < 1e-4);
        assert!((corners[2] - Vec2::new(-5.0, width / 2.0)).length() < 1e-4);
    }

    #[test]
    fn an_unset_canvas_has_no_frame() {
        assert!(
            game_frame(
                &PixelCanvas::default(),
                &OrthographicProjection::default_2d(),
                &GlobalTransform::IDENTITY,
            )
            .is_none()
        );
    }

    #[test]
    fn a_letterbox_reads_as_hex_or_channels() {
        assert_eq!(parse_colour("#ff0000"), Some(Srgba::RED));
        assert_eq!(parse_colour("0, 0, 1"), Some(Srgba::BLUE));
        assert_eq!(
            parse_colour("0,0,0,0.5"),
            Some(Srgba::new(0.0, 0.0, 0.0, 0.5))
        );
        assert_eq!(parse_colour("blue"), None);
        assert_eq!(parse_colour("32,32,32"), None, "channels are 0..=1");
        assert_eq!(parse_colour("0.5,-0.1,0.5"), None);
        assert_eq!(parse_colour("0.5,0.5,0.5,1.5"), None);
    }
}
