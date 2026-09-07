//! Playing one library clip on something in the open scene.
//!
//! The skeleton a clip is previewed on is often already driven by a bound
//! animation set, which owns its player, its graph and its bone target ids.
//! So the preview borrows that player through [`jackdaw_animation::graph_owner`]
//! and gives it back when it stops, and the set picks its state up again.

use bevy::animation::{
    AnimationPlayer,
    graph::{AnimationGraph, AnimationGraphHandle, AnimationNodeIndex},
};
use bevy::gltf::Gltf;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};
use jackdaw_animation::graph_owner::{LoanedPlayer, PlayerLoan, lend_player, return_player};
use jackdaw_animation_runtime::{AnimationSet, AnimationSetBound, AnimationState};
use jackdaw_api::prelude::*;

use super::panel::AnimationPanelState;

/// A model spawned only so a clip has something to play on.
///
/// Editor-side state: it carries no `Reflect`, so it never reaches a saved
/// scene, and stopping the preview takes it away again.
#[derive(Component, Debug)]
pub struct PreviewMannequin;

/// What the editor was asked to preview, until it has started.
#[derive(Clone, Debug)]
struct Request {
    file: String,
    clip: String,
    entity: Option<Entity>,
    /// Parked so the load is not cancelled while the file arrives.
    gltf: Option<Handle<Gltf>>,
    /// A model spawned for this request, waiting for its skeleton to appear.
    mannequin: Option<Entity>,
    /// Frames spent waiting on that model, so a file whose scene carries no
    /// skeleton is given up on rather than retried forever.
    waited: u32,
}

/// Frames a preview model may spend spawning before the request is dropped.
const MANNEQUIN_PATIENCE_FRAMES: u32 = 600;

/// What is playing, and what has to be put back when it stops.
#[derive(Debug)]
struct Active {
    target: Entity,
    mannequin: Option<Entity>,
    file: String,
    clip: String,
    node: AnimationNodeIndex,
    duration_secs: f32,
    elapsed_secs: f32,
    playing: bool,
}

/// The clip the editor is previewing, and on what.
#[derive(Resource, Default, Debug)]
pub struct AnimationPreview {
    wanted: Option<Request>,
    active: Option<Active>,
}

impl AnimationPreview {
    /// The file and clip being previewed.
    pub fn clip(&self) -> Option<(&str, &str)> {
        self.active
            .as_ref()
            .map(|active| (active.file.as_str(), active.clip.as_str()))
    }

    /// The entity whose player is borrowed.
    pub fn target(&self) -> Option<Entity> {
        self.active.as_ref().map(|active| active.target)
    }

    /// Whether the clip is running rather than held on a frame.
    pub fn is_playing(&self) -> bool {
        self.active.as_ref().is_some_and(|active| active.playing)
    }

    /// How far through the clip the preview is, in `0.0..=1.0`.
    pub fn progress(&self) -> f32 {
        let Some(active) = &self.active else {
            return 0.0;
        };
        if active.duration_secs <= 0.0 {
            return 0.0;
        }
        (active.elapsed_secs / active.duration_secs).clamp(0.0, 1.0)
    }
}

/// Play one glTF clip on an entity in the open scene.
#[operator(
    id = "animation.preview",
    label = "Preview Clip",
    description = "Play one glTF clip on an entity, without taking away the graph a bound \
                   animation set installed.",
    params(
        clip(
            String,
            doc = "Clip to play, as \"<assets-relative file>#<clip name>\". Left out, this \
                   resumes whatever is already previewing."
        ),
        entity(
            Entity,
            doc = "What to play it on. Defaults to the selection; a selection with no \
                   skeleton gets a preview model of the clip's own file."
        ),
    ),
    allows_undo = false
)]
pub(crate) fn animation_preview(
    params: In<OperatorParameters>,
    selection: Res<crate::selection::Selection>,
    mut preview: ResMut<AnimationPreview>,
    mut panel: ResMut<AnimationPanelState>,
) -> OperatorResult {
    let Some(spec) = params.as_str("clip").filter(|spec| !spec.is_empty()) else {
        // The transport's play button carries no clip: it means the one that
        // is up, held on a frame by a pause.
        let Some(active) = preview.active.as_mut() else {
            return OperatorResult::Cancelled;
        };
        active.playing = true;
        return OperatorResult::Finished;
    };
    let (file, clip) = spec.rsplit_once('#')?;
    panel.file = Some(file.to_string());
    panel.clip = Some(clip.to_string());

    let entity = params.as_entity("entity").or_else(|| selection.primary());
    // Asking again for the clip already up resumes it rather than restarting,
    // which is what the panel's play button means after a pause.
    if let Some(active) = preview.active.as_mut()
        && active.file == file
        && active.clip == clip
    {
        active.playing = true;
        return OperatorResult::Finished;
    }
    preview.wanted = Some(Request {
        file: file.to_string(),
        clip: clip.to_string(),
        entity,
        gltf: None,
        mannequin: None,
        waited: 0,
    });
    OperatorResult::Finished
}

/// Hold the previewed clip on the frame it has reached.
#[operator(
    id = "animation.preview.pause",
    label = "Pause Preview",
    description = "Hold the previewed clip on the frame it has reached.",
    is_available = a_clip_is_previewing,
    allows_undo = false
)]
pub(crate) fn animation_preview_pause(
    _: In<OperatorParameters>,
    mut preview: ResMut<AnimationPreview>,
) -> OperatorResult {
    let Some(active) = preview.active.as_mut() else {
        return OperatorResult::Cancelled;
    };
    active.playing = false;
    OperatorResult::Finished
}

/// Stop the preview and give the target back whatever was driving it.
#[operator(
    id = "animation.preview.stop",
    label = "Stop Preview",
    description = "Stop the previewed clip and hand the target back to whatever was driving it.",
    allows_undo = false
)]
pub(crate) fn animation_preview_stop(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(stop_preview);
    OperatorResult::Finished
}

fn a_clip_is_previewing(preview: Res<AnimationPreview>) -> bool {
    preview.active.is_some()
}

/// Take the preview down: give the player back, and take away a model spawned
/// only to carry it.
///
/// A set whose skeleton this was asks for its state again on the next frame,
/// through the one system in this module that watches for a returned player.
pub fn stop_preview(world: &mut World) {
    let Some(active) = world.resource_mut::<AnimationPreview>().active.take() else {
        return;
    };
    return_player(world, active.target);
    if let Some(mannequin) = active.mannequin
        && let Ok(mannequin) = world.get_entity_mut(mannequin)
    {
        mannequin.despawn();
    }
}

/// Start the clip the preview operator asked for, once its file has arrived
/// and something with a skeleton is there to play it on.
///
/// Retried each frame rather than done in the operator: a glTF load and the
/// scene it spawns both take frames the click cannot wait for.
fn start_requested_preview(world: &mut World) {
    let Some(mut request) = world.resource::<AnimationPreview>().wanted.clone() else {
        return;
    };

    let handle = match request.gltf.clone() {
        Some(handle) => handle,
        None => {
            let path = crate::entity_ops::to_asset_path(&request.file);
            let handle: Handle<Gltf> = world.resource::<AssetServer>().load(path);
            request.gltf = Some(handle.clone());
            world.resource_mut::<AnimationPreview>().wanted = Some(request.clone());
            handle
        }
    };
    let Some(gltf) = world.resource::<Assets<Gltf>>().get(&handle) else {
        if matches!(
            world.resource::<AssetServer>().get_load_state(&handle),
            Some(bevy::asset::LoadState::Failed(_))
        ) {
            warn!("animation.preview: {} could not be read", request.file);
            world.resource_mut::<AnimationPreview>().wanted = None;
        }
        return;
    };
    let Some(clip_handle) = gltf.named_animations.get(request.clip.as_str()).cloned() else {
        let held: Vec<&str> = gltf.named_animations.keys().map(|name| &**name).collect();
        warn!(
            "animation.preview: {} holds no clip named `{}`, only {held:?}",
            request.file, request.clip
        );
        world.resource_mut::<AnimationPreview>().wanted = None;
        return;
    };
    let first_scene = gltf.scenes.first().cloned();

    let Some(target) = resolve_target(world, &mut request, first_scene) else {
        // Either a model is still spawning, or there is nothing to play on and
        // `resolve_target` has already said so.
        return;
    };

    stop_preview(world);

    let duration_secs = world
        .resource::<Assets<AnimationClip>>()
        .get(&clip_handle)
        .map_or(0.0, AnimationClip::duration);
    let repeat = super::library::looped_hint(&request.clip);
    let tagged = jackdaw_animation_runtime::tag_animation_targets(world, target);
    let (graph, node) = AnimationGraph::from_clip(clip_handle);
    let graph = world.resource_mut::<Assets<AnimationGraph>>().add(graph);
    lend_player(
        world,
        target,
        PlayerLoan::new(graph, node)
            .at(0.0, true)
            .repeating(repeat)
            .untagging(tagged),
    );

    let mut preview = world.resource_mut::<AnimationPreview>();
    preview.wanted = None;
    preview.active = Some(Active {
        target,
        mannequin: request.mannequin,
        file: request.file,
        clip: request.clip,
        node,
        duration_secs,
        elapsed_secs: 0.0,
        playing: true,
    });
}

/// The entity whose player the clip should drive.
///
/// A set anywhere under the selection names its own skeleton root; a model
/// Bevy loaded already carries a player somewhere under it; anything else
/// takes the clip on itself, which is what a clip authored against one named
/// entity means. A selection with no skeleton to speak of gets a model of the
/// clip's own file, spawned at the origin for as long as the preview lasts.
///
/// Returns `None` while a model spawned for the preview has yet to appear, and
/// after saying so when there is nothing to play on at all.
fn resolve_target(
    world: &mut World,
    request: &mut Request,
    first_scene: Option<Handle<WorldAsset>>,
) -> Option<Entity> {
    if let Some(mannequin) = request.mannequin {
        // The model spawns over a few frames; keep the request until its
        // skeleton is there.
        if let Some(player) = player_descendant(world, mannequin) {
            return Some(player);
        }
        request.waited += 1;
        if request.waited > MANNEQUIN_PATIENCE_FRAMES {
            warn!(
                "animation.preview: the model of {} spawned no skeleton to play on",
                request.file
            );
            world.entity_mut(mannequin).despawn();
            world.resource_mut::<AnimationPreview>().wanted = None;
            return None;
        }
        world.resource_mut::<AnimationPreview>().wanted = Some(request.clone());
        return None;
    }
    if let Some(entity) = request
        .entity
        .filter(|&entity| world.entities().contains(entity))
    {
        let under = descendants(world, entity);
        let mut names_a_skeleton = false;
        for &candidate in &under {
            if let Some(bound) = world.get::<AnimationSetBound>(candidate) {
                return Some(bound.player);
            }
            let Some(set) = world.get::<AnimationSet>(candidate) else {
                continue;
            };
            names_a_skeleton = true;
            let wanted = set.skeleton_root.clone();
            if let Some(root) = descendant_named(world, candidate, &wanted) {
                return Some(root);
            }
        }
        if let Some(player) = player_descendant(world, entity) {
            return Some(player);
        }
        // A set whose skeleton is nowhere under it is worn by something the
        // game builds, not by anything in the open scene, so it has no bones
        // here to drive.
        if !names_a_skeleton && world.get::<Children>(entity).is_some() {
            return Some(entity);
        }
    }

    // Nothing selected wears a skeleton, so give the clip a body of its own.
    let Some(scene) = first_scene else {
        warn!(
            "animation.preview: nothing selected has a skeleton, and {} holds no scene to \
             preview one with",
            request.file
        );
        world.resource_mut::<AnimationPreview>().wanted = None;
        return None;
    };
    let mannequin = world
        .spawn((
            PreviewMannequin,
            Name::new(format!("Preview {}", request.file)),
            WorldAssetRoot(scene),
            Transform::default(),
            crate::EditorEntity,
        ))
        .id();
    request.mannequin = Some(mannequin);
    world.resource_mut::<AnimationPreview>().wanted = Some(request.clone());
    None
}

/// The first descendant of `root` carrying an `AnimationPlayer`, breadth first.
fn player_descendant(world: &World, root: Entity) -> Option<Entity> {
    descendants(world, root)
        .into_iter()
        .find(|&entity| world.get::<AnimationPlayer>(entity).is_some())
}

/// The nearest descendant of `root` answering to `wanted`.
fn descendant_named(world: &World, root: Entity, wanted: &str) -> Option<Entity> {
    descendants(world, root)
        .into_iter()
        .skip(1)
        .find(|&entity| {
            world
                .get::<Name>(entity)
                .is_some_and(|name| name.as_str() == wanted)
        })
}

/// `root` and everything under it, breadth first.
fn descendants(world: &World, root: Entity) -> Vec<Entity> {
    let mut found = vec![root];
    let mut visited = 0;
    while visited < found.len() {
        let entity = found[visited];
        visited += 1;
        if let Some(kids) = world.get::<Children>(entity) {
            found.extend(kids.iter());
        }
    }
    found
}

/// Keep the preview's transport in step with the player it borrowed.
fn drive_preview_player(
    mut preview: ResMut<AnimationPreview>,
    mut players: Query<&mut AnimationPlayer>,
) {
    let Some(active) = preview.active.as_mut() else {
        return;
    };
    let Ok(mut player) = players.get_mut(active.target) else {
        return;
    };
    let Some(animation) = player.animation_mut(active.node) else {
        return;
    };
    if active.playing == animation.is_paused() {
        if active.playing {
            animation.resume();
        } else {
            animation.pause();
        }
    }
    active.elapsed_secs = animation.seek_time();
}

/// Play a bound set's state again once the editor hands its player back.
///
/// The preview and the timeline transport both borrow the player through
/// [`jackdaw_animation::graph_owner`], which returns it stopped and back on the
/// set's own graph. Nothing else notices, so this asks for the state again.
fn restart_returned_sets(
    mut sets: Query<(&AnimationSetBound, &mut AnimationState)>,
    players: Query<&AnimationPlayer>,
    lent: Query<(), With<LoanedPlayer>>,
    graphs: Query<&AnimationGraphHandle>,
) {
    for (bound, mut state) in &mut sets {
        if lent.contains(bound.player) {
            continue;
        }
        let Ok(player) = players.get(bound.player) else {
            continue;
        };
        if player.playing_animations().next().is_some() {
            continue;
        }
        let on_its_own_graph = graphs
            .get(bound.player)
            .is_ok_and(|handle| handle.0.id() == bound.graph.id());
        if !on_its_own_graph || !bound.nodes.contains_key(&state.0) {
            continue;
        }
        state.set_changed();
    }
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<AnimationPreview>().add_systems(
        Update,
        (
            start_requested_preview,
            drive_preview_player,
            restart_returned_sets,
        )
            .chain()
            .run_if(in_state(crate::AppState::Editor)),
    );
}
