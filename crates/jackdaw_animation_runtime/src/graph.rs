//! A state machine over clips: what an animator authors, what the game writes,
//! and the evaluator that turns both into weights on Bevy's animation graph.
//!
//! A graph is its own file, `assets/animation/<name>.animgraph.bsn`, so one
//! graph serves a player prefab, a preview mannequin and every mob variant.
//! [`AnimationGraphRef`] names the file, [`AnimationParams`] is the whole of
//! what the game writes, and [`AnimationGraphPlayback`] is what the game and
//! the editor read back.

use std::{
    any::TypeId,
    collections::{HashMap, HashSet},
    time::Duration,
};

use bevy::{
    animation::{
        ActiveAnimation, AnimationClip, RepeatAnimation,
        graph::{AnimationGraph, AnimationGraphHandle, AnimationNodeIndex},
        transition::AnimationTransitions,
    },
    asset::{AssetLoader, LoadContext, io::Reader},
    gltf::Gltf,
    prelude::*,
    reflect::{FromReflect, TypePath, TypeRegistry, TypeRegistryArc},
};
use jackdaw_bsn::{BsnPatch, BsnValue, bsn_value_to_reflect, parse_bsn_text};
use serde::{Deserialize, Serialize};

use crate::{
    AnimationSet, AnimationSources, AnimationStateFinished, descendants_named,
    tag_animation_targets,
};

/// Two floats no further apart than this read as equal.
const EQUAL_EPSILON: f32 = 1.0e-5;

/// A weight closer than this to the one already written is left alone.
const WEIGHT_EPSILON: f32 = 1.0e-4;

/// A state machine over clips: the states a rig can be in, what carries it
/// between them, and the parameters those moves are decided by.
#[derive(Reflect, Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[reflect(Default)]
pub struct AnimationGraphDef {
    /// Every parameter the game may write on this graph.
    pub parameters: Vec<AnimationParameterDef>,
    /// Every state the graph can be in.
    pub states: Vec<AnimationGraphState>,
    /// The moves between states, in the order they are considered.
    pub transitions: Vec<AnimationTransitionDef>,
    /// Name of the state the graph starts in.
    pub entry: String,
}

/// A value the game writes to steer a graph.
#[derive(Reflect, Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[reflect(Default)]
pub struct AnimationParameterDef {
    /// What [`AnimationParams`] and a condition name to reach this parameter.
    pub name: String,
    /// What the parameter holds.
    pub kind: AnimationParameterKind,
    /// The value the parameter starts at. A `Bool` reads zero as false, and a
    /// `Trigger` starts unset whatever this says.
    pub default: f32,
}

/// What an [`AnimationParameterDef`] holds.
#[derive(Reflect, Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[reflect(Default)]
pub enum AnimationParameterKind {
    /// A number, such as ground speed.
    #[default]
    Float,
    /// A flag, held as zero or one.
    Bool,
    /// A one-off, set by the game and cleared by the transition that takes it.
    Trigger,
}

/// A clip named inside a glTF file.
#[derive(Reflect, Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[reflect(Default)]
pub struct AnimationClipRef {
    /// Assets-relative path of the glTF file holding the clip.
    pub source: String,
    /// Name the clip carries in that file.
    pub clip: String,
}

/// One state of a graph: what it plays and how it plays it.
#[derive(Reflect, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[reflect(Default)]
pub struct AnimationGraphState {
    /// What a transition and [`AnimationGraphPlayback`] name this state.
    pub name: String,
    /// The pose this state produces.
    pub motion: AnimationMotion,
    /// Whether the state runs forever or once. A state blending several clips
    /// always loops: its clips run for as long as the graph does.
    pub looped: bool,
    /// Playback rate, as a multiple of the clips' authored speed.
    pub speed: f32,
    /// Where the state's node sits on the editor's canvas. The evaluator never
    /// reads it, and a file that leaves it out is laid out when it is opened.
    pub position: Vec2,
}

impl Default for AnimationGraphState {
    fn default() -> Self {
        Self {
            name: String::new(),
            motion: AnimationMotion::default(),
            looped: true,
            speed: 1.0,
            position: Vec2::ZERO,
        }
    }
}

/// What a state plays: one clip, or several mixed along a parameter.
#[derive(Reflect, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[reflect(Default)]
pub enum AnimationMotion {
    /// A single clip.
    Clip(AnimationClipRef),
    /// Clips laid out along one float parameter, mixed between the two the
    /// parameter currently sits between.
    Blend1d {
        /// Name of the float parameter the mix reads.
        parameter: String,
        /// The clips and the parameter values they play alone at.
        points: Vec<AnimationBlendPoint>,
    },
}

impl Default for AnimationMotion {
    fn default() -> Self {
        Self::Clip(AnimationClipRef::default())
    }
}

/// One clip of a 1D blend and the parameter value it plays alone at.
#[derive(Reflect, Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[reflect(Default)]
pub struct AnimationBlendPoint {
    /// Parameter value at which this clip carries the whole state.
    pub threshold: f32,
    /// The clip played there.
    pub clip: AnimationClipRef,
}

/// A move from one state to another, and what has to hold for it to be taken.
#[derive(Reflect, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[reflect(Default)]
pub struct AnimationTransitionDef {
    /// State this move leaves. Empty applies from every state.
    pub from: String,
    /// State this move arrives at.
    pub to: String,
    /// Tests against the graph's parameters.
    pub conditions: Vec<AnimationCondition>,
    /// Whether every condition must hold, or any one of them.
    pub require_all: bool,
    /// How far through its clip the state it leaves must be, from zero to one.
    pub exit_time: Option<f32>,
    /// Seconds over which the state it leaves fades out.
    pub crossfade_secs: f32,
}

impl Default for AnimationTransitionDef {
    fn default() -> Self {
        Self {
            from: String::new(),
            to: String::new(),
            conditions: Vec::new(),
            require_all: true,
            exit_time: None,
            crossfade_secs: 0.15,
        }
    }
}

/// One test a transition makes against a parameter.
///
/// A condition on a `Trigger` parameter holds while the trigger is set, and
/// reads neither `op` nor `value`.
#[derive(Reflect, Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[reflect(Default)]
pub struct AnimationCondition {
    /// Name of the parameter tested.
    pub parameter: String,
    /// How the parameter is compared against `value`.
    pub op: AnimationConditionOp,
    /// What the parameter is compared against.
    pub value: f32,
}

/// How an [`AnimationCondition`] compares a parameter against its value.
#[derive(Reflect, Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[reflect(Default)]
pub enum AnimationConditionOp {
    /// The parameter is above the value.
    #[default]
    Greater,
    /// The parameter is at or above the value.
    GreaterOrEqual,
    /// The parameter is below the value.
    Less,
    /// The parameter is at or below the value.
    LessOrEqual,
    /// The parameter and the value agree to within a small tolerance.
    Equal,
    /// The parameter and the value differ by more than that tolerance.
    NotEqual,
}

impl AnimationConditionOp {
    /// Whether `parameter` stands in this relation to `value`.
    fn holds(self, parameter: f32, value: f32) -> bool {
        match self {
            Self::Greater => parameter > value,
            Self::GreaterOrEqual => parameter >= value,
            Self::Less => parameter < value,
            Self::LessOrEqual => parameter <= value,
            Self::Equal => (parameter - value).abs() <= EQUAL_EPSILON,
            Self::NotEqual => (parameter - value).abs() > EQUAL_EPSILON,
        }
    }
}

/// A graph read from an `.animgraph.bsn` file.
#[derive(Asset, TypePath, Debug, Clone, Default)]
pub struct AnimationGraphAsset {
    /// The states, transitions and parameters the file carried.
    pub def: AnimationGraphDef,
}

/// The graph an entity plays, as an assets-relative path.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[reflect(Component, Default)]
pub struct AnimationGraphRef {
    /// Assets-relative path of the `.animgraph.bsn` file.
    pub path: String,
    /// Name of the descendant carrying the skeleton the clips drive.
    pub skeleton_root: String,
}

impl Default for AnimationGraphRef {
    fn default() -> Self {
        Self {
            path: String::new(),
            skeleton_root: "Armature".to_string(),
        }
    }
}

/// Handle to the graph asset an [`AnimationGraphRef`] names.
///
/// Held on the entity so the file stays loaded while the graph built out of it
/// is playing. Inserting it beforehand skips the load, which is what an app
/// that already holds the asset should do.
#[derive(Component, Debug)]
pub struct AnimationGraphSource(
    /// The loaded graph.
    pub Handle<AnimationGraphAsset>,
);

/// The parameters steering an entity's animation graph.
///
/// This is the whole of what game code writes: it names parameters, never
/// states, so which clip a parameter reaches stays the animator's business.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct AnimationParams {
    /// Every `Float` and `Bool` parameter, a `Bool` as zero or one.
    values: HashMap<String, f32>,
    /// The `Trigger` parameters set and not yet taken.
    pending: HashSet<String>,
}

impl AnimationParams {
    /// Write a `Float` parameter.
    pub fn set_float(&mut self, name: impl Into<String>, value: f32) {
        self.values.insert(name.into(), value);
    }

    /// Write a `Bool` parameter.
    pub fn set_bool(&mut self, name: impl Into<String>, value: bool) {
        self.values
            .insert(name.into(), if value { 1.0 } else { 0.0 });
    }

    /// Set a `Trigger` parameter, until a transition takes it.
    pub fn trigger(&mut self, name: impl Into<String>) {
        self.pending.insert(name.into());
    }

    /// The value of a `Float` parameter, or zero for one never written.
    pub fn float(&self, name: &str) -> f32 {
        self.values.get(name).copied().unwrap_or_default()
    }

    /// Whether a `Bool` parameter is set.
    pub fn flag(&self, name: &str) -> bool {
        self.float(name) != 0.0
    }

    /// Whether a `Trigger` parameter is set and not yet taken.
    pub fn triggered(&self, name: &str) -> bool {
        self.pending.contains(name)
    }

    /// Clear a `Trigger` parameter, as the transition that takes it does.
    pub fn clear_trigger(&mut self, name: &str) {
        self.pending.remove(name);
    }

    /// Write a parameter that has never been written, leaving one the game has
    /// already set alone.
    fn seed(&mut self, name: &str, value: f32) {
        if !self.values.contains_key(name) {
            self.values.insert(name.to_string(), value);
        }
    }
}

/// The state an entity's animation graph is in.
#[derive(Component, Reflect, Debug, Clone, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct AnimationGraphPlayback {
    /// Name of the state now driving the skeleton.
    pub state: String,
    /// Seconds since the graph entered that state.
    pub since_secs: f32,
    /// The crossfade still running, while the move into `state` is not over.
    pub transition: Option<AnimationGraphTransition>,
}

/// A crossfade in progress, as [`AnimationGraphPlayback`] reports it.
#[derive(Reflect, Debug, Clone, Default, PartialEq)]
#[reflect(Default)]
pub struct AnimationGraphTransition {
    /// Name of the state being faded out.
    pub from: String,
    /// Seconds of the fade still to run.
    pub remaining_secs: f32,
}

/// One clip of a compiled state, and where it sits on the state's blend.
#[derive(Debug)]
struct BoundClip {
    /// The clip's leaf in the Bevy graph.
    node: AnimationNodeIndex,
    /// Parameter value at which this clip carries the whole state.
    threshold: f32,
    /// Seconds the clip runs for, which an exit time is read against.
    duration: f32,
    /// Assets-relative path of the file the clip came out of.
    source: String,
    /// Name the clip carries in that file.
    clip: String,
}

/// One compiled state of a graph.
#[derive(Debug)]
struct BoundState {
    /// The node a transition plays: the clip's leaf, or the blend above them.
    node: AnimationNodeIndex,
    /// Whether `node` is a blend, whose weight lives in the graph asset rather
    /// than on the player.
    blend: bool,
    /// The float parameter a blend reads, empty for a single clip.
    parameter: String,
    /// Whether the state runs forever or once.
    looped: bool,
    /// Playback rate, as a multiple of the clips' authored speed.
    speed: f32,
    /// The clips under this state, in threshold order.
    clips: Vec<BoundClip>,
}

/// What an [`AnimationGraphRef`] resolved to once its asset, its clips and its
/// skeleton were all in the world.
///
/// Runtime only: it names entities and holds asset handles, so it is neither
/// reflected nor written to a document.
#[derive(Component, Debug)]
pub struct AnimationGraphBound {
    /// The skeleton root, which carries the [`AnimationPlayer`].
    pub player: Entity,
    /// The Bevy graph the states were compiled into.
    pub graph: Handle<AnimationGraph>,
    /// One entry per state that compiled, in the order the file declared them.
    states: Vec<BoundState>,
    /// Where each state's name lands in `states`.
    by_name: HashMap<String, usize>,
    /// The files these states drew their clips from.
    sources: Vec<String>,
}

impl AnimationGraphBound {
    /// The source, name and player node of the clip a state leads with, which
    /// for a blend is whichever of its clips currently carries the most weight.
    pub(crate) fn leading_clip_of(
        &self,
        player: &AnimationPlayer,
        state: &str,
    ) -> Option<(&str, &str, AnimationNodeIndex)> {
        let bound = self.states.get(*self.by_name.get(state)?)?;
        let clip = leading_clip(player, bound)?;
        Some((clip.source.as_str(), clip.clip.as_str(), clip.node))
    }

    /// The node a state plays, when the state compiled.
    pub fn node(&self, state: &str) -> Option<AnimationNodeIndex> {
        self.by_name
            .get(state)
            .map(|&index| self.states[index].node)
    }
}

/// Why an `.animgraph.bsn` file could not be read.
#[derive(Debug)]
pub enum AnimationGraphLoadError {
    /// The file could not be read off the disk.
    Io(String),
    /// The text is not BSN.
    Parse(String),
    /// The file holds no [`AnimationGraphDef`] root.
    NoGraph,
    /// A field of the graph does not answer to the type the registry holds.
    Fields,
    /// [`AnimationGraphDef`] is not in the type registry, so nothing can read
    /// the file's fields.
    Unregistered,
}

impl std::fmt::Display for AnimationGraphLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "could not read the graph: {err}"),
            Self::Parse(err) => write!(f, "could not parse the graph: {err}"),
            Self::NoGraph => write!(f, "the file holds no AnimationGraphDef"),
            Self::Fields => write!(f, "a field of the graph does not read as its type"),
            Self::Unregistered => write!(f, "AnimationGraphDef is not registered for reflection"),
        }
    }
}

impl std::error::Error for AnimationGraphLoadError {}

/// Reads `.animgraph.bsn` files into [`AnimationGraphAsset`].
#[derive(TypePath)]
pub struct AnimationGraphLoader {
    /// Shared with `AppTypeRegistry`, so a type registered after this loader
    /// was built is still one it can read.
    registry: TypeRegistryArc,
}

impl FromWorld for AnimationGraphLoader {
    fn from_world(world: &mut World) -> Self {
        Self {
            registry: world.resource::<AppTypeRegistry>().0.clone(),
        }
    }
}

impl AssetLoader for AnimationGraphLoader {
    type Asset = AnimationGraphAsset;
    type Settings = ();
    type Error = AnimationGraphLoadError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|err| AnimationGraphLoadError::Io(err.to_string()))?;
        let text = jackdaw_bsn::document_text_from_bytes(&bytes, load_context.path().path())
            .map_err(|err| AnimationGraphLoadError::Parse(err.to_string()))?;
        let def = parse_animation_graph(&text, &self.registry.read())?;
        Ok(AnimationGraphAsset { def })
    }

    fn extensions(&self) -> &[&str] {
        &["animgraph.bsn", "animgraph.bsb"]
    }
}

/// Read the [`AnimationGraphDef`] a BSN document carries as its root.
///
/// The fields go through the same reflected conversion a scene's components
/// take, so a field the file elides keeps whatever the type's `Default` gives
/// it.
pub fn parse_animation_graph(
    text: &str,
    registry: &TypeRegistry,
) -> Result<AnimationGraphDef, AnimationGraphLoadError> {
    let ast =
        parse_bsn_text(text).map_err(|err| AnimationGraphLoadError::Parse(err.to_string()))?;
    if registry.get(TypeId::of::<AnimationGraphDef>()).is_none() {
        return Err(AnimationGraphLoadError::Unregistered);
    }
    let wanted = AnimationGraphDef::type_path();
    for &root in &ast.roots {
        let Some(patches) = ast.get_patches(root) else {
            continue;
        };
        for &patch_entity in &patches.0 {
            match ast.get_patch(patch_entity) {
                Some(BsnPatch::Type(path)) if path == wanted => {
                    return Ok(AnimationGraphDef::default());
                }
                Some(BsnPatch::Struct(data)) if data.type_path == wanted => {
                    let value = bsn_value_to_reflect(
                        &BsnValue::Struct(data.clone()),
                        TypeId::of::<AnimationGraphDef>(),
                        registry,
                        None,
                    )
                    .ok_or(AnimationGraphLoadError::Fields)?;
                    return AnimationGraphDef::from_reflect(&*value)
                        .ok_or(AnimationGraphLoadError::Fields);
                }
                _ => {}
            }
        }
    }
    Err(AnimationGraphLoadError::NoGraph)
}

impl AnimationSet {
    /// The same states as a graph: one state per entry, `then` as a transition
    /// taken once the clip has run out, and the default state as the entry.
    pub fn to_graph(&self) -> AnimationGraphDef {
        let states = self
            .states
            .iter()
            .map(|def| AnimationGraphState {
                name: def.name.clone(),
                motion: AnimationMotion::Clip(AnimationClipRef {
                    source: self.sources.get(def.source).cloned().unwrap_or_default(),
                    clip: def.clip.clone(),
                }),
                looped: def.looped,
                speed: def.speed,
                ..AnimationGraphState::default()
            })
            .collect();
        let transitions = self
            .states
            .iter()
            .filter_map(|def| {
                let next = def.then.as_ref()?;
                let crossfade_secs = self
                    .states
                    .iter()
                    .find(|other| &other.name == next)
                    .map_or(def.transition_secs, |other| other.transition_secs);
                Some(AnimationTransitionDef {
                    from: def.name.clone(),
                    to: next.clone(),
                    exit_time: Some(1.0),
                    crossfade_secs,
                    ..AnimationTransitionDef::default()
                })
            })
            .collect();
        AnimationGraphDef {
            parameters: Vec::new(),
            states,
            transitions,
            entry: self.default_state.clone(),
        }
    }
}

/// Registers the graph types for reflection.
///
/// [`crate::AnimationRuntimePlugin`] calls this; call it directly only to
/// author graphs in an app that never plays them.
pub fn register_animation_graph_types(app: &mut App) {
    app.register_type::<AnimationGraphDef>()
        .register_type::<AnimationParameterDef>()
        .register_type::<AnimationParameterKind>()
        .register_type::<AnimationClipRef>()
        .register_type::<AnimationGraphState>()
        .register_type::<AnimationMotion>()
        .register_type::<AnimationBlendPoint>()
        .register_type::<AnimationTransitionDef>()
        .register_type::<AnimationCondition>()
        .register_type::<AnimationConditionOp>()
        .register_type::<AnimationGraphRef>()
        .register_type::<AnimationParams>()
        .register_type::<AnimationGraphPlayback>()
        .register_type::<AnimationGraphTransition>();
}

/// Set on an entity whose graph file failed to load, so the failure is
/// reported once rather than each frame the binding is retried.
#[derive(Component)]
pub(crate) struct AnimationGraphUnread;

/// Compiles every graph whose asset, clips and skeleton have all arrived.
///
/// Retried each frame rather than run on insertion, because the asset and the
/// glTF scene both land several frames after the component naming them.
pub(crate) fn bind_animation_graphs(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    assets: Res<Assets<AnimationGraphAsset>>,
    gltfs: Res<Assets<Gltf>>,
    clips: Res<Assets<AnimationClip>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    unbound: Query<
        (
            Entity,
            &AnimationGraphRef,
            Option<&AnimationGraphSource>,
            Option<&AnimationSources>,
            Option<&AnimationParams>,
            Option<&AnimationGraphUnread>,
        ),
        Without<AnimationGraphBound>,
    >,
    children: Query<&Children>,
    names: Query<&Name>,
) {
    for (entity, graph_ref, source, sources, params, unread) in &unbound {
        // The component left at its default names no file, and an entity
        // holding one is left to whatever else drives its skeleton.
        if graph_ref.path.is_empty() {
            continue;
        }
        let Some(source) = source else {
            commands
                .entity(entity)
                .insert(AnimationGraphSource(asset_server.load(&graph_ref.path)));
            continue;
        };
        let Some(def) = assets.get(&source.0).map(|asset| &asset.def) else {
            if unread.is_none() && asset_server.load_state(&source.0).is_failed() {
                warn!("animation graph `{}` could not be read", graph_ref.path);
                commands.entity(entity).insert(AnimationGraphUnread);
            }
            continue;
        };

        // Held handles stand in for the files only while they answer to the
        // graph's own source list one for one.
        let wanted = graph_sources(def);
        let Some(sources) = sources.filter(|held| held.0.len() == wanted.len()) else {
            commands.entity(entity).insert(AnimationSources(
                wanted.iter().map(|path| asset_server.load(path)).collect(),
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
        let files: HashMap<&str, &Gltf> = wanted
            .iter()
            .map(String::as_str)
            .zip(loaded.iter().copied())
            .collect();

        let Some(&root) =
            descendants_named(entity, &graph_ref.skeleton_root, &children, &names).first()
        else {
            continue;
        };

        let mut graph = AnimationGraph::new();
        let mut states = Vec::new();
        let mut by_name = HashMap::new();
        for state in &def.states {
            let Some(bound) = compile_state(state, &files, &clips, &mut graph) else {
                continue;
            };
            by_name.insert(state.name.clone(), states.len());
            states.push(bound);
        }
        let graph = graphs.add(graph);

        commands.queue(move |world: &mut World| {
            tag_animation_targets(world, root);
        });

        // A blend's clips run for as long as the graph does, so the mix can
        // move without restarting a clip mid-stride; what the state is worth
        // is the blend's own weight, written each frame.
        let mut player = AnimationPlayer::default();
        let mut transitions = AnimationTransitions::new();
        for state in states.iter().filter(|state| state.blend) {
            for clip in &state.clips {
                player
                    .play(clip.node)
                    .set_repeat(RepeatAnimation::Forever)
                    .set_speed(state.speed)
                    .set_weight(0.0);
            }
        }
        match by_name.get(&def.entry) {
            Some(&index) => enter_state(
                &states[index],
                Duration::ZERO,
                &mut player,
                &mut transitions,
            ),
            None if def.entry.is_empty() => {}
            None => warn!("animation graph has no entry state named `{}`", def.entry),
        }

        commands
            .entity(root)
            .insert((player, AnimationGraphHandle(graph.clone()), transitions));

        let mut params = params.cloned().unwrap_or_default();
        for parameter in &def.parameters {
            if parameter.kind != AnimationParameterKind::Trigger {
                params.seed(&parameter.name, parameter.default);
            }
        }
        commands.entity(entity).insert((
            params,
            AnimationGraphPlayback {
                state: def.entry.clone(),
                since_secs: 0.0,
                transition: None,
            },
            AnimationGraphBound {
                player: root,
                graph,
                states,
                by_name,
                sources: wanted,
            },
        ));
    }
}

/// Drops what an edited graph file compiled to, so the next binding is of
/// what the file now says.
///
/// The states are compiled once, while the transitions and parameters are read
/// from the asset each frame: without this, an edited file would run its new
/// moves against the states of the old one.
pub(crate) fn rebind_edited_graphs(
    mut commands: Commands,
    mut edits: MessageReader<AssetEvent<AnimationGraphAsset>>,
    assets: Res<Assets<AnimationGraphAsset>>,
    bound: Query<(Entity, &AnimationGraphSource, &AnimationGraphBound)>,
) {
    let edited: Vec<AssetId<AnimationGraphAsset>> = edits
        .read()
        .filter_map(|edit| match edit {
            AssetEvent::Modified { id } => Some(*id),
            _ => None,
        })
        .collect();
    if edited.is_empty() {
        return;
    }
    for (entity, source, bound) in &bound {
        if !edited.contains(&source.0.id()) {
            continue;
        }
        let mut entity = commands.entity(entity);
        entity.remove::<AnimationGraphBound>();
        // The held files are only reloaded when the edit moved a clip to
        // another file, so a graph handed its files rather than loading them
        // keeps them.
        if assets
            .get(&source.0)
            .is_some_and(|asset| graph_sources(&asset.def) != bound.sources)
        {
            entity.remove::<AnimationSources>();
        }
    }
}

/// Runs the state machine: takes whichever transition its conditions and its
/// exit time allow, then writes the weights the states in play ask for.
pub(crate) fn advance_animation_graphs(
    time: Res<Time>,
    assets: Res<Assets<AnimationGraphAsset>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut graphed: Query<(
        Entity,
        &AnimationGraphSource,
        &AnimationGraphBound,
        &mut AnimationParams,
        &mut AnimationGraphPlayback,
    )>,
    mut players: Query<(&mut AnimationPlayer, &mut AnimationTransitions)>,
    mut finished: MessageWriter<AnimationStateFinished>,
) {
    let delta = time.delta_secs();
    for (entity, source, bound, mut params, mut playback) in &mut graphed {
        let Some(def) = assets.get(&source.0).map(|asset| &asset.def) else {
            continue;
        };
        let Ok((mut player, mut transitions)) = players.get_mut(bound.player) else {
            continue;
        };

        let current = bound
            .by_name
            .get(&playback.state)
            .map(|&index| &bound.states[index]);
        if let Some(state) = current
            && !state.looped
            && ran_out(&player, state)
        {
            finished.write(AnimationStateFinished {
                entity,
                state: playback.state.clone(),
            });
        }

        let normalized = current.map_or(1.0, |state| normalized_time(&player, state));
        match pick_transition(def, bound, &params, &playback.state, normalized) {
            Some(taken) => {
                let transition = &def.transitions[taken];
                for condition in &transition.conditions {
                    if parameter_kind(def, &condition.parameter) == AnimationParameterKind::Trigger
                    {
                        params.clear_trigger(&condition.parameter);
                    }
                }
                enter_state(
                    &bound.states[bound.by_name[&transition.to]],
                    Duration::from_secs_f32(transition.crossfade_secs.max(0.0)),
                    &mut player,
                    &mut transitions,
                );
                playback.transition =
                    (transition.crossfade_secs > 0.0).then(|| AnimationGraphTransition {
                        from: playback.state.clone(),
                        remaining_secs: transition.crossfade_secs,
                    });
                playback.state = transition.to.clone();
                playback.since_secs = 0.0;
            }
            None => {
                playback.since_secs += delta;
                if let Some(fade) = &mut playback.transition {
                    fade.remaining_secs -= delta;
                    if fade.remaining_secs <= 0.0 {
                        playback.transition = None;
                    }
                }
            }
        }

        write_weights(bound, &params, &mut player, &mut graphs);
    }
}

/// The distinct glTF files a graph draws clips from, in the order they are
/// first named.
fn graph_sources(def: &AnimationGraphDef) -> Vec<String> {
    let mut sources: Vec<String> = Vec::new();
    for state in &def.states {
        for clip in motion_clips(&state.motion) {
            if !sources.contains(&clip.source) {
                sources.push(clip.source.clone());
            }
        }
    }
    sources
}

/// Every clip a motion names, a single clip included.
fn motion_clips(motion: &AnimationMotion) -> Vec<&AnimationClipRef> {
    match motion {
        AnimationMotion::Clip(clip) => vec![clip],
        AnimationMotion::Blend1d { points, .. } => points.iter().map(|point| &point.clip).collect(),
    }
}

/// Adds a state's clips to the Bevy graph, under a blend when the state mixes
/// several of them.
fn compile_state(
    state: &AnimationGraphState,
    files: &HashMap<&str, &Gltf>,
    clips: &Assets<AnimationClip>,
    graph: &mut AnimationGraph,
) -> Option<BoundState> {
    match &state.motion {
        AnimationMotion::Clip(clip_ref) => {
            let (handle, duration) = resolve_clip(&state.name, clip_ref, files, clips)?;
            let node = graph.add_clip(handle, 1.0, graph.root);
            Some(BoundState {
                node,
                blend: false,
                parameter: String::new(),
                looped: state.looped,
                speed: state.speed,
                clips: vec![BoundClip {
                    node,
                    threshold: 0.0,
                    duration,
                    source: clip_ref.source.clone(),
                    clip: clip_ref.clip.clone(),
                }],
            })
        }
        AnimationMotion::Blend1d { parameter, points } => {
            let blend = graph.add_blend(0.0, graph.root);
            let mut bound: Vec<BoundClip> = points
                .iter()
                .filter_map(|point| {
                    let (handle, duration) = resolve_clip(&state.name, &point.clip, files, clips)?;
                    Some(BoundClip {
                        node: graph.add_clip(handle, 1.0, blend),
                        threshold: point.threshold,
                        duration,
                        source: point.clip.source.clone(),
                        clip: point.clip.clip.clone(),
                    })
                })
                .collect();
            if bound.is_empty() {
                warn!("animation state `{}` blends no clips", state.name);
                return None;
            }
            bound.sort_by(|a, b| a.threshold.total_cmp(&b.threshold));
            Some(BoundState {
                node: blend,
                blend: true,
                parameter: parameter.clone(),
                looped: state.looped,
                speed: state.speed,
                clips: bound,
            })
        }
    }
}

/// The clip a reference names, with how long it runs.
fn resolve_clip(
    state: &str,
    clip_ref: &AnimationClipRef,
    files: &HashMap<&str, &Gltf>,
    clips: &Assets<AnimationClip>,
) -> Option<(Handle<AnimationClip>, f32)> {
    let Some(gltf) = files.get(clip_ref.source.as_str()) else {
        warn!(
            "animation state `{state}` wants clips from `{}`, which the graph does not hold",
            clip_ref.source
        );
        return None;
    };
    let Some(handle) = gltf.named_animations.get(clip_ref.clip.as_str()) else {
        let available: Vec<&str> = gltf.named_animations.keys().map(|name| &**name).collect();
        warn!(
            "animation state `{state}` wants clip `{}`, and its source holds {available:?}",
            clip_ref.clip
        );
        return None;
    };
    let duration = clips.get(handle).map_or(0.0, AnimationClip::duration);
    Some((handle.clone(), duration))
}

/// Starts a state, fading out whatever it replaces.
///
/// A blend's weight is read off the graph asset rather than the player, so a
/// blend faded in starts at nothing here and rides the fade the player runs;
/// one that replaces its predecessor outright takes the whole pose at once.
fn enter_state(
    bound: &BoundState,
    crossfade: Duration,
    player: &mut AnimationPlayer,
    transitions: &mut AnimationTransitions,
) {
    let repeat = if bound.looped {
        RepeatAnimation::Forever
    } else {
        RepeatAnimation::Never
    };
    let active = transitions.play(player, bound.node, crossfade);
    active.set_repeat(repeat).set_speed(bound.speed);
    if bound.blend {
        active.set_weight(if crossfade.is_zero() { 1.0 } else { 0.0 });
    }
}

/// The first transition out of `current` that both its conditions and its exit
/// time allow, as an index into the graph's own list.
fn pick_transition(
    def: &AnimationGraphDef,
    bound: &AnimationGraphBound,
    params: &AnimationParams,
    current: &str,
    normalized: f32,
) -> Option<usize> {
    def.transitions.iter().position(|transition| {
        (transition.from.is_empty() || transition.from == current)
            && transition.to != current
            && bound.by_name.contains_key(&transition.to)
            && transition
                .exit_time
                .is_none_or(|exit| normalized >= exit - EQUAL_EPSILON)
            && conditions_hold(def, params, transition)
    })
}

/// Whether a transition's conditions are met, as its `require_all` asks.
fn conditions_hold(
    def: &AnimationGraphDef,
    params: &AnimationParams,
    transition: &AnimationTransitionDef,
) -> bool {
    if transition.conditions.is_empty() {
        return true;
    }
    let mut holds = transition.conditions.iter().map(|condition| {
        if parameter_kind(def, &condition.parameter) == AnimationParameterKind::Trigger {
            params.triggered(&condition.parameter)
        } else {
            condition
                .op
                .holds(params.float(&condition.parameter), condition.value)
        }
    });
    if transition.require_all {
        holds.all(|held| held)
    } else {
        holds.any(|held| held)
    }
}

/// What a parameter holds, reading one the graph never declared as a float.
fn parameter_kind(def: &AnimationGraphDef, name: &str) -> AnimationParameterKind {
    def.parameters
        .iter()
        .find(|parameter| parameter.name == name)
        .map_or(AnimationParameterKind::Float, |parameter| parameter.kind)
}

/// How far through its clip a state has come, from zero to one.
fn normalized_time(player: &AnimationPlayer, bound: &BoundState) -> f32 {
    let Some(clip) = leading_clip(player, bound) else {
        return 1.0;
    };
    let Some(active) = player.animation(clip.node) else {
        return 1.0;
    };
    if active.is_finished() || active.just_completed() || clip.duration <= 0.0 {
        return 1.0;
    }
    (active.seek_time() / clip.duration).clamp(0.0, 1.0)
}

/// The clip of a state carrying the most weight, which is the one an exit time
/// is read off.
fn leading_clip<'a>(player: &AnimationPlayer, bound: &'a BoundState) -> Option<&'a BoundClip> {
    bound
        .clips
        .iter()
        .max_by(|a, b| clip_weight(player, a).total_cmp(&clip_weight(player, b)))
}

/// What a clip is currently worth on the player, or nothing when it is not
/// playing at all.
fn clip_weight(player: &AnimationPlayer, clip: &BoundClip) -> f32 {
    player
        .animation(clip.node)
        .map_or(0.0, ActiveAnimation::weight)
}

/// Whether a state's clip reached the end of its single run this tick.
fn ran_out(player: &AnimationPlayer, bound: &BoundState) -> bool {
    bound.clips.iter().any(|clip| {
        player
            .animation(clip.node)
            .is_some_and(|active| active.just_completed() && active.is_finished())
    })
}

/// Writes the weights the states in play ask for: the mix inside each blend,
/// and the share of the pose the blend itself takes.
fn write_weights(
    bound: &AnimationGraphBound,
    params: &AnimationParams,
    player: &mut AnimationPlayer,
    graphs: &mut Assets<AnimationGraph>,
) {
    let mut shares: Vec<(AnimationNodeIndex, f32)> = Vec::new();
    for state in bound.states.iter().filter(|state| state.blend) {
        shares.push((
            state.node,
            player
                .animation(state.node)
                .map_or(0.0, ActiveAnimation::weight),
        ));
        let mix = blend_weights(&state.clips, params.float(&state.parameter));
        for (clip, weight) in state.clips.iter().zip(mix) {
            if let Some(active) = player.animation_mut(clip.node) {
                active.set_weight(weight);
            }
        }
    }
    if shares.is_empty() {
        return;
    }
    // A blend's weight is read off the graph asset rather than the player, and
    // the topology never changes, so the write is untracked: no threading of
    // the graph has to be redone for a weight.
    let Some(graph) = graphs.get_mut_untracked(&bound.graph) else {
        return;
    };
    for (index, share) in shares {
        if let Some(node) = graph.get_mut(index)
            && (node.weight - share).abs() > WEIGHT_EPSILON
        {
            node.weight = share;
        }
    }
}

/// How much of each clip a 1D blend takes at `value`: all of the nearest clip
/// outside the thresholds, and a straight mix of the two it sits between.
fn blend_weights(clips: &[BoundClip], value: f32) -> Vec<f32> {
    let mut weights = vec![0.0; clips.len()];
    let Some(last) = clips.len().checked_sub(1) else {
        return weights;
    };
    if value <= clips[0].threshold {
        weights[0] = 1.0;
        return weights;
    }
    if value >= clips[last].threshold {
        weights[last] = 1.0;
        return weights;
    }
    for (index, pair) in clips.windows(2).enumerate() {
        let (low, high) = (pair[0].threshold, pair[1].threshold);
        if value < low || value > high {
            continue;
        }
        let span = high - low;
        let toward = if span > 0.0 {
            (value - low) / span
        } else {
            1.0
        };
        weights[index] = 1.0 - toward;
        weights[index + 1] = toward;
        break;
    }
    weights
}
