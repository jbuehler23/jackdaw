//! Who owns the `AnimationPlayer` on an entity the editor is previewing on.
//!
//! A skeleton in an open scene is often already driven by a bound animation
//! set, which installed the player, the graph and the bone target ids. The
//! timeline transport and the clip preview both want that player for a while,
//! and neither may take away what the runtime put there. So both borrow it
//! through here: the borrow records what the target was playing, and returning
//! it puts that back.

use bevy::animation::{
    AnimatedBy, AnimationPlayer, AnimationTargetId, RepeatAnimation,
    graph::{AnimationGraph, AnimationGraphHandle, AnimationNodeIndex},
};
use bevy::prelude::*;

/// What a target's animation stack held before the editor borrowed it.
///
/// Editor-side state: it carries no `Reflect`, so it never reaches a saved
/// scene.
#[derive(Component, Debug)]
pub struct LoanedPlayer {
    /// The graph the target was playing, when it had one of its own.
    graph: Option<Handle<AnimationGraph>>,
    /// Whether the editor put the player there, so returning it takes it away.
    player_is_ours: bool,
    /// Entities the editor gave a target id to, to be untagged on return.
    tagged: Vec<Entity>,
}

/// What the editor asks a borrowed player to do.
pub struct PlayerLoan {
    /// The graph to play from while the loan lasts.
    pub graph: Handle<AnimationGraph>,
    /// Which node of that graph to run.
    pub node: AnimationNodeIndex,
    /// Where to start it.
    pub seek: f32,
    /// Whether it runs or holds the frame it is seeked to.
    pub playing: bool,
    /// Whether it repeats when it reaches the end.
    pub repeat: bool,
    /// Target id to write on the borrowed entity itself, for a clip authored
    /// against one named entity rather than a skeleton.
    pub self_target: Option<AnimationTargetId>,
    /// Entities the caller tagged to make this clip land, to be untagged when
    /// the loan ends.
    pub tagged: Vec<Entity>,
}

impl PlayerLoan {
    /// A loan of `graph`'s `node`, held at time zero and paused.
    pub fn new(graph: Handle<AnimationGraph>, node: AnimationNodeIndex) -> Self {
        Self {
            graph,
            node,
            seek: 0.0,
            playing: false,
            repeat: false,
            self_target: None,
            tagged: Vec::new(),
        }
    }

    /// Start the clip at `seek`, running or held.
    pub fn at(mut self, seek: f32, playing: bool) -> Self {
        self.seek = seek;
        self.playing = playing;
        self
    }

    /// Repeat the clip rather than stopping at its end.
    pub fn repeating(mut self, repeat: bool) -> Self {
        self.repeat = repeat;
        self
    }

    /// Write `id` on the borrowed entity, and name it as its own animated root.
    pub fn addressing_itself(mut self, id: AnimationTargetId) -> Self {
        self.self_target = Some(id);
        self
    }

    /// Untag these entities when the loan ends. See [`PlayerLoan::tagged`].
    pub fn untagging(mut self, tagged: Vec<Entity>) -> Self {
        self.tagged = tagged;
        self
    }
}

/// Point `target`'s player at the editor's graph, remembering what it held.
///
/// Installs a player when the target has none. Borrowing a target that is
/// already lent keeps the first record, so a second clip previewed on the same
/// skeleton still gives back what was there to begin with.
pub fn lend_player(world: &mut World, target: Entity, loan: PlayerLoan) {
    let Ok(mut entity) = world.get_entity_mut(target) else {
        return;
    };
    // A clip authored against one named entity addresses it by its own name,
    // but only where nothing has already said what this entity answers to.
    let self_tagged = match loan.self_target {
        Some(id) if !entity.contains::<AnimationTargetId>() => {
            entity.insert((id, AnimatedBy(target)));
            true
        }
        _ => false,
    };
    if !entity.contains::<LoanedPlayer>() {
        let graph = entity
            .get::<AnimationGraphHandle>()
            .map(|handle| handle.0.clone());
        let player_is_ours = !entity.contains::<AnimationPlayer>();
        let mut tagged = loan.tagged;
        if self_tagged {
            tagged.push(target);
        }
        entity.insert(LoanedPlayer {
            graph,
            player_is_ours,
            tagged,
        });
    }
    entity.insert(AnimationGraphHandle(loan.graph));
    if !entity.contains::<AnimationPlayer>() {
        entity.insert(AnimationPlayer::default());
    }
    let Some(mut player) = entity.get_mut::<AnimationPlayer>() else {
        return;
    };
    player.stop_all();
    let active = player.play(loan.node);
    active.seek_to(loan.seek);
    active.set_repeat(if loan.repeat {
        RepeatAnimation::Forever
    } else {
        RepeatAnimation::Never
    });
    if loan.playing {
        active.resume();
    } else {
        active.pause();
    }
}

/// Give `target`'s player back what it was playing, or take away the one the
/// editor installed.
///
/// Returns whether a graph was handed back, so a caller that knows what was
/// driving the target can ask it to start again.
pub fn return_player(world: &mut World, target: Entity) -> bool {
    let Ok(mut entity) = world.get_entity_mut(target) else {
        return false;
    };
    let Some(loan) = entity.take::<LoanedPlayer>() else {
        return false;
    };
    for tagged in &loan.tagged {
        if let Ok(mut tagged) = world.get_entity_mut(*tagged) {
            tagged.remove::<AnimationTargetId>();
            tagged.remove::<AnimatedBy>();
        }
    }
    let Ok(mut entity) = world.get_entity_mut(target) else {
        return false;
    };
    if loan.player_is_ours {
        entity.remove::<AnimationPlayer>();
        entity.remove::<AnimationGraphHandle>();
        entity.remove::<AnimationTargetId>();
        entity.remove::<AnimatedBy>();
        return false;
    }
    if let Some(mut player) = entity.get_mut::<AnimationPlayer>() {
        player.stop_all();
    }
    match loan.graph {
        Some(graph) => {
            entity.insert(AnimationGraphHandle(graph));
            true
        }
        None => {
            entity.remove::<AnimationGraphHandle>();
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph_with_one_clip(world: &mut World) -> (Handle<AnimationGraph>, AnimationNodeIndex) {
        let mut graph = AnimationGraph::new();
        let node = graph.add_clip(Handle::default(), 1.0, graph.root);
        let mut graphs = world.resource_mut::<Assets<AnimationGraph>>();
        (graphs.add(graph), node)
    }

    fn world_with_graphs() -> World {
        let mut world = World::new();
        world.insert_resource(Assets::<AnimationGraph>::default());
        world
    }

    #[test]
    fn a_target_with_no_player_keeps_none_after_the_loan() {
        let mut world = world_with_graphs();
        let target = world.spawn(Name::new("Armature")).id();
        let (graph, node) = graph_with_one_clip(&mut world);

        lend_player(&mut world, target, PlayerLoan::new(graph, node));
        assert!(world.get::<AnimationPlayer>(target).is_some());

        assert!(!return_player(&mut world, target));
        assert!(world.get::<AnimationPlayer>(target).is_none());
        assert!(world.get::<AnimationGraphHandle>(target).is_none());
    }

    #[test]
    fn a_target_already_playing_gets_its_own_graph_back() {
        let mut world = world_with_graphs();
        let (theirs, their_node) = graph_with_one_clip(&mut world);
        let target = world
            .spawn((
                Name::new("Armature"),
                AnimationPlayer::default(),
                AnimationGraphHandle(theirs.clone()),
            ))
            .id();
        world
            .get_mut::<AnimationPlayer>(target)
            .expect("the player just spawned")
            .play(their_node);

        let (ours, our_node) = graph_with_one_clip(&mut world);
        lend_player(&mut world, target, PlayerLoan::new(ours.clone(), our_node));
        assert_eq!(
            world.get::<AnimationGraphHandle>(target).map(|h| h.0.id()),
            Some(ours.id())
        );

        assert!(return_player(&mut world, target));
        assert_eq!(
            world.get::<AnimationGraphHandle>(target).map(|h| h.0.id()),
            Some(theirs.id()),
            "the graph the target was playing must come back"
        );
        assert!(world.get::<AnimationPlayer>(target).is_some());
    }

    #[test]
    fn returning_untags_only_what_the_loan_tagged() {
        let mut world = world_with_graphs();
        let target = world.spawn(Name::new("Armature")).id();
        let ours = world.spawn(Name::new("Bone")).id();
        let theirs = world
            .spawn((
                Name::new("Other"),
                AnimationTargetId::from_name(&"Other".into()),
            ))
            .id();
        let (graph, node) = graph_with_one_clip(&mut world);

        lend_player(
            &mut world,
            target,
            PlayerLoan::new(graph, node).untagging(vec![ours]),
        );
        world
            .entity_mut(ours)
            .insert(AnimationTargetId::from_name(&"Bone".into()));

        return_player(&mut world, target);
        assert!(world.get::<AnimationTargetId>(ours).is_none());
        assert!(
            world.get::<AnimationTargetId>(theirs).is_some(),
            "an id the loan did not write must survive it"
        );
    }
}
