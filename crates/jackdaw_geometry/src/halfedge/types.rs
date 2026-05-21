use bevy_math::Vec3;
use bitflags::bitflags;
use slotmap::{SlotMap, new_key_type};

new_key_type! {
    pub struct VertKey;
    pub struct EdgeKey;
    pub struct LoopKey;
    pub struct FaceKey;
}

#[derive(Default, Clone)]
pub struct HalfedgeMesh {
    pub verts: SlotMap<VertKey, HalfedgeVert>,
    pub edges: SlotMap<EdgeKey, HalfedgeEdge>,
    pub loops: SlotMap<LoopKey, HalfedgeLoop>,
    pub faces: SlotMap<FaceKey, HalfedgeFace>,
}

impl HalfedgeMesh {
    pub fn vert_count(&self) -> usize {
        self.verts.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn loop_count(&self) -> usize {
        self.loops.len()
    }

    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    pub fn add_vert(&mut self, co: Vec3) -> VertKey {
        self.verts.insert(HalfedgeVert {
            co,
            flag: VertFlag::empty(),
            edge: None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct HalfedgeVert {
    pub co: Vec3,
    pub flag: VertFlag,
    pub edge: Option<EdgeKey>,
}

#[derive(Clone, Debug)]
pub struct HalfedgeEdge {
    pub v: [VertKey; 2],
    pub flag: EdgeFlag,
    pub loop_first: Option<LoopKey>,
    pub disk_next: [EdgeKey; 2],
    pub disk_prev: [EdgeKey; 2],
}

#[derive(Clone, Debug)]
pub struct HalfedgeLoop {
    pub vert: VertKey,
    pub edge: EdgeKey,
    pub face: FaceKey,
    pub next: LoopKey,
    pub prev: LoopKey,
    pub radial_next: LoopKey,
    pub radial_prev: LoopKey,
}

#[derive(Clone, Debug)]
pub struct HalfedgeFace {
    pub flag: FaceFlag,
    pub material_idx: u32,
    pub loop_first: LoopKey,
    pub loop_count: u32,
    pub normal_cache: Vec3,
}

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct VertFlag: u8 {
        const SELECT = 1 << 0;
        const HIDDEN = 1 << 1;
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct EdgeFlag: u8 {
        const SELECT = 1 << 0;
        const HIDDEN = 1 << 1;
        const SHARP = 1 << 2;
        const SEAM = 1 << 3;
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct FaceFlag: u8 {
        const SELECT = 1 << 0;
        const HIDDEN = 1 << 1;
    }
}
