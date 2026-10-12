//! The mesh boolean kernel behind the brush pipeline: the pure-Rust
//! `manifold-rust` port of manifold3d. The rest of the crate reaches the
//! kernel only through this module.

use manifold_rust::manifold::Manifold;
use manifold_rust::types::{Error, MeshGL64, OpType};

use crate::BooleanOp;

/// A closed, oriented triangle mesh owned by the kernel.
pub(crate) type Solid = Manifold;

/// Build a solid from flat `xyz` positions and triangle indices, with
/// `tolerance` as the merge and coplanarity distance.
pub(crate) fn from_mesh(positions: &[f64], tris: &[u64], tolerance: f64) -> Result<Solid, String> {
    let mesh = MeshGL64 {
        num_prop: 3,
        vert_properties: positions.to_vec(),
        tri_verts: tris.to_vec(),
        ..Default::default()
    };
    let solid = Manifold::from_mesh_gl64(&mesh);
    match solid.status() {
        Error::NoError => Ok(solid.set_tolerance(tolerance)),
        e => Err(format!("{e:?}")),
    }
}

pub(crate) fn boolean(a: &Solid, b: &Solid, op: BooleanOp) -> Solid {
    match op {
        BooleanOp::Union => a.union(b),
        BooleanOp::Difference => a.difference(b),
        BooleanOp::Intersection => a.intersection(b),
    }
}

pub(crate) fn batch_union(solids: &[Solid]) -> Solid {
    Manifold::batch_boolean(solids, OpType::Add)
}

pub(crate) fn is_empty(solid: &Solid) -> bool {
    solid.is_empty()
}

/// Positions as flat `xyz` and triangle indices.
pub(crate) fn to_mesh(solid: &Solid) -> (Vec<f64>, Vec<u64>) {
    let mesh = solid.get_mesh_gl64(-1);
    let nprops = mesh.num_prop as usize;
    let positions = if nprops == 3 {
        mesh.vert_properties
    } else {
        mesh.vert_properties
            .chunks(nprops)
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect()
    };
    (positions, mesh.tri_verts)
}

/// Connected components of `solid`.
pub(crate) fn decompose(solid: &Solid) -> Vec<Solid> {
    solid.decompose()
}
