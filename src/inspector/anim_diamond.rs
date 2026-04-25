//! Per-property "animate" diamond on inspector field rows.
//!
//! Adds a diamond button next to animatable fields (see
//! `ANIMATABLE_FIELDS`). Clicking it finds-or-creates a clip + track
//! and spawns a keyframe at the cursor time. New animatable properties
//! need one entry in `ANIMATABLE_FIELDS` plus matching arms in
//! `spawn_typed_keyframe` and `compile::build_curve_for_track`.

use bevy::prelude::*;
use jackdaw_animation::{
    AnimationTrack, Clip, F32Keyframe, QuatKeyframe, SelectedClip, TimelineCursor, TimelineDirty,
    Vec3Keyframe,
};
use jackdaw_feathers::button::{ButtonClickEvent, ButtonProps, ButtonSize, ButtonVariant, button};
use jackdaw_feathers::icons::Icon;

use super::InspectorFieldRow;
use crate::prelude::*;

/// Epsilon for "is the cursor on this keyframe?" (~1 frame at 60fps).
const CURSOR_ON_KEYFRAME_EPS: f32 = 0.02;

const TRANSFORM: &str = "bevy_transform::components::transform::Transform";

/// The `(component_type_path, field_path)` pairs that get a keyframe
/// diamond in the inspector. Keep in sync with the compile dispatch
/// in `jackdaw_animation::compile::build_curve_for_track` and with
/// [`spawn_typed_keyframe`] below.
const ANIMATABLE_FIELDS: &[(&str, &str)] = &[
    (TRANSFORM, "translation"),
    (TRANSFORM, "rotation"),
    (TRANSFORM, "scale"),
];

/// Marker on the diamond button. The click observer reads this to
/// know which source entity + property to keyframe.
#[derive(Component, Clone, Debug)]
pub struct AnimDiamondButton {
    pub source_entity: Entity,
    pub component_type_path: String,
    pub field_path: String,
}

/// True if the given `(component_type_path, field_path)` is in the
/// animatable allowlist.
fn is_animatable(component_type_path: &str, field_path: &str) -> bool {
    ANIMATABLE_FIELDS
        .iter()
        .any(|(t, f)| *t == component_type_path && *f == field_path)
}

/// Spawn a diamond button on every newly-added `InspectorFieldRow`
/// whose root property is animatable. Runs in `Update` and fires only
/// when rows are (re-)spawned, so it's cheap.
///
/// The `InspectorFieldRow` marker sits on the row's **outer column
/// container**, which `reflect_fields.rs` spawns with `position_type:
/// Relative` specifically so absolutely-positioned children land in
/// the row's coordinate space. That lets us tuck the diamond into
/// the top-right corner next to the field label without reflowing
/// the column's flex layout, and gives us exactly one diamond per
/// composite field (not one per scalar axis input inside it).
pub fn decorate_animatable_fields(
    new_rows: Query<(Entity, &InspectorFieldRow), Added<InspectorFieldRow>>,
    mut commands: Commands,
) {
    for (row_entity, row) in &new_rows {
        if !is_animatable(&row.type_path, &row.field_path) {
            continue;
        }
        // Two-entity split: absolutely-positioned wrapper +
        // button-bundle child (the bundle ships its own Node, so
        // stacking a custom Node alongside would double-insert).
        //
        // Attached via `attach_or_despawn` so concurrent inspector
        // rebuilds that cascade-despawn `row_entity` this frame
        // don't leave orphaned `ChildOf` relationships.
        let wrapper = commands
            .spawn((Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                right: Val::Px(4.0),
                ..default()
            },))
            .id();

        let button_entity = commands
            .spawn((
                AnimDiamondButton {
                    source_entity: row.source_entity,
                    component_type_path: row.type_path.clone(),
                    field_path: row.field_path.clone(),
                },
                button(
                    ButtonProps::new("")
                        .with_variant(ButtonVariant::Ghost)
                        .with_size(ButtonSize::IconSM)
                        .with_left_icon(Icon::Diamond),
                ),
            ))
            .id();

        jackdaw_feathers::utils::attach_or_despawn(&mut commands, wrapper, button_entity);
        jackdaw_feathers::utils::attach_or_despawn(&mut commands, row_entity, wrapper);
    }
}

/// Observer: when a diamond button is clicked, dispatch
/// `animation.toggle_keyframe` with the bound property's params.
pub fn on_diamond_click(
    event: On<ButtonClickEvent>,
    buttons: Query<&AnimDiamondButton>,
    mut commands: Commands,
) {
    let Ok(button_ref) = buttons.get(event.entity) else {
        return;
    };
    commands
        .operator(super::ops::AnimationToggleKeyframeOp::ID)
        .param("entity", button_ref.source_entity.to_bits() as i64)
        .param(
            "component_type_path",
            button_ref.component_type_path.clone(),
        )
        .param("field_path", button_ref.field_path.clone())
        .call();
}

/// Spawn (or replace) a keyframe at the current cursor time on the
/// given entity's clip/track for the named property. Shared exclusive
/// system; call via `world.run_system_cached_with(toggle_keyframe,
/// (entity, type_path, field_path))`.
pub(crate) fn toggle_keyframe(
    In((source_entity, component_type_path, field_path)): In<(Entity, String, String)>,
    world: &mut World,
) {
    let cursor_time = world
        .get_resource::<TimelineCursor>()
        .map(|c| c.seek_time)
        .unwrap_or(0.0);

    let Some(clip_entity) = find_or_create_clip(world, source_entity) else {
        warn!(
            "toggle_keyframe: source entity {source_entity} has no Name - \
             give it one in the inspector first so the clip's target can \
             resolve"
        );
        return;
    };

    let track_entity = find_or_create_track(world, clip_entity, &component_type_path, &field_path);
    world
        .run_system_cached_with(
            spawn_typed_keyframe,
            (
                source_entity,
                track_entity,
                component_type_path.clone(),
                field_path.clone(),
                cursor_time,
            ),
        )
        .ok();

    if let Some(mut clip) = world.get_mut::<Clip>(clip_entity)
        && cursor_time > clip.duration
    {
        clip.duration = cursor_time;
    }

    if let Some(mut selected) = world.get_resource_mut::<SelectedClip>() {
        selected.0 = Some(clip_entity);
    }
    if let Some(mut dirty) = world.get_resource_mut::<TimelineDirty>() {
        dirty.0 = true;
    }
}

/// Return an existing `Clip` child of `source_entity`, or spawn one
/// and return its entity. Returns `None` if the source entity has no
/// `Name`, because name is required for the compile step to derive
/// the `AnimationTargetId`.
fn find_or_create_clip(world: &mut World, source_entity: Entity) -> Option<Entity> {
    let target_name = world
        .get::<Name>(source_entity)
        .map(|n| n.as_str().to_string())?;

    // Check existing Clip children.
    if let Some(children) = world.get::<Children>(source_entity) {
        let children_vec: Vec<Entity> = children.iter().collect();
        for child in children_vec {
            if world.get::<Clip>(child).is_some() {
                return Some(child);
            }
        }
    }

    // None exist - spawn one as a child of the source.
    let clip = world
        .spawn((
            Clip::default(),
            Name::new(format!("{target_name} Clip")),
            ChildOf(source_entity),
        ))
        .id();
    Some(clip)
}

/// Return an existing `AnimationTrack` child of `clip_entity` matching
/// `(component_type_path, field_path)`, or spawn a new one.
fn find_or_create_track(
    world: &mut World,
    clip_entity: Entity,
    component_type_path: &str,
    field_path: &str,
) -> Entity {
    if let Some(children) = world.get::<Children>(clip_entity) {
        let children_vec: Vec<Entity> = children.iter().collect();
        for child in children_vec {
            if let Some(track) = world.get::<AnimationTrack>(child)
                && track.component_type_path == component_type_path
                && track.field_path == field_path
            {
                return child;
            }
        }
    }

    let label = format!("/ {field_path}");
    world
        .spawn((
            AnimationTrack::new(component_type_path.to_string(), field_path.to_string()),
            Name::new(label),
            ChildOf(clip_entity),
        ))
        .id()
}

/// Snapshot the current value of the animated field on the source
/// entity and spawn the appropriate typed keyframe component.
///
/// This is the dispatch mirror of
/// `jackdaw_animation::compile::build_curve_for_track` and
/// `jackdaw_animation::timeline::handle_add_keyframe_click`. Adding a
/// new animatable property means a new arm here plus a new arm
/// there. Keep them in sync.
fn spawn_typed_keyframe(
    In((source_entity, track_entity, component_type_path, field_path, time)): In<(
        Entity,
        Entity,
        String,
        String,
        f32,
    )>,
    transforms: Query<&Transform>,
    mut commands: Commands,
) {
    match (component_type_path.as_str(), field_path.as_str()) {
        (TRANSFORM, "translation") => {
            let Ok(&transform) = transforms.get(source_entity) else {
                warn!("Diamond click: source has no Transform");
                return;
            };
            commands.spawn((
                Vec3Keyframe {
                    time,
                    value: transform.translation,
                },
                ChildOf(track_entity),
            ));
        }
        (TRANSFORM, "rotation") => {
            let Ok(&transform) = transforms.get(source_entity) else {
                warn!("Diamond click: source has no Transform");
                return;
            };
            commands.spawn((
                QuatKeyframe {
                    time,
                    value: transform.rotation,
                },
                ChildOf(track_entity),
            ));
        }
        (TRANSFORM, "scale") => {
            let Ok(&transform) = transforms.get(source_entity) else {
                warn!("Diamond click: source has no Transform");
                return;
            };
            commands.spawn((
                Vec3Keyframe {
                    time,
                    value: transform.scale,
                },
                ChildOf(track_entity),
            ));
        }
        _ => {
            let _ = F32Keyframe::default();
            warn!("Diamond click: no snapshot dispatch for {component_type_path}.{field_path}",);
        }
    }
}

/// State of an animatable field, used to style its diamond icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiamondState {
    /// No track exists yet for this `(source, component, field)`.
    /// The diamond reads "click me to start animating this."
    NoTrack,
    /// A track exists and has keyframes, but the cursor isn't
    /// sitting on any of them - clicking adds a new keyframe.
    HasTrack,
    /// The cursor is exactly on an existing keyframe - clicking
    /// would replace it (currently spawns a duplicate, but the
    /// compile step dedupes on time).
    OnKeyframe,
}

/// Per-frame highlight updater: walks each diamond button's source
/// entity → `Clip` child → matching `AnimationTrack` → keyframes, and
/// paints the diamond icon with the right state color.
///
/// Runs every frame. Cheap because there are only a handful of
/// diamonds on screen (one per animatable Transform field on the
/// inspected entity) and the tree walk is shallow.
pub fn update_anim_diamond_highlights(
    buttons: Query<(Entity, &AnimDiamondButton)>,
    children_query: Query<&Children>,
    clips: Query<(), With<Clip>>,
    tracks: Query<&AnimationTrack>,
    vec3_keyframes: Query<&Vec3Keyframe>,
    quat_keyframes: Query<&QuatKeyframe>,
    f32_keyframes: Query<&F32Keyframe>,
    cursor: Res<TimelineCursor>,
    mut text_colors: Query<&mut TextColor>,
) {
    for (btn_entity, btn) in &buttons {
        let state = compute_diamond_state(
            btn,
            &children_query,
            &clips,
            &tracks,
            &vec3_keyframes,
            &quat_keyframes,
            &f32_keyframes,
            cursor.seek_time,
        );
        let color = match state {
            // Dim and slightly transparent - the field isn't
            // animated yet. Still clickable, just unobtrusive.
            DiamondState::NoTrack => Color::srgba(0.55, 0.55, 0.55, 0.65),
            // Accent blue - matches the track strip diamonds in
            // the timeline. "There's a track here; click to add a
            // keyframe at the current cursor time."
            DiamondState::HasTrack => Color::srgb(0.38, 0.72, 1.0),
            // Amber - "you're standing on an existing keyframe."
            // Same color the timeline widget uses for selected
            // keyframes, so the visual language is consistent.
            DiamondState::OnKeyframe => Color::srgb(1.0, 0.78, 0.12),
        };

        // The feathers `button()` bundle spawns an icon child
        // (`Text` + `TextFont`) via `setup_button`. Walk the
        // button's children and recolor any text nodes we find -
        // there should be exactly one (the Diamond glyph).
        recolor_button_icon(btn_entity, color, &children_query, &mut text_colors);
    }
}

fn recolor_button_icon(
    root: Entity,
    color: Color,
    children_query: &Query<&Children>,
    text_colors: &mut Query<&mut TextColor>,
) {
    let Ok(children) = children_query.get(root) else {
        return;
    };
    for child in children.iter() {
        if let Ok(mut tc) = text_colors.get_mut(child) {
            tc.0 = color;
        }
        // Feathers sometimes wraps the icon in an extra container;
        // recurse one level to be safe.
        recolor_button_icon(child, color, children_query, text_colors);
    }
}

fn compute_diamond_state(
    btn: &AnimDiamondButton,
    children_query: &Query<&Children>,
    clips: &Query<(), With<Clip>>,
    tracks: &Query<&AnimationTrack>,
    vec3_keyframes: &Query<&Vec3Keyframe>,
    quat_keyframes: &Query<&QuatKeyframe>,
    f32_keyframes: &Query<&F32Keyframe>,
    cursor_time: f32,
) -> DiamondState {
    // Step 1: find the `Clip` child of the source entity.
    let Ok(source_children) = children_query.get(btn.source_entity) else {
        return DiamondState::NoTrack;
    };
    let clip_entity = source_children.iter().find(|c| clips.contains(*c));
    let Some(clip_entity) = clip_entity else {
        return DiamondState::NoTrack;
    };

    // Step 2: find an `AnimationTrack` under that clip matching this
    // button's (component_type_path, field_path).
    let Ok(clip_children) = children_query.get(clip_entity) else {
        return DiamondState::NoTrack;
    };
    let track_entity = clip_children.iter().find(|c| {
        tracks
            .get(*c)
            .map(|t| {
                t.component_type_path == btn.component_type_path && t.field_path == btn.field_path
            })
            .unwrap_or(false)
    });
    let Some(track_entity) = track_entity else {
        return DiamondState::NoTrack;
    };

    // Step 3: walk the track's keyframes and check if any lands
    // on the cursor time within the epsilon.
    let Ok(track_children) = children_query.get(track_entity) else {
        return DiamondState::HasTrack;
    };
    for kf in track_children.iter() {
        let t = vec3_keyframes
            .get(kf)
            .map(|k| k.time)
            .or_else(|_| quat_keyframes.get(kf).map(|k| k.time))
            .or_else(|_| f32_keyframes.get(kf).map(|k| k.time))
            .ok();
        if let Some(t) = t
            && (t - cursor_time).abs() < CURSOR_ON_KEYFRAME_EPS
        {
            return DiamondState::OnKeyframe;
        }
    }

    DiamondState::HasTrack
}
