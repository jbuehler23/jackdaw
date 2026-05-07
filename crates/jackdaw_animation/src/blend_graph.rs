//! Blend-graph authoring: an alternative clip source using the node
//! canvas instead of keyframe tracks.
//!
//! A blend graph clip carries `Clip` + `AnimationBlendGraph` +
//! `NodeGraph` + `GraphCanvasView`. Its children are `GraphNode` and
//! `Connection` entities rather than `AnimationTrack` / keyframes.
//!
//! Four node types are registered: `anim.clip_ref` (references
//! another clip), `anim.blend`, `anim.additive`, and `anim.output`.
//! Currently only single-clip passthrough compiles (`ClipRef` -> Output).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Marker on a `Clip` entity whose source is a node-canvas blend tree.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, Default)]
#[reflect(Component, Serialize, Deserialize, @jackdaw_jsn::EditorHidden)]
pub struct AnimationBlendGraph;

/// Body component for an `anim.clip_ref` node. References another
/// `Clip` entity whose compiled handle feeds into this graph.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone)]
#[reflect(Component, Serialize, Deserialize, @jackdaw_jsn::EditorHidden)]
pub struct ClipNodeRef {
    pub clip_entity: Entity,
}

impl Default for ClipNodeRef {
    fn default() -> Self {
        Self {
            clip_entity: Entity::PLACEHOLDER,
        }
    }
}

/// Body component for an `anim.blend` node. Linear blend between
/// `a` and `b`; the `weight` terminal is a compile-time constant if
/// not connected, otherwise driven by the incoming scalar curve.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, Copy)]
#[reflect(Component, Serialize, Deserialize, @jackdaw_jsn::EditorHidden)]
pub struct BlendNode {
    pub weight: f32,
}

impl Default for BlendNode {
    fn default() -> Self {
        Self { weight: 0.5 }
    }
}

/// Body component for an `anim.additive` node. Adds `add` on top of
/// `base` with intensity `weight`.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, Copy)]
#[reflect(Component, Serialize, Deserialize, @jackdaw_jsn::EditorHidden)]
pub struct AdditiveBlendNode {
    pub weight: f32,
}

impl Default for AdditiveBlendNode {
    fn default() -> Self {
        Self { weight: 1.0 }
    }
}

/// Body component for `anim.output`. One per graph; compile walks
/// back from here.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, Default)]
#[reflect(Component, Serialize, Deserialize, @jackdaw_jsn::EditorHidden)]
pub struct OutputNode;

/// Register the four animation node types with `NodeTypeRegistry`.
pub fn register_animation_node_types(mut registry: ResMut<jackdaw_node_graph::NodeTypeRegistry>) {
    use jackdaw_node_graph::{NodeTypeDescriptor, TerminalDescriptor};

    const POSE: &str = "anim.pose";
    const SCALAR: &str = "anim.scalar";
    let pose_color = Color::srgb(0.95, 0.70, 0.30);
    let scalar_color = Color::srgb(0.55, 0.80, 0.95);
    let category = "Animation".to_string();

    let pose_out = || TerminalDescriptor {
        label: "pose".into(),
        data_type: POSE.into(),
        color: pose_color,
    };
    let pose_in = |label: &str| TerminalDescriptor {
        label: label.into(),
        data_type: POSE.into(),
        color: pose_color,
    };
    let scalar_in = |label: &str| TerminalDescriptor {
        label: label.into(),
        data_type: SCALAR.into(),
        color: scalar_color,
    };

    registry.register(NodeTypeDescriptor {
        id: "anim.clip_ref".into(),
        display_name: "Clip Reference".into(),
        category: category.clone(),
        accent_color: Color::srgb(0.38, 0.72, 1.0),
        inputs: vec![],
        outputs: vec![pose_out()],
        body_components: vec!["jackdaw_animation::blend_graph::ClipNodeRef".into()],
    });

    registry.register(NodeTypeDescriptor {
        id: "anim.blend".into(),
        display_name: "Blend".into(),
        category: category.clone(),
        accent_color: Color::srgb(0.55, 0.80, 0.95),
        inputs: vec![pose_in("a"), pose_in("b"), scalar_in("weight")],
        outputs: vec![pose_out()],
        body_components: vec!["jackdaw_animation::blend_graph::BlendNode".into()],
    });

    registry.register(NodeTypeDescriptor {
        id: "anim.additive".into(),
        display_name: "Additive Blend".into(),
        category: category.clone(),
        accent_color: Color::srgb(0.75, 0.60, 0.95),
        inputs: vec![pose_in("base"), pose_in("add"), scalar_in("weight")],
        outputs: vec![pose_out()],
        body_components: vec!["jackdaw_animation::blend_graph::AdditiveBlendNode".into()],
    });

    registry.register(NodeTypeDescriptor {
        id: "anim.output".into(),
        display_name: "Output".into(),
        category,
        accent_color: Color::srgb(0.95, 0.50, 0.40),
        inputs: vec![pose_in("pose")],
        outputs: vec![],
        body_components: vec!["jackdaw_animation::blend_graph::OutputNode".into()],
    });
}
