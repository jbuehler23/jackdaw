//! UI material for rendering connection wires.
//!
//! Each `Connection` entity is paired with a child UI node that uses
//! [`ConnectionMaterial`]. A `PostUpdate` system updates `p0`..`p3` each frame
//! from the actual terminal positions; the shader computes a signed distance
//! to the cubic Bezier and anti-aliases it with `smoothstep(fwidth(d))`.
//!
//! Pattern mirrors `jackdaw_feathers::color_picker::materials`.

use bevy_asset::prelude::*;
use bevy_ecs::prelude::*;
use bevy_math::prelude::*;
use bevy_reflect::TypePath;
use bevy_render::render_resource::*;
use bevy_shader::ShaderRef;
use bevy_ui_render::prelude::*;

const SHADER_CONNECTION_PATH: &str = "embedded://jackdaw_node_graph/shaders/connection.wgsl";

/// GPU material for a single connection wire.
///
/// `p0`..`p3` are the four control points of a cubic Bezier in the local
/// pixel-space of the material node (i.e. relative to the UI node's top-left).
/// The canvas layout system chooses the node's size to be the bounding box of
/// the curve and positions it absolutely.
#[derive(AsBindGroup, Asset, TypePath, Debug, Clone)]
pub struct ConnectionMaterial {
    #[uniform(0)]
    pub p0: Vec2,
    #[uniform(0)]
    pub p1: Vec2,
    #[uniform(0)]
    pub p2: Vec2,
    #[uniform(0)]
    pub p3: Vec2,
    #[uniform(0)]
    pub color: Vec4,
    #[uniform(0)]
    pub width: f32,
    #[uniform(0)]
    pub feather: f32,
}

impl Default for ConnectionMaterial {
    fn default() -> Self {
        Self {
            p0: Vec2::ZERO,
            p1: Vec2::ZERO,
            p2: Vec2::ZERO,
            p3: Vec2::ZERO,
            color: Vec4::new(0.6, 0.6, 0.7, 1.0),
            width: 2.0,
            feather: 1.0,
        }
    }
}

impl UiMaterial for ConnectionMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER_CONNECTION_PATH.into()
    }
}
