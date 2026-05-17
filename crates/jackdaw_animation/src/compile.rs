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
    animation_curves::{AnimatableCurve, AnimatableKeyframeCurve},
    graph::{AnimationGraph, AnimationNodeIndex},
};
use bevy::gltf::Gltf;
use bevy::prelude::*;

use crate::blend_graph::{AnimationBlendGraph, ClipNodeRef, OutputNode};
use crate::clip::{
    AnimationTrack, Clip, F32Keyframe, GltfClipRef, Interpolation, QuatKeyframe, Vec3Keyframe,
};

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
    gltf_refs: Query<&GltfClipRef>,
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
        // glTF-sourced and blend-graph-sourced clips are handled by
        // `compile_gltf_clips` / `compile_blend_graphs`. Skipping here
        // also means those clips never have their imported
        // `AnimationClip` handle overwritten by an empty authored
        // rebuild.
        if gltf_refs.contains(clip_entity) || blend_graphs.contains(clip_entity) {
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
            if matches!(track.interpolation, Interpolation::Step) {
                // Scaffolded: step interpolation is modeled in the data
                // model but not yet implemented in the compile step.
                // When the first user needs it, add a `StepCurve<T>`
                // impl and a dispatch arm below.
                warn!(
                    "Step interpolation not yet supported (track targets {}.{})",
                    track.component_type_path, track.field_path
                );
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
            && let Some(clip_data) = clip_store.get_mut(&existing.clip)
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

        // Source must be a clip_ref for the MVP passthrough case.
        let Ok(clip_ref) = clip_refs.get(source_node) else {
            warn!(
                "Blend graph {clip_entity}: only direct Clip Reference -> \
                 Output is supported in MVP; got source node {source_node}"
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

/// Resolve glTF-sourced clips by looking up their Bevy
/// [`AnimationClip`] handle in the loaded [`Gltf`] asset and wrapping
/// it in a [`CompiledClip`]. Mirrors [`compile_clips`] for imported
/// data: no keyframes, no tracks, just a direct handle -> graph
/// conversion.
///
/// Runs every frame but only touches un-compiled glTF clips
/// (`Without<CompiledClip>`) - once a clip is compiled it falls out of
/// the query. If the Gltf asset isn't loaded yet, or the named
/// animation can't be found, the clip is left un-compiled and the
/// system retries next frame.
pub fn compile_gltf_clips(
    uncompiled: Query<(Entity, &GltfClipRef), (With<Clip>, Without<CompiledClip>)>,
    asset_server: Res<AssetServer>,
    gltfs: Res<Assets<Gltf>>,
    clip_store: Res<Assets<AnimationClip>>,
    mut graph_store: ResMut<Assets<AnimationGraph>>,
    mut clip_meta: Query<&mut Clip>,
    mut commands: Commands,
) {
    for (clip_entity, gltf_ref) in &uncompiled {
        // Request the Gltf asset (dedup if already loading); we don't
        // need to hold the handle - the scene's GltfSource entity
        // keeps it alive.
        let handle: Handle<Gltf> = asset_server.load(&gltf_ref.gltf_path);
        let Some(gltf) = gltfs.get(&handle) else {
            continue;
        };
        let Some(clip_handle) = gltf.named_animations.get(gltf_ref.clip_name.as_str()) else {
            warn!(
                "glTF clip '{}' not found in {} - available: {:?}",
                gltf_ref.clip_name,
                gltf_ref.gltf_path,
                gltf.named_animations.keys().collect::<Vec<_>>(),
            );
            continue;
        };
        let Some(clip_data) = clip_store.get(clip_handle) else {
            continue;
        };

        // Sync the authored `Clip::duration` from the imported clip so
        // the timeline widget shows the right range without requiring
        // the user to type it in manually.
        if let Ok(mut clip) = clip_meta.get_mut(clip_entity) {
            let imported = clip_data.duration();
            if (clip.duration - imported).abs() > f32::EPSILON {
                clip.duration = imported;
            }
        }

        let (graph, root_node) = AnimationGraph::from_clip(clip_handle.clone());
        let graph_handle = graph_store.add(graph);
        commands.entity(clip_entity).insert(CompiledClip {
            clip: clip_handle.clone(),
            graph: graph_handle,
            root_node,
        });
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
    match track.property_path() {
        (TRANSFORM, TRANSLATION) => {
            let kfs = collect_vec3_keyframes(track_children, vec3_keyframes);
            if let Some(curve) = build_vec3_curve(kfs) {
                clip.add_curve_to_target(
                    target_id,
                    AnimatableCurve::new(animated_field!(Transform::translation), curve),
                );
            }
        }
        (TRANSFORM, ROTATION) => {
            let kfs = collect_quat_keyframes(track_children, quat_keyframes);
            if let Some(curve) = build_quat_curve(kfs) {
                clip.add_curve_to_target(
                    target_id,
                    AnimatableCurve::new(animated_field!(Transform::rotation), curve),
                );
            }
        }
        (TRANSFORM, SCALE) => {
            let kfs = collect_vec3_keyframes(track_children, vec3_keyframes);
            if let Some(curve) = build_vec3_curve(kfs) {
                clip.add_curve_to_target(
                    target_id,
                    AnimatableCurve::new(animated_field!(Transform::scale), curve),
                );
            }
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

fn build_vec3_curve(mut kfs: Vec<(f32, Vec3)>) -> Option<AnimatableKeyframeCurve<Vec3>> {
    if kfs.is_empty() {
        return None;
    }
    if kfs.len() == 1 {
        // Bevy's keyframe curve requires at least two samples with
        // strictly increasing times. Duplicate the single authored
        // keyframe so the curve is a trivial constant - this is what
        // lets scrubbing show the authored value at all times while
        // the user is still building up the track.
        let (t, v) = kfs[0];
        kfs.push((t + 1.0, v));
    }
    AnimatableKeyframeCurve::new(kfs).ok()
}

fn build_quat_curve(mut kfs: Vec<(f32, Quat)>) -> Option<AnimatableKeyframeCurve<Quat>> {
    if kfs.is_empty() {
        return None;
    }
    if kfs.len() == 1 {
        let (t, v) = kfs[0];
        kfs.push((t + 1.0, v));
    }
    AnimatableKeyframeCurve::new(kfs).ok()
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
