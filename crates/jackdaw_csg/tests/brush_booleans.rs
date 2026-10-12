//! Representative brush booleans checked for volume, fragment count and
//! closed (watertight) polygon loops, plus a seeded random sweep of
//! grid-snapped and rotated pairs.

use std::collections::HashMap;

use glam::{Affine3A, DVec3, EulerRot, Quat, Vec3};
use jackdaw_csg::{
    BooleanOp, CsgBrush, CsgError, CsgInput, brush_batch_union, brush_boolean,
    brush_difference_split, brush_to_world,
};
use jackdaw_geometry::halfedge::{HalfedgeMesh, ops::edge_bevel::edge_bevel};
use jackdaw_scene_types::Brush;

fn cuboid(half: Vec3, center: Vec3) -> Brush {
    transformed(half, Affine3A::from_translation(center))
}

fn transformed(half: Vec3, tf: Affine3A) -> Brush {
    let brush = Brush::cuboid(half.x, half.y, half.z);
    let (faces, topology) = brush_to_world(&brush.faces, &brush.topology, tf);
    Brush { faces, topology }
}

fn beveled_cube(half: f32, width: f32) -> Brush {
    let brush = Brush::cuboid(half, half, half);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let edge = mesh.edges.keys().next().unwrap();
    edge_bevel(&mut mesh, &[edge], width).unwrap();
    let topology = mesh.flatten_to_topology();
    let mut faces = brush.faces;
    let template = faces.last().cloned().unwrap_or_default();
    while faces.len() < topology.polygons.len() {
        faces.push(template.clone());
    }
    for (i, face) in faces.iter_mut().enumerate() {
        if i < topology.polygons.len() {
            face.plane = topology.face_plane(i);
            face.ensure_uv_axes();
        }
    }
    Brush { faces, topology }
}

fn input(b: &Brush) -> CsgInput<'_> {
    CsgInput::new(&b.faces, &b.topology)
}

/// Volume and number of unpaired directed edges of a brush's polygon loops.
fn stats(b: &CsgBrush) -> (f64, usize) {
    let pos: Vec<DVec3> = b
        .topology
        .vertices
        .iter()
        .map(|v| v.position.as_dvec3())
        .collect();
    let mut volume = 0.0;
    let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
    for poly in &b.topology.polygons {
        let ring: Vec<u32> = (0..poly.loop_total)
            .map(|i| b.topology.loops[(poly.loop_start + i) as usize].vert)
            .collect();
        let p0 = pos[ring[0] as usize];
        for i in 0..ring.len() {
            let (u, v) = (ring[i], ring[(i + 1) % ring.len()]);
            if i >= 1 && i + 1 < ring.len() {
                volume += p0.dot(pos[u as usize].cross(pos[v as usize])) / 6.0;
            }
            *edges.entry((u, v)).or_default() += 1;
        }
    }
    let open = edges
        .iter()
        .filter(|((u, v), c)| edges.get(&(*v, *u)).copied().unwrap_or(0) != **c)
        .count();
    (volume, open)
}

fn close(v: f64, expected: f64) -> bool {
    (v - expected).abs() < 1e-3 * expected.abs().max(1.0)
}

#[derive(Clone, Copy)]
enum Op {
    Bool(BooleanOp),
    Split,
}

/// Fragments of `a op b`; an empty result is no fragments.
fn run(a: &Brush, b: &Brush, op: Op) -> Result<Vec<CsgBrush>, CsgError> {
    let result = match op {
        Op::Bool(op) => brush_boolean(&input(a), &input(b), op).map(|r| vec![r]),
        Op::Split => brush_difference_split(&input(a), &input(b)),
    };
    match result {
        Err(CsgError::EmptyResult) => Ok(Vec::new()),
        other => other,
    }
}

struct Case {
    name: &'static str,
    a: Brush,
    b: Brush,
    op: Op,
    /// Expected (volume, fragments); `None` when the kernel may reject it.
    expect: Option<(f64, usize)>,
}

fn cases() -> Vec<Case> {
    let unit = |c: Vec3| cuboid(Vec3::ONE, c);
    let rot = Affine3A::from_rotation_translation(Quat::from_rotation_y(0.5), Vec3::ZERO);
    let case = |name, a, b, op, expect| Case {
        name,
        a,
        b,
        op,
        expect,
    };
    vec![
        case(
            "corner bite",
            unit(Vec3::ZERO),
            unit(Vec3::ONE),
            Op::Split,
            Some((7.0, 1)),
        ),
        case(
            "tunnel",
            cuboid(Vec3::splat(2.0), Vec3::ZERO),
            cuboid(Vec3::new(1.0, 1.0, 4.0), Vec3::ZERO),
            Op::Bool(BooleanOp::Difference),
            Some((48.0, 1)),
        ),
        case(
            "overlap union",
            unit(Vec3::ZERO),
            unit(Vec3::new(1.0, 0.5, 0.0)),
            Op::Bool(BooleanOp::Union),
            Some((13.0, 1)),
        ),
        case(
            "touching union",
            unit(Vec3::ZERO),
            unit(Vec3::new(2.0, 0.0, 0.0)),
            Op::Bool(BooleanOp::Union),
            Some((16.0, 1)),
        ),
        case(
            "touching difference",
            unit(Vec3::ZERO),
            unit(Vec3::new(2.0, 0.0, 0.0)),
            Op::Split,
            Some((8.0, 1)),
        ),
        case(
            "coplanar top cut",
            unit(Vec3::ZERO),
            cuboid(Vec3::splat(0.5), Vec3::new(0.0, 0.5, 0.0)),
            Op::Split,
            Some((7.0, 1)),
        ),
        case(
            "slab split",
            unit(Vec3::ZERO),
            cuboid(Vec3::new(2.0, 0.1, 2.0), Vec3::ZERO),
            Op::Split,
            Some((7.2, 2)),
        ),
        case(
            "sliver 1e-3",
            unit(Vec3::ZERO),
            cuboid(Vec3::new(2.0, 2.0, 0.9995), Vec3::new(0.0, 0.0, -0.0005)),
            Op::Split,
            Some((0.004, 1)),
        ),
        case(
            "sliver 1e-5",
            unit(Vec3::ZERO),
            cuboid(
                Vec3::new(2.0, 2.0, 0.999995),
                Vec3::new(0.0, 0.0, -0.000005),
            ),
            Op::Split,
            None,
        ),
        case(
            "rotated cutter",
            cuboid(Vec3::splat(2.0), Vec3::ZERO),
            transformed(Vec3::new(0.5, 4.0, 0.5), rot),
            Op::Bool(BooleanOp::Difference),
            Some((60.0, 1)),
        ),
        case(
            "beveled minus cube",
            beveled_cube(1.0, 0.2),
            unit(Vec3::ONE),
            Op::Split,
            None,
        ),
        case(
            "intersection",
            unit(Vec3::ZERO),
            unit(Vec3::new(1.0, 1.0, 0.0)),
            Op::Bool(BooleanOp::Intersection),
            Some((2.0, 1)),
        ),
        case(
            "identical difference",
            unit(Vec3::ZERO),
            unit(Vec3::ZERO),
            Op::Split,
            Some((0.0, 0)),
        ),
        case(
            "identical union",
            unit(Vec3::ZERO),
            unit(Vec3::ZERO),
            Op::Bool(BooleanOp::Union),
            Some((8.0, 1)),
        ),
        case(
            "flat cutter",
            unit(Vec3::ZERO),
            cuboid(Vec3::new(2.0, 0.0, 2.0), Vec3::ZERO),
            Op::Split,
            None,
        ),
        case(
            "jittered corner bite",
            unit(Vec3::new(10.0, 0.5, -3.0) + Vec3::splat(1.0e-6)),
            unit(Vec3::new(11.0, 1.5, -2.0) - Vec3::splat(3.0e-7)),
            Op::Split,
            Some((7.0, 1)),
        ),
    ]
}

#[test]
fn named_cases_keep_volume_fragments_and_closed_loops() {
    let mut failures = Vec::new();
    for c in cases() {
        let fragments = match (run(&c.a, &c.b, c.op), c.expect) {
            (Ok(f), _) => f,
            (Err(_), None) => continue,
            (Err(e), Some(_)) => {
                failures.push(format!("{}: {e}", c.name));
                continue;
            }
        };
        let (mut volume, mut open) = (0.0, 0);
        for f in &fragments {
            let (v, o) = stats(f);
            volume += v;
            open += o;
        }
        if open != 0 {
            failures.push(format!("{}: {open} open edges", c.name));
        }
        if let Some((expected, count)) = c.expect
            && (!close(volume, expected) || fragments.len() != count)
        {
            failures.push(format!(
                "{}: volume {volume:.4} in {} fragments, expected {expected} in {count}",
                c.name,
                fragments.len()
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn repeated_through_slots_keep_one_closed_brush() {
    let mut target = cuboid(Vec3::new(4.0, 1.0, 4.0), Vec3::ZERO);
    for i in 0..12 {
        let cutter = cuboid(
            Vec3::new(0.2, 0.6, 5.0),
            Vec3::new(-3.5 + i as f32 * 0.6, 1.0, 0.0),
        );
        let mut fragments = brush_difference_split(&input(&target), &input(&cutter))
            .unwrap_or_else(|e| panic!("slot {i}: {e}"));
        assert_eq!(fragments.len(), 1, "slot {i}");
        let f = fragments.remove(0);
        target = Brush {
            faces: f.faces,
            topology: f.topology,
        };
    }
    let (volume, open) = stats(&CsgBrush {
        faces: target.faces,
        topology: target.topology,
    });
    assert_eq!(open, 0);
    let expected = 8.0 * 2.0 * 8.0 - 12.0 * 0.4 * 0.6 * 8.0;
    assert!(
        close(volume, expected),
        "volume {volume}, expected {expected}"
    );
}

/// Cuts twelve staggered pockets into the top of a slab, feeding each
/// result into the next cut, and returns the final brush.
fn pocketed_slab() -> Result<CsgBrush, String> {
    let mut target = cuboid(Vec3::new(4.0, 1.0, 4.0), Vec3::ZERO);
    for i in 0..12 {
        let center = Vec3::new(-3.5 + i as f32 * 0.6, 1.0, (i % 3) as f32 - 1.0);
        let cutter = cuboid(Vec3::new(0.2, 0.6, 2.0), center);
        let mut fragments = brush_difference_split(&input(&target), &input(&cutter))
            .map_err(|e| format!("pocket {i}: {e}"))?;
        if fragments.len() != 1 {
            return Err(format!("pocket {i}: {} fragments", fragments.len()));
        }
        let f = fragments.remove(0);
        target = Brush {
            faces: f.faces,
            topology: f.topology,
        };
    }
    Ok(CsgBrush {
        faces: target.faces,
        topology: target.topology,
    })
}

#[test]
fn repeated_pockets_give_the_same_closed_brush_every_run() {
    let first = pocketed_slab().unwrap();
    let (volume, open) = stats(&first);
    assert_eq!(open, 0);
    let expected = 8.0 * 2.0 * 8.0 - 12.0 * 0.4 * 0.6 * 4.0;
    assert!(
        close(volume, expected),
        "volume {volume}, expected {expected}"
    );
    for _ in 0..5 {
        let again = pocketed_slab().unwrap();
        assert_eq!(again.topology.polygons.len(), first.topology.polygons.len());
        assert_eq!(again.topology.vertices.len(), first.topology.vertices.len());
    }
}

#[test]
fn batch_union_of_overlapping_grid_is_one_closed_brush() {
    let brushes: Vec<Brush> = (0..9)
        .map(|i| {
            cuboid(
                Vec3::splat(0.75),
                Vec3::new((i % 3) as f32, 0.0, (i / 3) as f32),
            )
        })
        .collect();
    let inputs: Vec<CsgInput<'_>> = brushes.iter().map(input).collect();
    let (volume, open) = stats(&brush_batch_union(&inputs).unwrap());
    assert_eq!(open, 0);
    assert!(close(volume, 3.5 * 3.5 * 1.5), "volume {volume}");
}

/// Deterministic LCG so the sweep is the same on every run.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }

    /// Value in `[lo, hi]` snapped to a 0.25 grid, like editor snapping.
    fn snapped(&mut self, lo: f32, hi: f32) -> f32 {
        ((lo + self.next() * (hi - lo)) * 4.0).round() / 4.0
    }

    fn snapped_vec(&mut self, lo: f32, hi: f32) -> Vec3 {
        Vec3::new(
            self.snapped(lo, hi),
            self.snapped(lo, hi),
            self.snapped(lo, hi),
        )
    }
}

/// Total volume of `a op b`, or `None` when the kernel rejects the pair.
/// Panics when a fragment has open edges.
fn volume_of(a: &Brush, b: &Brush, op: Op, label: &str) -> Option<f64> {
    let fragments = run(a, b, op).ok()?;
    let mut volume = 0.0;
    for f in &fragments {
        let (v, open) = stats(f);
        assert_eq!(open, 0, "{label}: open edges");
        volume += v;
    }
    Some(volume)
}

#[test]
fn random_grid_snapped_pairs_stay_closed_and_conserve_volume() {
    let mut rng = Lcg(7);
    let mut checked = 0;
    for i in 0..200 {
        let a = cuboid(rng.snapped_vec(0.25, 2.0), rng.snapped_vec(-1.0, 1.0));
        let half = rng.snapped_vec(0.25, 2.0);
        let center = rng.snapped_vec(-1.5, 1.5);
        let b = if i % 3 == 0 {
            let q = Quat::from_euler(
                EulerRot::XYZ,
                rng.next() * 3.0,
                rng.next() * 3.0,
                rng.next() * 3.0,
            );
            transformed(half, Affine3A::from_rotation_translation(q, center))
        } else {
            cuboid(half, center)
        };
        let label = format!("pair {i}");
        let difference = volume_of(&a, &b, Op::Split, &label);
        let union = volume_of(&a, &b, Op::Bool(BooleanOp::Union), &label);
        let intersection = volume_of(&a, &b, Op::Bool(BooleanOp::Intersection), &label);
        let (Some(difference), Some(union), Some(intersection)) = (difference, union, intersection)
        else {
            continue;
        };
        let va = stats(&CsgBrush {
            faces: a.faces.clone(),
            topology: a.topology.clone(),
        })
        .0;
        let vb = stats(&CsgBrush {
            faces: b.faces.clone(),
            topology: b.topology.clone(),
        })
        .0;
        assert!(
            close(difference + intersection, va),
            "{label}: difference {difference} + intersection {intersection} != {va}"
        );
        assert!(
            close(union + intersection, va + vb),
            "{label}: union {union} + intersection {intersection} != {va} + {vb}"
        );
        checked += 1;
    }
    assert!(checked >= 190, "only {checked} of 200 pairs were accepted");
}
