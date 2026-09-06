//! Playing an authored set of animation states on a skeleton loaded from glTF.
//!
//! A rig is rarely one file. The clips are exported once against a reference
//! armature, and the bodies that play them are exported separately, so the
//! animation player cannot be built by whatever loaded either half; it has to
//! be built once both have spawned. This crate holds that binding step, the
//! small state machine an author writes beside it, and the joint remapping
//! that lets several parts wear one skeleton.
//!
//! Clips address bones by a hash of their name path rather than by entity, so
//! a clip exported against one armature drives any armature whose bones carry
//! the same names. [`AnimationRuntimePlugin`] writes those ids itself because
//! Bevy's glTF loader only writes them for a file that carries animations of
//! its own, which a body exported without clips does not.

#![deny(missing_docs)]

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use bevy::{
    animation::{
        AnimatedBy, AnimationTargetId, RepeatAnimation,
        graph::{AnimationGraph, AnimationGraphHandle, AnimationNodeIndex},
        transition::AnimationTransitions,
    },
    gltf::Gltf,
    mesh::skinning::SkinnedMesh,
    prelude::*,
};
use serde::{Deserialize, Serialize};

/// The clips an entity can play and the states that choose between them.
///
/// A component equal to its `Default` emits as a bare type path, so changing
/// this `Default` silently reinterprets every scene already saved that way.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[reflect(Component, Default)]
pub struct AnimationSet {
    /// Assets-relative paths of the glTF files holding the clips. A state
    /// names one of them by its position in this list.
    pub sources: Vec<String>,
    /// Every state this set can be asked for.
    pub states: Vec<AnimationStateDef>,
    /// The state played as soon as the set binds. Empty asks for nothing.
    pub default_state: String,
    /// Name of the descendant carrying the skeleton the clips drive.
    pub skeleton_root: String,
}

impl Default for AnimationSet {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            states: Vec::new(),
            default_state: String::new(),
            skeleton_root: "Armature".to_string(),
        }
    }
}

/// One state of an [`AnimationSet`]: which clip it plays, and how.
#[derive(Reflect, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[reflect(Default)]
pub struct AnimationStateDef {
    /// What an [`AnimationState`] names to ask for this state.
    pub name: String,
    /// Index into [`AnimationSet::sources`] of the file holding the clip.
    pub source: usize,
    /// Name the clip carries in that file.
    pub clip: String,
    /// Whether the clip runs forever or once.
    pub looped: bool,
    /// Seconds over which the state it replaces fades out.
    pub transition_secs: f32,
    /// Playback rate, as a multiple of the clip's authored speed.
    pub speed: f32,
    /// State to fall back to once a non-looping clip has run out.
    pub then: Option<String>,
}

impl Default for AnimationStateDef {
    fn default() -> Self {
        Self {
            name: String::new(),
            source: 0,
            clip: String::new(),
            looped: true,
            transition_secs: 0.15,
            speed: 1.0,
            then: None,
        }
    }
}

/// The state an [`AnimationSet`] is being asked to play.
///
/// Whatever drives the entity writes this: game logic in a running world, the
/// operator that previews a state in the editor. Empty asks for nothing, so a
/// set with no default holds whatever pose its skeleton spawned in.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct AnimationState(
    /// Name of the wanted [`AnimationStateDef`].
    pub String,
);

/// Handles to the glTF files an [`AnimationSet`] draws its clips from.
///
/// Held on the entity so the files stay loaded for as long as the graph built
/// out of them is playing. Inserting it before the set binds skips the load,
/// which is what an app that already holds the files should do.
#[derive(Component, Debug, Default)]
pub struct AnimationSources(
    /// One handle per entry of [`AnimationSet::sources`], in the same order.
    pub Vec<Handle<Gltf>>,
);

/// What an [`AnimationSet`] resolved to once its skeleton and its clips were
/// both in the world.
///
/// Runtime only: it names entities and holds asset handles, so it is neither
/// reflected nor written to a document.
#[derive(Component, Debug)]
pub struct AnimationSetBound {
    /// The skeleton root, which carries the [`AnimationPlayer`].
    pub player: Entity,
    /// The graph node each playable state was added as.
    pub nodes: HashMap<String, AnimationNodeIndex>,
    /// The graph every animated root under this set shares.
    pub graph: Handle<AnimationGraph>,
    /// Further animated roots: parts whose skeleton could not be folded into
    /// the primary one and so kept their own.
    pub parts: Vec<Entity>,
    /// State names already reported as unknown, so asking again stays quiet.
    pub warned_states: HashSet<String>,
}

/// Sent when a non-looping state reaches the end of its clip.
#[derive(Message, Debug, Clone, PartialEq, Eq)]
pub struct AnimationStateFinished {
    /// The entity carrying the [`AnimationSet`].
    pub entity: Entity,
    /// The state that ran out.
    pub state: String,
}

/// The systems that bind animation sets and play the state they are asked for.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnimationSetSystems;

/// Binds authored animation sets to their skeletons and plays their states.
///
/// Add it wherever scenes carrying an [`AnimationSet`] are spawned. Nothing
/// here touches an entity without one.
pub struct AnimationRuntimePlugin;

impl Plugin for AnimationRuntimePlugin {
    fn build(&self, app: &mut App) {
        register_animation_set_types(app);
        app.add_message::<AnimationStateFinished>().add_systems(
            Update,
            (
                bind_animation_sets,
                merge_part_skeletons,
                apply_animation_state,
                report_finished_states,
            )
                .chain()
                .in_set(AnimationSetSystems)
                .run_if(
                    resource_exists::<AssetServer>
                        .and_then(resource_exists::<Assets<Gltf>>)
                        .and_then(resource_exists::<Assets<AnimationGraph>>),
                ),
        );
    }
}

/// Registers the authored animation types for reflection.
///
/// [`AnimationRuntimePlugin`] calls this; call it directly only to author and
/// load sets in an app that never plays them, such as one that only writes
/// documents.
pub fn register_animation_set_types(app: &mut App) {
    app.register_type::<AnimationSet>()
        .register_type::<AnimationStateDef>()
        .register_type::<AnimationState>();
}

/// Builds the player, the graph and the bone target ids of every set whose
/// skeleton and source files have arrived.
///
/// Retried each frame rather than run on insertion, because a glTF scene
/// spawns asynchronously and its skeleton can be several frames behind the
/// component naming it.
fn bind_animation_sets(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    gltfs: Res<Assets<Gltf>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    unbound: Query<
        (
            Entity,
            &AnimationSet,
            Option<&AnimationSources>,
            Option<&AnimationState>,
        ),
        Without<AnimationSetBound>,
    >,
    children: Query<&Children>,
    names: Query<&Name>,
    targets: Query<&AnimationTargetId>,
) {
    for (entity, set, sources, state) in &unbound {
        let Some(sources) = sources else {
            commands.entity(entity).insert(AnimationSources(
                set.sources
                    .iter()
                    .map(|path| asset_server.load(path))
                    .collect(),
            ));
            continue;
        };
        let Some(loaded) = sources
            .0
            .iter()
            .map(|handle| gltfs.get(handle))
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let Some(&root) = descendants_named(entity, &set.skeleton_root, &children, &names).first()
        else {
            continue;
        };

        let mut graph = AnimationGraph::new();
        let mut nodes = HashMap::new();
        for def in &set.states {
            let Some(gltf) = loaded.get(def.source) else {
                warn!(
                    "animation state `{}` wants source {} of {}",
                    def.name,
                    def.source,
                    loaded.len()
                );
                continue;
            };
            let Some(clip) = gltf.named_animations.get(def.clip.as_str()) else {
                let available: Vec<&str> =
                    gltf.named_animations.keys().map(|name| &**name).collect();
                warn!(
                    "animation state `{}` wants clip `{}`, and its source holds {:?}",
                    def.name, def.clip, available
                );
                continue;
            };
            let node = graph.add_clip(clip.clone(), 1.0, graph.root);
            nodes.insert(def.name.clone(), node);
        }
        let graph = graphs.add(graph);

        tag_animation_targets(
            &mut commands,
            root,
            root,
            &mut Vec::new(),
            &children,
            &names,
            &targets,
        );

        let wanted = state.map_or(set.default_state.as_str(), |state| state.0.as_str());
        let mut player = AnimationPlayer::default();
        let mut transitions = AnimationTransitions::new();
        let mut warned_states = HashSet::new();
        match resolve_state(set, &nodes, wanted) {
            Some((def, node)) => play_state(def, node, &mut player, &mut transitions),
            None if wanted.is_empty() => {}
            None => {
                warn!("animation set has no state named `{wanted}`");
                warned_states.insert(wanted.to_string());
            }
        }
        commands
            .entity(root)
            .insert((player, AnimationGraphHandle(graph.clone()), transitions));

        if state.is_none() {
            commands
                .entity(entity)
                .insert(AnimationState(wanted.to_string()));
        }
        commands.entity(entity).insert(AnimationSetBound {
            player: root,
            nodes,
            graph,
            parts: Vec::new(),
            warned_states,
        });
    }
}

/// Folds a part exported with its own copy of the skeleton onto the skeleton
/// already bound, so one player drives the whole body.
///
/// A part whose bones do not answer to the same names keeps its own skeleton
/// and becomes a second animated root on the same graph, which costs a player
/// but leaves the part visible and in step.
fn merge_part_skeletons(
    mut commands: Commands,
    mut sets: Query<(
        Entity,
        &AnimationSet,
        &mut AnimationSetBound,
        Option<&AnimationState>,
    )>,
    children: Query<&Children>,
    names: Query<&Name>,
    child_of: Query<&ChildOf>,
    targets: Query<&AnimationTargetId>,
    animated: Query<(), With<AnimationPlayer>>,
    mut skins: Query<&mut SkinnedMesh>,
) {
    for (entity, set, mut bound, state) in &mut sets {
        let primary = bound.player;
        let candidates: Vec<Entity> =
            descendants_named(entity, &set.skeleton_root, &children, &names)
                .into_iter()
                .filter(|&part| part != primary && !animated.contains(part))
                .collect();
        if candidates.is_empty() {
            continue;
        }

        let primary_joints: HashMap<String, Entity> = descendants(primary, &children)
            .into_iter()
            .filter_map(|joint| names.get(joint).ok().map(|name| (name.to_string(), joint)))
            .collect();
        let primary_parent = child_of
            .get(primary)
            .map_or(entity, |&ChildOf(parent)| parent);

        for part in candidates {
            // A part exported beside its skeleton keeps its meshes as the
            // skeleton's siblings, so the part is its parent's whole subtree.
            let part_root = child_of
                .get(part)
                .map(|&ChildOf(parent)| parent)
                .ok()
                .filter(|&parent| !descendants(parent, &children).contains(&primary))
                .unwrap_or(part);
            let part_meshes: Vec<Entity> = descendants(part_root, &children)
                .into_iter()
                .filter(|&mesh| skins.contains(mesh))
                .collect();
            let Some(remapped) = remap_joints(&part_meshes, &primary_joints, &skins, &names) else {
                warn!(
                    "an animated part does not answer to the same bone names as the skeleton it \
                     was placed under, so it keeps its own"
                );
                bind_extra_root(
                    &mut commands,
                    part,
                    &bound,
                    set,
                    state,
                    &children,
                    &names,
                    &targets,
                );
                bound.parts.push(part);
                continue;
            };
            for (mesh, joints) in remapped {
                if let Ok(mut skin) = skins.get_mut(mesh) {
                    skin.joints = joints;
                }
                commands.entity(mesh).insert(ChildOf(primary_parent));
            }
            commands.entity(part).despawn();
        }
    }
}

/// Plays the state an [`AnimationState`] was changed to.
fn apply_animation_state(
    mut sets: Query<
        (&AnimationSet, &mut AnimationSetBound, &AnimationState),
        Changed<AnimationState>,
    >,
    mut players: Query<(&mut AnimationPlayer, &mut AnimationTransitions)>,
) {
    for (set, mut bound, state) in &mut sets {
        let Some((def, node)) = resolve_state(set, &bound.nodes, &state.0) else {
            if !state.0.is_empty() && bound.warned_states.insert(state.0.clone()) {
                warn!("animation set has no state named `{}`", state.0);
            }
            continue;
        };
        let roots: Vec<Entity> = std::iter::once(bound.player)
            .chain(bound.parts.iter().copied())
            .collect();
        for root in roots {
            let Ok((mut player, mut transitions)) = players.get_mut(root) else {
                continue;
            };
            play_state(def, node, &mut player, &mut transitions);
        }
    }
}

/// Reports a non-looping state that has run out, and moves on to whatever it
/// said should follow.
fn report_finished_states(
    mut sets: Query<(
        Entity,
        &AnimationSet,
        &AnimationSetBound,
        &mut AnimationState,
    )>,
    players: Query<(&AnimationPlayer, &AnimationTransitions)>,
    mut finished: MessageWriter<AnimationStateFinished>,
) {
    for (entity, set, bound, mut state) in &mut sets {
        let Some((def, node)) = resolve_state(set, &bound.nodes, &state.0) else {
            continue;
        };
        if def.looped {
            continue;
        }
        let Ok((player, transitions)) = players.get(bound.player) else {
            continue;
        };
        if transitions.get_main_animation() != Some(node) {
            continue;
        }
        // `just_completed` holds for the one frame the clip ran out on, which
        // is what keeps a state with no `then` from reporting forever.
        let ran_out = player
            .animation(node)
            .is_some_and(|active| active.just_completed() && active.is_finished());
        if !ran_out {
            continue;
        }
        finished.write(AnimationStateFinished {
            entity,
            state: state.0.clone(),
        });
        if let Some(next) = &def.then {
            state.0 = next.clone();
        }
    }
}

/// The state definition and graph node a name asks for, when the set has both.
fn resolve_state<'a>(
    set: &'a AnimationSet,
    nodes: &HashMap<String, AnimationNodeIndex>,
    wanted: &str,
) -> Option<(&'a AnimationStateDef, AnimationNodeIndex)> {
    let def = set.states.iter().find(|def| def.name == wanted)?;
    let node = *nodes.get(wanted)?;
    Some((def, node))
}

/// Starts a state on one animated root, fading out whatever it replaces.
fn play_state(
    def: &AnimationStateDef,
    node: AnimationNodeIndex,
    player: &mut AnimationPlayer,
    transitions: &mut AnimationTransitions,
) {
    // Asking again for the state already running would restart its fade.
    if transitions.get_main_animation() == Some(node)
        && player
            .animation(node)
            .is_some_and(|active| !active.is_finished())
    {
        return;
    }
    let repeat = if def.looped {
        RepeatAnimation::Forever
    } else {
        RepeatAnimation::Never
    };
    transitions
        .play(player, node, Duration::from_secs_f32(def.transition_secs))
        .set_repeat(repeat)
        .set_speed(def.speed);
}

/// Gives a part that kept its own skeleton a player of its own on the shared
/// graph, so it stays in step with the body it was placed under.
fn bind_extra_root(
    commands: &mut Commands,
    root: Entity,
    bound: &AnimationSetBound,
    set: &AnimationSet,
    state: Option<&AnimationState>,
    children: &Query<&Children>,
    names: &Query<&Name>,
    targets: &Query<&AnimationTargetId>,
) {
    tag_animation_targets(
        commands,
        root,
        root,
        &mut Vec::new(),
        children,
        names,
        targets,
    );
    let wanted = state.map_or(set.default_state.as_str(), |state| state.0.as_str());
    let mut player = AnimationPlayer::default();
    let mut transitions = AnimationTransitions::new();
    if let Some((def, node)) = resolve_state(set, &bound.nodes, wanted) {
        play_state(def, node, &mut player, &mut transitions);
    }
    commands.entity(root).insert((
        player,
        AnimationGraphHandle(bound.graph.clone()),
        transitions,
    ));
}

/// The joints each of a part's meshes should point at once it wears the
/// primary skeleton, or `None` when a bone name has no counterpart there.
fn remap_joints(
    meshes: &[Entity],
    primary_joints: &HashMap<String, Entity>,
    skins: &Query<&mut SkinnedMesh>,
    names: &Query<&Name>,
) -> Option<Vec<(Entity, Vec<Entity>)>> {
    let mut remapped = Vec::with_capacity(meshes.len());
    for &mesh in meshes {
        let skin = skins.get(mesh).ok()?;
        let joints = skin
            .joints
            .iter()
            .map(|&joint| primary_joints.get(names.get(joint).ok()?.as_str()).copied())
            .collect::<Option<Vec<_>>>()?;
        remapped.push((mesh, joints));
    }
    Some(remapped)
}

/// Gives every named entity under `root` the target id of its name path, so a
/// clip authored against a skeleton of the same names drives this one.
///
/// An unnamed entity ends the walk: its descendants have no path to hash, and
/// glTF's own loader passes over them for the same reason.
fn tag_animation_targets(
    commands: &mut Commands,
    root: Entity,
    entity: Entity,
    path: &mut Vec<Name>,
    children: &Query<&Children>,
    names: &Query<&Name>,
    targets: &Query<&AnimationTargetId>,
) {
    let Ok(name) = names.get(entity) else {
        return;
    };
    path.push(name.clone());
    if !targets.contains(entity) {
        commands
            .entity(entity)
            .insert((AnimationTargetId::from_names(path.iter()), AnimatedBy(root)));
    }
    if let Ok(kids) = children.get(entity) {
        for child in kids.iter() {
            tag_animation_targets(commands, root, child, path, children, names, targets);
        }
    }
    path.pop();
}

/// Every descendant of `root` carrying `wanted` as its name, nearest first.
fn descendants_named(
    root: Entity,
    wanted: &str,
    children: &Query<&Children>,
    names: &Query<&Name>,
) -> Vec<Entity> {
    descendants(root, children)
        .into_iter()
        .skip(1)
        .filter(|&entity| names.get(entity).is_ok_and(|name| name.as_str() == wanted))
        .collect()
}

/// `root` and everything under it, breadth first.
fn descendants(root: Entity, children: &Query<&Children>) -> Vec<Entity> {
    let mut found = vec![root];
    let mut visited = 0;
    while visited < found.len() {
        let entity = found[visited];
        visited += 1;
        if let Ok(kids) = children.get(entity) {
            found.extend(kids.iter());
        }
    }
    found
}
