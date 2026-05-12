//! Polygon mesh storage. Layout follows the standard polygon-mesh shape used
//! by mature 3D modelers: parallel arrays of verts / edges / polys / loops
//! with per-element flags. Inspired by Blender's `Mesh` (architectural
//! reference); implementation original.

use std::borrow::Cow;
use std::collections::HashMap;

use bevy::math::{Vec2, Vec3};
use bevy::prelude::*;
use bevy::reflect::Reflect;
use bitflags::bitflags;
use serde::{Deserialize, Serialize};

use crate::BrushPlane;
use crate::newell::newell_normal;

#[derive(Reflect, Clone, Debug, Default, Serialize, Deserialize)]
pub struct BrushTopology {
    pub vertices: Vec<MeshVert>,
    pub edges: Vec<MeshEdge>,
    pub polygons: Vec<MeshPoly>,
    pub loops: Vec<MeshLoop>,
    pub attributes: AttributeStack,
}

#[derive(Reflect, Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct MeshVert {
    pub position: Vec3,
}

#[derive(Reflect, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct MeshEdge {
    pub v: [u32; 2],
    #[reflect(ignore)]
    pub flags: EdgeFlag,
}

impl Default for MeshEdge {
    fn default() -> Self {
        Self {
            v: [0, 0],
            flags: EdgeFlag::empty(),
        }
    }
}

#[derive(Reflect, Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct MeshPoly {
    pub loop_start: u32,
    pub loop_total: u32,
}

#[derive(Reflect, Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct MeshLoop {
    pub vert: u32,
    pub edge: u32,
}

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct EdgeFlag: u8 {
        const SELECT = 1 << 0;
        const HIDDEN = 1 << 1;
        const SHARP  = 1 << 2;
        const SEAM   = 1 << 3;
    }
}

impl Serialize for EdgeFlag {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.bits().serialize(s)
    }
}

impl<'de> Deserialize<'de> for EdgeFlag {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(EdgeFlag::from_bits_truncate(u8::deserialize(d)?))
    }
}

#[derive(Reflect, Clone, Debug, Default, Serialize, Deserialize)]
pub struct AttributeStack {
    pub vert: HashMap<Cow<'static, str>, AttributeData>,
    pub edge: HashMap<Cow<'static, str>, AttributeData>,
    #[serde(rename = "loop")]
    pub loop_: HashMap<Cow<'static, str>, AttributeData>,
    pub face: HashMap<Cow<'static, str>, AttributeData>,
}

#[derive(Reflect, Clone, Debug, Serialize, Deserialize)]
pub enum AttributeData {
    F32(Vec<f32>),
    Vec2(Vec<Vec2>),
    Vec3(Vec<Vec3>),
    Color(Vec<[f32; 4]>),
    U32(Vec<u32>),
}

impl BrushTopology {
    pub fn face_ring(&self, face_idx: usize) -> impl Iterator<Item = u32> + '_ {
        let poly = &self.polygons[face_idx];
        let start = poly.loop_start as usize;
        let total = poly.loop_total as usize;
        self.loops[start..start + total].iter().map(|l| l.vert)
    }

    pub fn edge_id(&self, v0: u32, v1: u32) -> Option<u32> {
        let (a, b) = if v0 <= v1 { (v0, v1) } else { (v1, v0) };
        self.edges
            .iter()
            .position(|e| e.v == [a, b])
            .map(|i| i as u32)
    }

    /// Compute normal of `face_idx` from its ring + a positions slice.
    /// Use this when you have a separately-cached positions slice.
    pub fn face_normal_with(&self, positions: &[Vec3], face_idx: usize) -> Vec3 {
        let ring: Vec<Vec3> = self
            .face_ring(face_idx)
            .map(|i| positions[i as usize])
            .collect();
        newell_normal(&ring)
    }

    pub fn face_normal(&self, face_idx: usize) -> Vec3 {
        let positions: Vec<Vec3> = self.vertices.iter().map(|v| v.position).collect();
        self.face_normal_with(&positions, face_idx)
    }

    pub fn face_centroid_with(&self, positions: &[Vec3], face_idx: usize) -> Vec3 {
        let mut sum = Vec3::ZERO;
        let mut count = 0u32;
        for vi in self.face_ring(face_idx) {
            sum += positions[vi as usize];
            count += 1;
        }
        if count == 0 {
            Vec3::ZERO
        } else {
            sum / count as f32
        }
    }

    pub fn face_centroid(&self, face_idx: usize) -> Vec3 {
        let positions: Vec<Vec3> = self.vertices.iter().map(|v| v.position).collect();
        self.face_centroid_with(&positions, face_idx)
    }

    pub fn face_plane(&self, face_idx: usize) -> BrushPlane {
        let positions: Vec<Vec3> = self.vertices.iter().map(|v| v.position).collect();
        let normal = self.face_normal_with(&positions, face_idx);
        let v0_idx = self.loops[self.polygons[face_idx].loop_start as usize].vert as usize;
        let distance = positions[v0_idx].dot(normal);
        BrushPlane { normal, distance }
    }
}
