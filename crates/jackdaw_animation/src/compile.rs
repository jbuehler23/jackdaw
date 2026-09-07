//! AST to Bevy animation compile step.
//!
//! Rebuilds `AnimationClip` + `AnimationGraph` from the authored
//! track/keyframe tree whenever it changes. Output is stored as a
//! runtime-only `CompiledClip` component, never serialized.
//!
//! New animated fields: add an arm in `build_curve_for_track`.
//! New value types: add a keyframe component in `clip.rs` plus a
//! collector here.

use std::cmp::Ordering;
use std::collections::HashSet;

use bevy::animation::{
    AnimationClip, AnimationTargetId, animated_field,
    animation_curves::{AnimatableCurve, AnimatableKeyframeCurve, AnimatableProperty},
    gltf_curves::{CubicKeyframeCurve, CubicRotationCurve, SteppedKeyframeCurve},
    graph::{AnimationGraph, AnimationNodeIndex},
};
use bevy::prelude::*;

use crate::blend_graph::{AnimationBlendGraph, ClipNodeRef, OutputNode};
use crate::clip::{AnimationTrack, Clip, F32Keyframe, Interpolation, QuatKeyframe, Vec3Keyframe};

// Well-known property paths we know how to animate. These constants
// keep the dispatch table in `build_curve_for_track` readable and give
// one place to look when mapping a Bevy component+field string to a
// compile-time `animated_field!` call.
const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
const TRANSLATION: &str = "translation";
const ROTATION: &str = "rotation";
const SCALE: &str = "scale";

/// Runtime-only: the compiled Bevy assets for an authored clip.
///
/// Created the first time a clip is compiled; on subsequent compiles
/// the underlying `AnimationClip` asset is **mutated in place** via
/// `Assets::get_mut` so the handles (and the bound target's
/// `AnimationGraphHandle`) stay valid across edits. Without this, every
/// recompile would produce fresh handles and the target would keep
/// sampling the stale graph it was first bound to.
#[derive(Component, Debug, Clone)]
pub struct CompiledClip {
    pub clip: Handle<AnimationClip>,
    pub graph: Handle<AnimationGraph>,
    pub root_node: AnimationNodeIndex,
}

/// Rebuild `AnimationClip` + `AnimationGraph` assets for any clip whose
/// authored data changed this frame. Walks up from every changed entity
/// (clip, track, or keyframe) to find the owning clip, deduplicates,
/// then rebuilds each affected clip.
///
/// On the **first** compile of a clip, we create fresh asset handles
/// and attach a [`CompiledClip`] component. On **subsequent** compiles,
/// we mutate the existing `AnimationClip` asset in place so the handle
/// stays valid - otherwise the bound target's `AnimationGraphHandle`
/// would go stale after every edit.
pub fn compile_clips(
    changed: Query<
        Entity,
        Or<(
            Changed<Clip>,
            Changed<AnimationTrack>,
            Changed<Vec3Keyframe>,
            Changed<QuatKeyframe>,
            Changed<F32Keyframe>,
            Changed<Children>,
        )>,
    >,
    parents: Query<&ChildOf>,
    existing_compiled: Query<&CompiledClip>,
    clips: Query<(&Clip, Option<&Children>)>,
    blend_graphs: Query<(), With<AnimationBlendGraph>>,
    tracks: Query<(&AnimationTrack, Option<&Children>)>,
    vec3_keyframes: Query<&Vec3Keyframe>,
    quat_keyframes: Query<&QuatKeyframe>,
    f32_keyframes: Query<&F32Keyframe>,
    names: Query<&Name>,
    mut clip_store: ResMut<Assets<AnimationClip>>,
    mut graph_store: ResMut<Assets<AnimationGraph>>,
    mut commands: Commands,
) {
    let mut dirty: HashSet<Entity> = HashSet::new();
    for entity in &changed {
        if let Some(clip) = find_owning_clip(entity, &parents, &clips) {
            dirty.insert(clip);
        }
    }

    for clip_entity in dirty {
        // Blend-graph clips are handled by `compile_blend_graphs`; skipping
        // here keeps an empty authored rebuild from overwriting the handle
        // that step resolved.
        if blend_graphs.contains(clip_entity) {
            continue;
        }
        let Ok((clip_meta, clip_children)) = clips.get(clip_entity) else {
            continue;
        };

        // Derive the animation target from the clip's parent - that's
        // the entity this clip animates. Without a parent we can't
        // compile (there's no target for curves to reference).
        let Some(target_id) = target_for_clip(clip_entity, &parents, &names) else {
            warn!(
                "Clip {clip_entity} has no named parent; skipping compile. \
                 Clips must be spawned as children of the entity they animate."
            );
            continue;
        };

        let mut new_clip_data = AnimationClip::default();

        for track_entity in clip_children.into_iter().flatten() {
            let Ok((track, track_children)) = tracks.get(*track_entity) else {
                continue;
            };
            if !track.enabled {
                continue;
            }
            build_curve_for_track(
                track,
                target_id,
                track_children,
                &vec3_keyframes,
                &quat_keyframes,
                &f32_keyframes,
                &mut new_clip_data,
            );
        }

        // Honor the clip's authored duration so Bevy's evaluator plays
        // through any "dead space" past the last keyframe instead of
        // stopping at the curve's natural end. `add_curve_to_target`
        // only ever grows the internal duration, so if the authored
        // duration is longer we set it explicitly via the public
        // setter.
        let target_duration = new_clip_data.duration().max(clip_meta.duration);
        new_clip_data.set_duration(target_duration);

        // If this clip was compiled before, mutate the existing asset
        // in place so the target entity's `AnimationGraphHandle` stays
        // valid. Otherwise spin up fresh assets.
        if let Ok(existing) = existing_compiled.get(clip_entity)
            && let Some(mut clip_data) = clip_store.get_mut(&existing.clip)
        {
            *clip_data = new_clip_data;
            continue;
        }
        let clip_handle = clip_store.add(new_clip_data);
        let (graph, root_node) = AnimationGraph::from_clip(clip_handle.clone());
        let graph_handle = graph_store.add(graph);
        commands.entity(clip_entity).insert(CompiledClip {
            clip: clip_handle,
            graph: graph_handle,
            root_node,
        });
    }
}

/// Compile [`AnimationBlendGraph`] clips into [`CompiledClip`] by walking
/// the node canvas subtree and resolving it to a Bevy
/// [`AnimationGraph`]. Runs every frame, but cheap - only walks when
/// a blend graph clip doesn't yet have a `CompiledClip` or when its
/// canvas contents changed.
///
/// **Scope:** single-clip passthrough only. If the
/// graph has exactly one `anim.clip_ref` node connected to one
/// `anim.output` node and the referenced clip has a `CompiledClip`,
/// this system clones the referenced clip's compiled handles onto
/// the blend graph clip. More complex topologies (actual blends,
/// additive, chained graphs) warn and leave the blend graph
/// un-compiled until a later phase adds the proper tree walker.
///
/// [`AnimationBlendGraph`]: crate::blend_graph::AnimationBlendGraph
/// [`AnimationGraph`]: bevy::animation::graph::AnimationGraph
pub fn compile_blend_graphs(
    blend_graphs: Query<(Entity, Option<&Children>), (With<Clip>, With<AnimationBlendGraph>)>,
    existing_compiled: Query<&CompiledClip>,
    graph_nodes: Query<&jackdaw_node_graph::GraphNode>,
    connections: Query<&jackdaw_node_graph::Connection>,
    clip_refs: Query<&ClipNodeRef>,
    outputs: Query<(), With<OutputNode>>,
    mut commands: Commands,
) {
    for (clip_entity, clip_children) in &blend_graphs {
        let Some(children) = clip_children else {
            // No graph yet - leave any previous CompiledClip alone so
            // an already-working blend graph keeps playing while the
            // user mid-edits the canvas.
            continue;
        };

        // Collect all graph nodes + connections under this clip.
        let mut output_node: Option<Entity> = None;
        let mut clip_ref_nodes: Vec<Entity> = Vec::new();
        let mut blend_graph_conns: Vec<&jackdaw_node_graph::Connection> = Vec::new();
        for &child in children.iter().collect::<Vec<_>>().iter() {
            if graph_nodes.contains(child) {
                if outputs.contains(child) {
                    output_node = Some(child);
                }
                if clip_refs.contains(child) {
                    clip_ref_nodes.push(child);
                }
            } else if let Ok(conn) = connections.get(child) {
                blend_graph_conns.push(conn);
            }
        }

        let Some(output_entity) = output_node else {
            // No output node yet - user still building the graph.
            continue;
        };

        // Find the incoming connection to the output's single input.
        let incoming: Vec<&jackdaw_node_graph::Connection> = blend_graph_conns
            .iter()
            .filter(|c| c.target_node == output_entity)
            .copied()
            .collect();
        if incoming.len() != 1 {
            // Zero or multiple incoming - ambiguous or incomplete.
            continue;
        }
        let source_node = incoming[0].source_node;

        // The passthrough case requires the source to be a clip_ref.
        let Ok(clip_ref) = clip_refs.get(source_node) else {
            warn!(
                "Blend graph {clip_entity}: only a direct Clip Reference -> \
                 Output connection is supported; got source node {source_node}"
            );
            continue;
        };
        let referenced_clip = clip_ref.clip_entity;
        if referenced_clip == Entity::PLACEHOLDER {
            continue;
        }
        let Ok(compiled) = existing_compiled.get(referenced_clip) else {
            // Referenced clip hasn't compiled yet. Retry next frame.
            continue;
        };

        // Passthrough: clone the referenced clip's compiled handles
        // onto this blend graph clip. Unconditionally overwrite any
        // prior CompiledClip so canvas edits (e.g. swapping the
        // referenced clip) propagate to the bound player next frame.
        let target = existing_compiled
            .get(clip_entity)
            .map(|prior| prior.clip != compiled.clip || prior.root_node != compiled.root_node)
            .unwrap_or(true);
        if target {
            commands.entity(clip_entity).insert(compiled.clone());
        }
    }
}

/// Dispatch table: given a track and its child keyframes, collect the
/// right keyframe component type, sort by time, and call Bevy's
/// `animated_field!` macro with the matching concrete type. This is
/// the one place in the codebase that bridges "string-addressed
/// property in the AST" to "compile-time-typed curve constructor in
/// Bevy" - every other step is generic.
fn build_curve_for_track(
    track: &AnimationTrack,
    target_id: AnimationTargetId,
    track_children: Option<&Children>,
    vec3_keyframes: &Query<&Vec3Keyframe>,
    quat_keyframes: &Query<&QuatKeyframe>,
    f32_keyframes: &Query<&F32Keyframe>,
    clip: &mut AnimationClip,
) {
    let interpolation = track.interpolation;
    match track.property_path() {
        (TRANSFORM, TRANSLATION) => {
            let kfs = collect_vec3_keyframes(track_children, vec3_keyframes);
            add_vec3_curve(
                clip,
                target_id,
                animated_field!(Transform::translation),
                interpolation,
                kfs,
            );
        }
        (TRANSFORM, ROTATION) => {
            let kfs = collect_quat_keyframes(track_children, quat_keyframes);
            add_quat_curve(clip, target_id, interpolation, kfs);
        }
        (TRANSFORM, SCALE) => {
            let kfs = collect_vec3_keyframes(track_children, vec3_keyframes);
            add_vec3_curve(
                clip,
                target_id,
                animated_field!(Transform::scale),
                interpolation,
                kfs,
            );
        }
        (component, field) => {
            warn!(
                "No compile dispatch entry for {component}.{field} - \
                 add one in build_curve_for_track",
            );
            let _ = f32_keyframes; // reserved for future scalar fields
        }
    }
}

/// Add the curve one interpolation mode asks for over a `Vec3`-valued
/// property. Every mode reads the same authored keys; only the shape between
/// them differs.
fn add_vec3_curve<P>(
    clip: &mut AnimationClip,
    target_id: AnimationTargetId,
    property: P,
    interpolation: Interpolation,
    kfs: Vec<(f32, Vec3)>,
) where
    P: AnimatableProperty<Property = Vec3> + Clone + Send + Sync + 'static,
{
    let kfs = padded(kfs);
    if kfs.is_empty() {
        return;
    }
    match interpolation {
        Interpolation::Linear => {
            if let Ok(curve) = AnimatableKeyframeCurve::new(kfs) {
                clip.add_curve_to_target(target_id, AnimatableCurve::new(property, curve));
            }
        }
        Interpolation::Step => {
            if let Ok(curve) = SteppedKeyframeCurve::new(kfs) {
                clip.add_curve_to_target(target_id, AnimatableCurve::new(property, curve));
            }
        }
        Interpolation::Cubic => {
            let times: Vec<f32> = kfs.iter().map(|(t, _)| *t).collect();
            let values = spline_samples(&kfs, |a, b| a - b, |v, k| v * k, Vec3::ZERO);
            if let Ok(curve) = CubicKeyframeCurve::new(times, values) {
                clip.add_curve_to_target(target_id, AnimatableCurve::new(property, curve));
            }
        }
    }
}

/// Add the curve one interpolation mode asks for over the rotation, which
/// takes its own curve types because a quaternion is not a vector space.
fn add_quat_curve(
    clip: &mut AnimationClip,
    target_id: AnimationTargetId,
    interpolation: Interpolation,
    kfs: Vec<(f32, Quat)>,
) {
    let kfs = padded(kfs);
    if kfs.is_empty() {
        return;
    }
    let property = animated_field!(Transform::rotation);
    match interpolation {
        Interpolation::Linear => {
            if let Ok(curve) = AnimatableKeyframeCurve::new(kfs) {
                clip.add_curve_to_target(target_id, AnimatableCurve::new(property, curve));
            }
        }
        Interpolation::Step => {
            if let Ok(curve) = SteppedKeyframeCurve::new(kfs) {
                clip.add_curve_to_target(target_id, AnimatableCurve::new(property, curve));
            }
        }
        Interpolation::Cubic => {
            // The cubic rotation curve works in `Vec4` and normalizes what it
            // samples back into a quaternion, so the tangents are built on the
            // same four components.
            let times: Vec<f32> = kfs.iter().map(|(t, _)| *t).collect();
            let as_vec4: Vec<(f32, Vec4)> = kfs.iter().map(|(t, q)| (*t, Vec4::from(*q))).collect();
            let values = spline_samples(&as_vec4, |a, b| a - b, |v, k| v * k, Vec4::ZERO);
            if let Ok(curve) = CubicRotationCurve::new(times, values) {
                clip.add_curve_to_target(target_id, AnimatableCurve::new(property, curve));
            }
        }
    }
}

/// The `in-tangent, value, out-tangent` triple a cubic keyframe curve wants,
/// for each authored key.
///
/// Tangents are not authored, so each is the slope through the neighbouring
/// keys; the ends repeat their one-sided slope. That is the smooth reading of
/// a set of keys, and it is why editing a tangent is not offered.
fn spline_samples<T: Copy>(
    kfs: &[(f32, T)],
    subtract: impl Fn(T, T) -> T,
    scale: impl Fn(T, f32) -> T,
    zero: T,
) -> Vec<T> {
    let mut out = Vec::with_capacity(kfs.len() * 3);
    for (at, &(time, value)) in kfs.iter().enumerate() {
        let before = kfs
            .get(at.wrapping_sub(1))
            .copied()
            .unwrap_or((time, value));
        let after = kfs.get(at + 1).copied().unwrap_or((time, value));
        let span = after.0 - before.0;
        let tangent = if span > f32::EPSILON {
            scale(subtract(after.1, before.1), 1.0 / span)
        } else {
            zero
        };
        out.extend([tangent, value, tangent]);
    }
    out
}

/// Keys with a lone key doubled, because Bevy's keyframe curves want two
/// samples at strictly increasing times.
///
/// Duplicating the single authored key makes the curve a constant, which is
/// what lets scrubbing show the authored value while a track is still being
/// built up.
fn padded<T: Copy>(mut kfs: Vec<(f32, T)>) -> Vec<(f32, T)> {
    if kfs.len() == 1 {
        let (t, v) = kfs[0];
        kfs.push((t + 1.0, v));
    }
    kfs
}

fn collect_vec3_keyframes(
    children: Option<&Children>,
    query: &Query<&Vec3Keyframe>,
) -> Vec<(f32, Vec3)> {
    let mut kfs: Vec<(f32, Vec3)> = children
        .into_iter()
        .flatten()
        .filter_map(|c| query.get(*c).ok().map(|k| (k.time, k.value)))
        .collect();
    sort_and_dedupe_by_time(&mut kfs, |kf| kf.0);
    kfs
}

fn collect_quat_keyframes(
    children: Option<&Children>,
    query: &Query<&QuatKeyframe>,
) -> Vec<(f32, Quat)> {
    let mut kfs: Vec<(f32, Quat)> = children
        .into_iter()
        .flatten()
        .filter_map(|c| query.get(*c).ok().map(|k| (k.time, k.value)))
        .collect();
    sort_and_dedupe_by_time(&mut kfs, |kf| kf.0);
    kfs
}

/// Return the clip's visible/playback duration.
///
/// Always reads from the authored [`Clip::duration`] field rather than
/// deriving from keyframes. This keeps the timeline's visual range
/// stable as the user edits - a new keyframe lands at the cursor
/// position instead of at the visual right edge, which is what would
/// happen if the duration grew to match every new keyframe time.
pub fn clip_display_duration(
    clip_entity: Entity,
    clips: &Query<(&Clip, Option<&Children>)>,
) -> f32 {
    clips
        .get(clip_entity)
        .ok()
        .map(|(clip, _)| clip.duration.max(0.01))
        .unwrap_or(1.0)
}

/// Walk a clip's keyframes and return the max `time`. Used by the
/// add-keyframe handler to decide whether the stored duration needs
/// to grow.
pub fn max_keyframe_time(
    clip_entity: Entity,
    clips: &Query<(&Clip, Option<&Children>)>,
    tracks: &Query<(&AnimationTrack, Option<&Children>)>,
    vec3_keyframes: &Query<&Vec3Keyframe>,
    quat_keyframes: &Query<&QuatKeyframe>,
    f32_keyframes: &Query<&F32Keyframe>,
) -> f32 {
    let Ok((_, clip_children)) = clips.get(clip_entity) else {
        return 0.0;
    };
    let mut max_time = 0.0_f32;
    for track_entity in clip_children.into_iter().flatten() {
        let Ok((_, track_children)) = tracks.get(*track_entity) else {
            continue;
        };
        for kf_entity in track_children.into_iter().flatten() {
            if let Ok(kf) = vec3_keyframes.get(*kf_entity) {
                max_time = max_time.max(kf.time);
            }
            if let Ok(kf) = quat_keyframes.get(*kf_entity) {
                max_time = max_time.max(kf.time);
            }
            if let Ok(kf) = f32_keyframes.get(*kf_entity) {
                max_time = max_time.max(kf.time);
            }
        }
    }
    max_time
}

fn find_owning_clip(
    start: Entity,
    parents: &Query<&ChildOf>,
    clips: &Query<(&Clip, Option<&Children>)>,
) -> Option<Entity> {
    let mut cur = start;
    for _ in 0..8 {
        if clips.contains(cur) {
            return Some(cur);
        }
        cur = parents.get(cur).ok()?.parent();
    }
    None
}

/// Derive the `AnimationTargetId` for a clip from the clip entity's
/// parent. All tracks under the clip share this target. Returns
/// `None` if the clip has no parent or the parent has no `Name`.
pub fn target_for_clip(
    clip_entity: Entity,
    parents: &Query<&ChildOf>,
    names: &Query<&Name>,
) -> Option<AnimationTargetId> {
    let parent = parents.get(clip_entity).ok()?.parent();
    let name = names.get(parent).ok()?;
    Some(AnimationTargetId::from_name(name))
}

fn sort_and_dedupe_by_time<T>(items: &mut Vec<T>, time_of: impl Fn(&T) -> f32) {
    items.sort_by(|a, b| {
        time_of(a)
            .partial_cmp(&time_of(b))
            .unwrap_or(Ordering::Equal)
    });
    items.dedup_by(|a, b| (time_of(a) - time_of(b)).abs() < f32::EPSILON);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;

    /// An app holding just enough to run the compile step.
    fn app_that_compiles() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<AnimationClip>()
            .init_asset::<AnimationGraph>()
            .add_systems(Update, compile_clips);
        app
    }

    /// A door with a clip on it, holding a translation track and a scale track
    /// of two keys each.
    fn door_with_two_tracks(app: &mut App) -> (Entity, Entity) {
        let door = app.world_mut().spawn(Name::new("Door")).id();
        let clip = app.world_mut().spawn((Clip::default(), ChildOf(door))).id();
        let mut track_of = |field: &str| {
            let track = app
                .world_mut()
                .spawn((AnimationTrack::new(TRANSFORM, field), ChildOf(clip)))
                .id();
            for (time, value) in [(0.0, Vec3::ZERO), (1.0, Vec3::ONE)] {
                app.world_mut()
                    .spawn((Vec3Keyframe { time, value }, ChildOf(track)));
            }
            track
        };
        let translation = track_of(TRANSLATION);
        track_of(SCALE);
        (clip, translation)
    }

    /// How many curves the compiled clip drives its target with.
    fn compiled_curve_count(app: &mut App, clip: Entity) -> usize {
        let handle = app
            .world()
            .get::<CompiledClip>(clip)
            .expect("the clip compiled")
            .clip
            .clone();
        app.world()
            .resource::<Assets<AnimationClip>>()
            .get(&handle)
            .expect("the compiled asset")
            .curves()
            .values()
            .map(Vec::len)
            .sum()
    }

    #[test]
    fn a_disabled_track_is_left_out_of_the_compiled_clip() {
        let mut app = app_that_compiles();
        let (clip, translation) = door_with_two_tracks(&mut app);
        app.update();
        assert_eq!(
            compiled_curve_count(&mut app, clip),
            2,
            "both tracks should compile while both are on"
        );

        app.world_mut()
            .get_mut::<AnimationTrack>(translation)
            .expect("the track")
            .enabled = false;
        app.update();

        assert_eq!(
            compiled_curve_count(&mut app, clip),
            1,
            "a track switched off must not drive its property"
        );
        assert_eq!(
            app.world()
                .get::<Children>(translation)
                .map(RelationshipTarget::len),
            Some(2),
            "and its keys have to stay where they are"
        );
    }

    #[test]
    fn every_interpolation_mode_compiles_to_a_curve() {
        for mode in [
            Interpolation::Linear,
            Interpolation::Cubic,
            Interpolation::Step,
        ] {
            let mut app = app_that_compiles();
            let (clip, translation) = door_with_two_tracks(&mut app);
            app.world_mut()
                .get_mut::<AnimationTrack>(translation)
                .expect("the track")
                .interpolation = mode;

            app.update();

            assert_eq!(
                compiled_curve_count(&mut app, clip),
                2,
                "{mode:?} left a track without a curve"
            );
        }
    }

    #[test]
    fn a_cubic_track_reads_its_tangents_off_the_keys_beside_each_one() {
        let keys = [(0.0, 0.0_f32), (1.0, 2.0), (2.0, 2.0)];
        let samples = spline_samples(&keys, |a, b| a - b, |v, k| v * k, 0.0);

        assert_eq!(samples.len(), keys.len() * 3);
        // Every triple carries the key's own value in the middle.
        assert!((samples[1] - 0.0).abs() < 1e-6, "{samples:?}");
        assert!((samples[4] - 2.0).abs() < 1e-6, "{samples:?}");
        // The middle key rises from 0 to 2 over two seconds, so its slope is
        // one; the last key sits on a flat pair, so its slope is nothing.
        assert!((samples[3] - 1.0).abs() < 1e-6, "{samples:?}");
        assert!((samples[6] - 0.0).abs() < 1e-6, "{samples:?}");
    }
}
