//! Ordered modifier stack for a brush. Each entry wraps one modifier and
//! the flags that decide where it is evaluated (editor viewport, exported
//! mesh) and whether its output carries editable handles.

#[cfg(feature = "render")]
use bevy::prelude::ReflectComponent;
use glam::Vec3;

use crate::{BrushFaceData, EvaluatedBrush, MeshMirror, NO_SOURCE, evaluate_mirror};

/// Ordered list of modifiers applied to a brush. Evaluation folds the
/// authored geometry through each enabled entry in order.
#[derive(Clone, Default, Debug, PartialEq)]
#[cfg_attr(
    feature = "render",
    derive(bevy::ecs::component::Component, bevy::reflect::Reflect)
)]
#[cfg_attr(feature = "render", reflect(Component))]
pub struct ModifierStack {
    pub modifiers: Vec<ModifierEntry>,
}

impl ModifierStack {
    /// Payload of the first editor-enabled Mirror modifier, if any. The live
    /// drag clip rule and handle symmetry key off the mirror the viewport is
    /// showing, so they read the first enabled Mirror entry here.
    pub fn first_enabled_mirror(&self) -> Option<&MeshMirror> {
        self.modifiers
            .iter()
            .find_map(|entry| match &entry.modifier {
                Modifier::Mirror(mirror) if entry.enabled => Some(mirror),
                _ => None,
            })
    }

    /// Mutable payload of the first editor-enabled Mirror modifier, if any.
    /// The plane-drag / plane-set operators write `offset` through this.
    pub fn first_enabled_mirror_mut(&mut self) -> Option<&mut MeshMirror> {
        self.modifiers
            .iter_mut()
            .find_map(|entry| match &mut entry.modifier {
                Modifier::Mirror(mirror) if entry.enabled => Some(mirror),
                _ => None,
            })
    }
}

/// One modifier plus the flags controlling where it evaluates.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "render", derive(bevy::reflect::Reflect))]
pub struct ModifierEntry {
    pub modifier: Modifier,
    /// Evaluate for the editor viewport.
    pub enabled: bool,
    /// Evaluate for the exported / in-game mesh.
    pub in_game: bool,
    /// Draw editable handles at the modifier output.
    pub on_mesh: bool,
}

impl ModifierEntry {
    pub fn new(modifier: Modifier) -> Self {
        Self {
            modifier,
            enabled: true,
            in_game: true,
            on_mesh: true,
        }
    }
}

/// A single brush modifier.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "render", derive(bevy::reflect::Reflect))]
pub enum Modifier {
    Mirror(MeshMirror),
}

impl Modifier {
    pub fn kind_str(&self) -> &'static str {
        match self {
            Modifier::Mirror(_) => "mirror",
        }
    }

    pub fn from_kind(kind: &str) -> Option<Self> {
        match kind {
            "mirror" => Some(Modifier::Mirror(MeshMirror::default())),
            _ => None,
        }
    }
}

/// Fold `modifiers` over the base geometry in order. Each modifier consumes
/// the previous output. Returns the final geometry plus a source map from
/// every evaluated element back to its authored (base) origin. With no
/// modifiers the source maps are empty (identity).
pub fn evaluate_modifier_stack(
    base_vertices: &[Vec3],
    base_face_polygons: &[Vec<usize>],
    base_faces: &[BrushFaceData],
    modifiers: &[&Modifier],
) -> EvaluatedBrush {
    // Chain a source index through the running map; a NO_SOURCE marker (a bisect
    // cap or split vert) has no authored origin and passes straight through.
    fn remap_index(table: &[u32], prev: u32) -> u32 {
        if prev == NO_SOURCE {
            NO_SOURCE
        } else {
            table[prev as usize]
        }
    }

    let mut vertices = base_vertices.to_vec();
    let mut face_polygons = base_face_polygons.to_vec();
    let mut vert_to_base: Vec<u32> = (0..vertices.len() as u32).collect();
    let mut face_to_base: Vec<u32> = (0..face_polygons.len() as u32).collect();
    let mut faces: Vec<BrushFaceData> = base_faces.to_vec();

    let mut any = false;
    for modifier in modifiers {
        let step = match modifier {
            Modifier::Mirror(m) if !m.axes().is_empty() => {
                evaluate_mirror(&vertices, &face_polygons, m)
            }
            Modifier::Mirror(_) => continue,
        };
        any = true;
        // Chain each step's source maps through the running maps to the base index.
        let new_vert_to_base: Vec<u32> = step
            .vert_source
            .iter()
            .map(|&prev| remap_index(&vert_to_base, prev))
            .collect();
        let new_face_to_base: Vec<u32> = step
            .face_source
            .iter()
            .map(|&prev| remap_index(&face_to_base, prev))
            .collect();
        faces = step
            .face_source
            .iter()
            .map(|&prev| {
                if prev == NO_SOURCE {
                    BrushFaceData::default()
                } else {
                    faces.get(prev as usize).cloned().unwrap_or_default()
                }
            })
            .collect();
        vertices = step.vertices;
        face_polygons = step.face_polygons;
        vert_to_base = new_vert_to_base;
        face_to_base = new_face_to_base;
    }

    EvaluatedBrush {
        vertices,
        face_polygons,
        face_source: if any { face_to_base } else { Vec::new() },
        vert_source: if any { vert_to_base } else { Vec::new() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MirrorAxes;

    /// Open half-quad on the +X side with two verts ON the X plane, plus
    /// one `BrushFaceData` for its single face.
    fn half_quad() -> (Vec<Vec3>, Vec<Vec<usize>>, Vec<BrushFaceData>) {
        (
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
            ],
            vec![vec![0, 1, 2, 3]],
            vec![BrushFaceData::default()],
        )
    }

    fn mirror(axes: MirrorAxes) -> MeshMirror {
        MeshMirror {
            mirror_x: axes.contains(MirrorAxes::X),
            mirror_y: axes.contains(MirrorAxes::Y),
            mirror_z: axes.contains(MirrorAxes::Z),
            ..Default::default()
        }
    }

    #[test]
    fn first_enabled_mirror_skips_disabled_and_picks_the_first() {
        let mut disabled = ModifierEntry::new(Modifier::Mirror(mirror(MirrorAxes::X)));
        disabled.enabled = false;
        let enabled_y = ModifierEntry::new(Modifier::Mirror(mirror(MirrorAxes::Y)));
        let enabled_z = ModifierEntry::new(Modifier::Mirror(mirror(MirrorAxes::Z)));
        let stack = ModifierStack {
            modifiers: vec![disabled, enabled_y, enabled_z],
        };
        let picked = stack.first_enabled_mirror().expect("an enabled mirror");
        // The disabled X entry is skipped; the first enabled entry (Y) wins.
        assert!(picked.mirror_y);
        assert!(!picked.mirror_x);
        assert!(!picked.mirror_z);

        assert!(ModifierStack::default().first_enabled_mirror().is_none());
    }

    #[test]
    fn empty_stack_is_identity() {
        let (v, f, faces) = half_quad();
        let eval = evaluate_modifier_stack(&v, &f, &faces, &[]);
        assert_eq!(eval.vertices, v);
        assert_eq!(eval.face_polygons, f);
        assert!(eval.face_source.is_empty());
        assert!(eval.vert_source.is_empty());
    }

    #[test]
    fn single_mirror_matches_evaluate_mirror() {
        let (v, f, faces) = half_quad();
        let m = MeshMirror::default();
        let direct = evaluate_mirror(&v, &f, &m);
        let stack =
            evaluate_modifier_stack(&v, &f, &faces, &[&Modifier::Mirror(MeshMirror::default())]);
        assert_eq!(stack.vertices, direct.vertices);
        assert_eq!(stack.face_polygons, direct.face_polygons);
        assert_eq!(stack.face_source, direct.face_source);
        assert_eq!(stack.vert_source, direct.vert_source);
    }

    #[test]
    fn two_mirrors_compose_source_maps_back_to_base() {
        let (v, f, faces) = half_quad();
        let mods = [
            Modifier::Mirror(mirror(MirrorAxes::X)),
            Modifier::Mirror(mirror(MirrorAxes::Y)),
        ];
        let refs: Vec<&Modifier> = mods.iter().collect();
        let eval = evaluate_modifier_stack(&v, &f, &faces, &refs);

        let base_verts = v.len() as u32;
        let base_faces = f.len() as u32;
        assert!(
            eval.vert_source.iter().all(|&s| s < base_verts),
            "every vert source maps into the base vertex range"
        );
        assert!(
            eval.face_source.iter().all(|&s| s < base_faces),
            "every face source maps into the base face range"
        );
        // Identity prefix: the authored elements keep their own index.
        assert_eq!(eval.vert_source[0], 0);
        assert_eq!(eval.face_source[0], 0);
    }

    /// Axis-aligned cube spanning [-1, 1] with one `BrushFaceData` per face.
    fn cube() -> (Vec<Vec3>, Vec<Vec<usize>>, Vec<BrushFaceData>) {
        let verts = vec![
            Vec3::new(-1.0, -1.0, -1.0),
            Vec3::new(1.0, -1.0, -1.0),
            Vec3::new(1.0, 1.0, -1.0),
            Vec3::new(-1.0, 1.0, -1.0),
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(-1.0, 1.0, 1.0),
        ];
        let polys = vec![
            vec![0, 3, 2, 1],
            vec![4, 5, 6, 7],
            vec![0, 1, 5, 4],
            vec![3, 7, 6, 2],
            vec![0, 4, 7, 3],
            vec![1, 2, 6, 5],
        ];
        let faces = vec![BrushFaceData::default(); polys.len()];
        (verts, polys, faces)
    }

    fn bisecting_mirror() -> MeshMirror {
        MeshMirror {
            mirror_x: true,
            bisect: [true, false, false],
            ..Default::default()
        }
    }

    #[test]
    fn bisecting_mirror_passes_no_source_through_the_fold() {
        let (v, f, faces) = cube();
        let eval =
            evaluate_modifier_stack(&v, &f, &faces, &[&Modifier::Mirror(bisecting_mirror())]);
        // The cap's NO_SOURCE markers survive the fold without panicking on the
        // u32::MAX index.
        assert!(
            eval.vert_source.contains(&NO_SOURCE),
            "split-vert markers survive the stack fold"
        );
        assert!(
            eval.face_source.contains(&NO_SOURCE),
            "cap-face marker survives the stack fold"
        );
        // Authored elements still resolve to real base indices.
        assert!(
            eval.vert_source
                .iter()
                .filter(|&&s| s != NO_SOURCE)
                .all(|&s| (s as usize) < v.len()),
            "non-cap verts map into the base range"
        );
    }

    #[test]
    fn two_modifier_stack_with_leading_bisect_keeps_no_source() {
        let (v, f, faces) = cube();
        let mods = [
            Modifier::Mirror(bisecting_mirror()),
            Modifier::Mirror(mirror(MirrorAxes::Y)),
        ];
        let refs: Vec<&Modifier> = mods.iter().collect();
        // Composing a bisecting mirror with a second mirror must not panic on
        // the NO_SOURCE index and must keep cap-derived markers.
        let eval = evaluate_modifier_stack(&v, &f, &faces, &refs);
        assert!(
            eval.vert_source.contains(&NO_SOURCE),
            "cap-derived verts keep NO_SOURCE after a second modifier"
        );
        assert!(
            eval.face_source.contains(&NO_SOURCE),
            "cap-derived faces keep NO_SOURCE after a second modifier"
        );
        assert!(
            eval.vert_source
                .iter()
                .filter(|&&s| s != NO_SOURCE)
                .all(|&s| (s as usize) < v.len()),
            "authored verts still map into the base range"
        );
    }

    #[test]
    #[cfg(feature = "render")]
    fn modifier_stack_round_trips_through_reflection() {
        use bevy::reflect::{
            FromReflect, TypeRegistry,
            serde::{TypedReflectDeserializer, TypedReflectSerializer},
        };
        use serde::de::DeserializeSeed;

        let mut registry = TypeRegistry::default();
        registry.register::<ModifierStack>();
        registry.register::<ModifierEntry>();
        registry.register::<Modifier>();
        registry.register::<MeshMirror>();
        registry.register::<Vec3>();

        let original = ModifierStack {
            modifiers: vec![ModifierEntry::new(Modifier::Mirror(MeshMirror {
                mirror_x: true,
                mirror_y: true,
                merge: false,
                bisect: [true, false, false],
                ..Default::default()
            }))],
        };

        let serializer = TypedReflectSerializer::new(&original, &registry);
        let json = serde_json::to_string(&serializer).expect("serialize");

        let registration = registry
            .get(std::any::TypeId::of::<ModifierStack>())
            .expect("ModifierStack registered");
        let mut de = serde_json::Deserializer::from_str(&json);
        let reflected = TypedReflectDeserializer::new(registration, &registry)
            .deserialize(&mut de)
            .expect("deserialize");
        let back =
            ModifierStack::from_reflect(reflected.as_partial_reflect()).expect("from_reflect");
        assert_eq!(back, original);
    }
}
