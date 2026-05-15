use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

// Re-export geometry types so consumers see them from jackdaw_jsn
pub use jackdaw_geometry::{
    BrushFaceData, BrushPlane, BrushTopology, compute_brush_topology, compute_face_tangent_axes,
};

/// Groups multiple convex brush fragments produced by CSG subtraction.
/// Fragments become children of the group entity.
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component, Default, @crate::EditorHidden)]
pub struct BrushGroup;

/// Canonical brush data. Serialized. Geometry derived from this.
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component, Default, @crate::EditorCategory::new("Brush"), @crate::EditorHidden)]
pub struct Brush {
    pub faces: Vec<BrushFaceData>,
    /// Explicit half-edge topology. Empty for legacy brushes loaded from `.jsn` without topology
    /// data; they continue to use the plane-intersection path. New brushes built by constructors
    /// have both `faces` (planes) and `topology` populated in lockstep.
    pub topology: BrushTopology,
}

impl Brush {
    /// Create a cuboid brush from 6 axis-aligned face planes.
    ///
    /// Vertex layout (indices 0-7):
    ///   0: (-x, -y, -z)   1: (+x, -y, -z)   2: (+x, +y, -z)   3: (-x, +y, -z)
    ///   4: (-x, -y, +z)   5: (+x, -y, +z)   6: (+x, +y, +z)   7: (-x, +y, +z)
    ///
    /// Edge layout (canonical v[0] < v[1], indices 0-11):
    ///   0:(0,1) 1:(1,2) 2:(2,3) 3:(0,3) - bottom ring
    ///   4:(4,5) 5:(5,6) 6:(6,7) 7:(4,7) - top ring
    ///   8:(0,4) 9:(1,5) 10:(2,6) 11:(3,7) - verticals
    ///
    /// Face order matches the existing plane order: +X, -X, +Y, -Y, +Z, -Z.
    pub fn cuboid(half_x: f32, half_y: f32, half_z: f32) -> Self {
        use jackdaw_geometry::{MeshEdge, MeshLoop, MeshPoly, MeshVert};

        let (hx, hy, hz) = (half_x, half_y, half_z);

        // --- planes (existing order: +X, -X, +Y, -Y, +Z, -Z) ---
        let normals = [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ];
        let distances = [hx, hx, hy, hy, hz, hz];
        let faces: Vec<BrushFaceData> = normals
            .iter()
            .zip(distances.iter())
            .map(|(&normal, &distance)| {
                let (u, v) = compute_face_tangent_axes(normal);
                BrushFaceData {
                    plane: BrushPlane { normal, distance },
                    uv_scale: Vec2::ONE,
                    uv_u_axis: u,
                    uv_v_axis: v,
                    ..default()
                }
            })
            .collect();

        // --- topology ---
        //
        // Vertices (see doc comment for layout):
        let vertices = vec![
            MeshVert {
                position: Vec3::new(-hx, -hy, -hz),
            }, // 0
            MeshVert {
                position: Vec3::new(hx, -hy, -hz),
            }, // 1
            MeshVert {
                position: Vec3::new(hx, hy, -hz),
            }, // 2
            MeshVert {
                position: Vec3::new(-hx, hy, -hz),
            }, // 3
            MeshVert {
                position: Vec3::new(-hx, -hy, hz),
            }, // 4
            MeshVert {
                position: Vec3::new(hx, -hy, hz),
            }, // 5
            MeshVert {
                position: Vec3::new(hx, hy, hz),
            }, // 6
            MeshVert {
                position: Vec3::new(-hx, hy, hz),
            }, // 7
        ];

        // Edges (canonical v[0] < v[1]):
        //   bottom ring: 0:(0,1) 1:(1,2) 2:(2,3) 3:(0,3)
        //   top ring:    4:(4,5) 5:(5,6) 6:(6,7) 7:(4,7)
        //   verticals:   8:(0,4) 9:(1,5) 10:(2,6) 11:(3,7)
        let edges = vec![
            MeshEdge {
                v: [0, 1],
                ..default()
            }, //  0
            MeshEdge {
                v: [1, 2],
                ..default()
            }, //  1
            MeshEdge {
                v: [2, 3],
                ..default()
            }, //  2
            MeshEdge {
                v: [0, 3],
                ..default()
            }, //  3
            MeshEdge {
                v: [4, 5],
                ..default()
            }, //  4
            MeshEdge {
                v: [5, 6],
                ..default()
            }, //  5
            MeshEdge {
                v: [6, 7],
                ..default()
            }, //  6
            MeshEdge {
                v: [4, 7],
                ..default()
            }, //  7
            MeshEdge {
                v: [0, 4],
                ..default()
            }, //  8
            MeshEdge {
                v: [1, 5],
                ..default()
            }, //  9
            MeshEdge {
                v: [2, 6],
                ..default()
            }, // 10
            MeshEdge {
                v: [3, 7],
                ..default()
            }, // 11
        ];

        // Loops: each face has 4 loops (CCW from outside).
        // Loop layout - (vert, edge) pairs per face ring:
        //
        //   Face 0 (+X): verts 1,2,6,5 - edges 1,10,5,9
        //   Face 1 (-X): verts 0,4,7,3 - edges 8,7,11,3
        //   Face 2 (+Y): verts 2,3,7,6 - edges 2,11,6,10
        //   Face 3 (-Y): verts 0,1,5,4 - edges 0,9,4,8
        //   Face 4 (+Z): verts 4,5,6,7 - edges 4,5,6,7
        //   Face 5 (-Z): verts 0,3,2,1 - edges 3,2,1,0
        let loop_data: &[(u32, u32)] = &[
            // Face 0 (+X)
            (1, 1),
            (2, 10),
            (6, 5),
            (5, 9),
            // Face 1 (-X)
            (0, 8),
            (4, 7),
            (7, 11),
            (3, 3),
            // Face 2 (+Y)
            (2, 2),
            (3, 11),
            (7, 6),
            (6, 10),
            // Face 3 (-Y)
            (0, 0),
            (1, 9),
            (5, 4),
            (4, 8),
            // Face 4 (+Z)
            (4, 4),
            (5, 5),
            (6, 6),
            (7, 7),
            // Face 5 (-Z)
            (0, 3),
            (3, 2),
            (2, 1),
            (1, 0),
        ];
        let loops: Vec<MeshLoop> = loop_data
            .iter()
            .map(|&(vert, edge)| MeshLoop { vert, edge })
            .collect();

        // Polygons: each face has loop_start and loop_total = 4.
        let polygons: Vec<MeshPoly> = (0..6u32)
            .map(|i| MeshPoly {
                loop_start: i * 4,
                loop_total: 4,
            })
            .collect();

        let topology = BrushTopology {
            vertices,
            edges,
            polygons,
            loops,
            ..default()
        };

        Self { faces, topology }
    }

    /// Create a prism brush from a polygon base and extrusion depth along a normal.
    ///
    /// `vertices` are the polygon vertices in local space (must be coplanar, convex, >= 3).
    /// `normal` is the extrusion direction (unit vector, perpendicular to the polygon plane).
    /// `depth` is the total extrusion distance (can be negative; absolute value is used).
    ///
    /// The brush is centered at the origin: the polygon base sits at -|depth|/2 along the normal,
    /// and the top cap sits at +|depth|/2.
    ///
    /// Face order: top cap (index 0), bottom cap (index 1), then N side quads (indices 2..2+N).
    /// Topology vertices: base ring (0..N), then top ring (N..2N).
    ///
    /// Returns `None` if fewer than 3 vertices or zero depth.
    pub fn prism(vertices: &[Vec3], normal: Vec3, depth: f32) -> Option<Self> {
        use jackdaw_geometry::{MeshEdge, MeshLoop, MeshPoly, MeshVert};

        if vertices.len() < 3 || depth.abs() < 1e-6 {
            return None;
        }

        let n = vertices.len();
        let half_depth = depth.abs() / 2.0;
        let mut faces = Vec::new();

        // Top cap: faces outward along +normal
        let (top_u, top_v) = compute_face_tangent_axes(normal);
        faces.push(BrushFaceData {
            plane: BrushPlane {
                normal,
                distance: half_depth,
            },
            uv_scale: Vec2::ONE,
            uv_u_axis: top_u,
            uv_v_axis: top_v,
            ..default()
        });

        // Bottom cap: faces outward along -normal
        let (bot_u, bot_v) = compute_face_tangent_axes(-normal);
        faces.push(BrushFaceData {
            plane: BrushPlane {
                normal: -normal,
                distance: half_depth,
            },
            uv_scale: Vec2::ONE,
            uv_u_axis: bot_u,
            uv_v_axis: bot_v,
            ..default()
        });

        // Side planes: one for each edge of the polygon
        let centroid: Vec3 = vertices.iter().sum::<Vec3>() / n as f32;
        let mut valid_side_indices: Vec<usize> = Vec::new();
        for i in 0..n {
            let a = vertices[i];
            let b = vertices[(i + 1) % n];
            let edge = b - a;
            let side_normal = edge.cross(normal).normalize_or_zero();
            if side_normal.length_squared() < 0.5 {
                continue;
            }

            // Ensure outward-facing: dot with (vertex - centroid) should be positive
            let side_normal = if side_normal.dot(a - centroid) < 0.0 {
                -side_normal
            } else {
                side_normal
            };
            let distance = side_normal.dot(a);
            let (su, sv) = compute_face_tangent_axes(side_normal);
            faces.push(BrushFaceData {
                plane: BrushPlane {
                    normal: side_normal,
                    distance,
                },
                uv_scale: Vec2::ONE,
                uv_u_axis: su,
                uv_v_axis: sv,
                ..default()
            });
            valid_side_indices.push(i);
        }

        if faces.len() < 4 {
            return None;
        }

        // --- topology ---
        //
        // Vertices: base ring at offset = -normal * half_depth, top ring at +normal * half_depth.
        // Base ring: indices 0..n
        // Top ring:  indices n..2n
        let mut topo_verts: Vec<MeshVert> = Vec::with_capacity(2 * n);
        for i in 0..n {
            topo_verts.push(MeshVert {
                position: vertices[i] - normal * half_depth,
            });
        }
        for i in 0..n {
            topo_verts.push(MeshVert {
                position: vertices[i] + normal * half_depth,
            });
        }

        // Edges:
        //   base ring edges:    0..n - edge i connects base[i] -> base[(i+1)%n]
        //   top ring edges:     n..2n - edge n+i connects top[i] -> top[(i+1)%n]
        //   vertical edges:     2n..3n - edge 2n+i connects base[i] -> top[i]
        let mut topo_edges: Vec<MeshEdge> = Vec::with_capacity(3 * n);
        for i in 0..n {
            let a = i as u32;
            let b = ((i + 1) % n) as u32;
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            topo_edges.push(MeshEdge {
                v: [lo, hi],
                ..default()
            });
        }
        for i in 0..n {
            let a = (n + i) as u32;
            let b = (n + (i + 1) % n) as u32;
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            topo_edges.push(MeshEdge {
                v: [lo, hi],
                ..default()
            });
        }
        for i in 0..n {
            topo_edges.push(MeshEdge {
                v: [i as u32, (n + i) as u32],
                ..default()
            });
        }

        // Loops and polygons (face order: top cap, bottom cap, then sides).
        // total loops = n (top cap) + n (bottom cap) + n_sides * 4
        let n_sides = valid_side_indices.len();
        let total_loops = n + n + n_sides * 4;
        let mut topo_loops: Vec<MeshLoop> = Vec::with_capacity(total_loops);
        let mut topo_polys: Vec<MeshPoly> = Vec::new();

        // Face 0 - top cap: top ring CCW looking along +normal (from outside).
        // Top ring verts: n, n+1, ..., n+(n-1). Edge for loop[i] is the top ring edge n+i.
        {
            let loop_start = topo_loops.len() as u32;
            for i in 0..n {
                let vert = (n + i) as u32;
                let edge = (n + i) as u32; // top ring edge i
                topo_loops.push(MeshLoop { vert, edge });
            }
            topo_polys.push(MeshPoly {
                loop_start,
                loop_total: n as u32,
            });
        }

        // Face 1 - bottom cap: base ring CW when looking along +normal = CCW from below (-normal).
        // Use reversed base ring: n-1, n-2, ..., 0.
        {
            let loop_start = topo_loops.len() as u32;
            for i in (0..n).rev() {
                let vert = i as u32;
                let edge = i as u32; // base ring edge i
                topo_loops.push(MeshLoop { vert, edge });
            }
            topo_polys.push(MeshPoly {
                loop_start,
                loop_total: n as u32,
            });
        }

        // Side faces: one quad per valid edge index.
        // Side i (polygon base edge at valid_side_indices[si]):
        //   verts: base[i], base[i+1], top[i+1], top[i]  (CCW from outside)
        //   edges: base_ring[i], vert[i+1], top_ring[i], vert[i]
        for &i in &valid_side_indices {
            let j = (i + 1) % n;
            let loop_start = topo_loops.len() as u32;
            // base[i] -> edge: base ring edge i
            topo_loops.push(MeshLoop {
                vert: i as u32,
                edge: i as u32,
            });
            // base[j] -> edge: vertical j
            topo_loops.push(MeshLoop {
                vert: j as u32,
                edge: (2 * n + j) as u32,
            });
            // top[j] -> edge: top ring edge j (reversed direction, but we store the edge index)
            topo_loops.push(MeshLoop {
                vert: (n + j) as u32,
                edge: (n + j) as u32,
            });
            // top[i] -> edge: vertical i
            topo_loops.push(MeshLoop {
                vert: (n + i) as u32,
                edge: (2 * n + i) as u32,
            });
            topo_polys.push(MeshPoly {
                loop_start,
                loop_total: 4,
            });
        }

        let topology = BrushTopology {
            vertices: topo_verts,
            edges: topo_edges,
            polygons: topo_polys,
            loops: topo_loops,
            ..default()
        };

        Some(Self { faces, topology })
    }

    /// Create a sphere brush approximated as an icosahedron (20 triangular faces).
    pub fn sphere(radius: f32) -> Self {
        let phi = (1.0 + 5.0_f32.sqrt()) / 2.0;
        let raw = [
            Vec3::new(-1.0, phi, 0.0),
            Vec3::new(1.0, phi, 0.0),
            Vec3::new(-1.0, -phi, 0.0),
            Vec3::new(1.0, -phi, 0.0),
            Vec3::new(0.0, -1.0, phi),
            Vec3::new(0.0, 1.0, phi),
            Vec3::new(0.0, -1.0, -phi),
            Vec3::new(0.0, 1.0, -phi),
            Vec3::new(phi, 0.0, -1.0),
            Vec3::new(phi, 0.0, 1.0),
            Vec3::new(-phi, 0.0, -1.0),
            Vec3::new(-phi, 0.0, 1.0),
        ];
        let verts: Vec<Vec3> = raw.iter().map(|v| v.normalize() * radius).collect();

        // 20 triangular faces (standard icosahedron topology)
        let tris: [[usize; 3]; 20] = [
            [0, 11, 5],
            [0, 5, 1],
            [0, 1, 7],
            [0, 7, 10],
            [0, 10, 11],
            [1, 5, 9],
            [5, 11, 4],
            [11, 10, 2],
            [10, 7, 6],
            [7, 1, 8],
            [3, 9, 4],
            [3, 4, 2],
            [3, 2, 6],
            [3, 6, 8],
            [3, 8, 9],
            [4, 9, 5],
            [2, 4, 11],
            [6, 2, 10],
            [8, 6, 7],
            [9, 8, 1],
        ];

        let faces: Vec<BrushFaceData> = tris
            .iter()
            .map(|&[a, b, c]| {
                let normal = (verts[b] - verts[a]).cross(verts[c] - verts[a]).normalize();
                let distance = normal.dot(verts[a]);
                // Ensure outward-facing
                let (normal, distance) = if distance < 0.0 {
                    (-normal, -distance)
                } else {
                    (normal, distance)
                };
                let (u, v) = compute_face_tangent_axes(normal);
                BrushFaceData {
                    plane: BrushPlane { normal, distance },
                    uv_scale: Vec2::ONE,
                    uv_u_axis: u,
                    uv_v_axis: v,
                    ..default()
                }
            })
            .collect();

        let topology = compute_brush_topology(&faces);
        Self { faces, topology }
    }
}

#[derive(Component, Reflect, Default, Clone, Debug, Deref, DerefMut)]
#[reflect(Component, Default, @crate::EditorHidden)]
pub struct CustomProperties {
    pub properties: BTreeMap<String, PropertyValue>,
}

/// One enum for every editor parameter value: runtime
/// `OperatorParameters`, const operator schemas (`Operator::PARAMETERS`),
/// concrete button-call params, and reflected `CustomProperties`
/// fields. `String` uses `Cow<'static, str>` so the enum can sit in a
/// `const` slice.
#[derive(Reflect, Clone, Debug, PartialEq)]
pub enum PropertyValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(Cow<'static, str>),
    Vec2(Vec2),
    Vec3(Vec3),
    Color(Color),
    Entity(Entity),
}

impl From<bool> for PropertyValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for PropertyValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<f64> for PropertyValue {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<String> for PropertyValue {
    fn from(value: String) -> Self {
        Self::String(Cow::Owned(value))
    }
}

impl From<&'static str> for PropertyValue {
    fn from(value: &'static str) -> Self {
        Self::String(Cow::Borrowed(value))
    }
}

impl From<Cow<'static, str>> for PropertyValue {
    fn from(value: Cow<'static, str>) -> Self {
        Self::String(value)
    }
}

impl From<Vec2> for PropertyValue {
    fn from(value: Vec2) -> Self {
        Self::Vec2(value)
    }
}

impl From<Vec3> for PropertyValue {
    fn from(value: Vec3) -> Self {
        Self::Vec3(value)
    }
}

impl From<Color> for PropertyValue {
    fn from(value: Color) -> Self {
        Self::Color(value)
    }
}

impl From<Entity> for PropertyValue {
    fn from(value: Entity) -> Self {
        Self::Entity(value)
    }
}

impl std::fmt::Display for PropertyValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bool(b) => write!(f, "{b}"),
            Self::Int(i) => write!(f, "{i}"),
            Self::Float(x) => write!(f, "{x}"),
            Self::String(s) => write!(f, "\"{s}\""),
            Self::Vec2(v) => write!(f, "vec2({}, {})", v.x, v.y),
            Self::Vec3(v) => write!(f, "vec3({}, {}, {})", v.x, v.y, v.z),
            Self::Color(c) => {
                let s = c.to_srgba();
                write!(
                    f,
                    "Color::srgba({}, {}, {}, {})",
                    s.red, s.green, s.blue, s.alpha
                )
            }
            Self::Entity(e) => write!(f, "Entity({})", e.to_bits()),
        }
    }
}

impl PropertyValue {
    /// Canonical title-case type name (`"Bool"`, `"Int"`, `"Float"`,
    /// `"String"`, `"Vec2"`, `"Vec3"`, `"Color"`, `"Entity"`). Used by
    /// the Custom Properties picker, operator-signature tooltips, and
    /// matched against `ParamSpec::ty` (in `jackdaw_api_internal`).
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Bool(_) => "Bool",
            Self::Int(_) => "Int",
            Self::Float(_) => "Float",
            Self::String(_) => "String",
            Self::Vec2(_) => "Vec2",
            Self::Vec3(_) => "Vec3",
            Self::Color(_) => "Color",
            Self::Entity(_) => "Entity",
        }
    }

    /// Default value for the given [`type_name`](Self::type_name)
    /// string. Used by the Custom Properties picker.
    pub fn default_for_type(name: &str) -> Option<Self> {
        match name {
            "Bool" => Some(Self::Bool(false)),
            "Int" => Some(Self::Int(0)),
            "Float" => Some(Self::Float(0.0)),
            "String" => Some(Self::String(Cow::Borrowed(""))),
            "Vec2" => Some(Self::Vec2(Vec2::ZERO)),
            "Vec3" => Some(Self::Vec3(Vec3::ZERO)),
            "Color" => Some(Self::Color(Color::WHITE)),
            "Entity" => Some(Self::Entity(Entity::PLACEHOLDER)),
            _ => None,
        }
    }

    /// All available type names for the UI picker, derived from one
    /// default per variant. Adding a new `PropertyValue` variant only
    /// requires updating [`type_name`](Self::type_name); this list and
    /// the picker pick it up automatically.
    pub fn all_type_names() -> &'static [&'static str] {
        const NAMES: &[&str] = &[
            PropertyValue::Bool(false).type_name(),
            PropertyValue::Int(0).type_name(),
            PropertyValue::Float(0.0).type_name(),
            PropertyValue::String(Cow::Borrowed("")).type_name(),
            PropertyValue::Vec2(Vec2::ZERO).type_name(),
            PropertyValue::Vec3(Vec3::ZERO).type_name(),
            PropertyValue::Color(Color::WHITE).type_name(),
            PropertyValue::Entity(Entity::PLACEHOLDER).type_name(),
        ];
        NAMES
    }
}

#[derive(Component, Reflect, Clone)]
#[reflect(Component, @crate::EditorHidden)]
pub struct GltfSource {
    pub path: String,
    pub scene_index: usize,
}

/// Tracks the source `.jsn` file for a prefab instance.
#[derive(Component, Reflect, Clone, Debug, Default, Serialize, Deserialize)]
#[reflect(Component, Default, @crate::EditorHidden)]
pub struct JsnPrefab {
    pub path: String,
}

/// Stores the original serialized component values from a prefab at instantiation time.
/// Used to detect overrides and support per-component revert.
#[derive(Component, Clone, Debug, Default)]
pub struct JsnPrefabBaseline {
    pub components: HashMap<String, serde_json::Value>,
}

#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component, Default, @crate::EditorCategory::new("Navmesh"), @crate::EditorHidden)]
pub struct NavmeshRegion {
    pub agent_radius: f32,
    pub agent_height: f32,
    pub walkable_climb: f32,
    pub walkable_slope_degrees: f32,
    pub cell_size_fraction: f32,
    pub cell_height_fraction: f32,
    pub min_region_size: u16,
    pub merge_region_size: u16,
    pub max_simplification_error: f32,
    pub max_vertices_per_polygon: u16,
    pub edge_max_len_factor: u16,
    pub detail_sample_dist: f32,
    pub detail_sample_max_error: f32,
    pub tiling: bool,
    pub tile_size: u16,
    pub connection_url: String,
}

/// Terrain heightmap component. Stores all data needed for serialization.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component, Default, @crate::EditorCategory::new("Terrain"), @crate::EditorHidden)]
pub struct Terrain {
    /// Vertices per edge.
    pub resolution: u32,
    /// World-space XZ dimensions.
    pub size: Vec2,
    /// Maximum height value for normalization.
    pub max_height: f32,
    /// Row-major height data, length = resolution^2.
    pub heights: Vec<f32>,
}

impl Default for Terrain {
    fn default() -> Self {
        let resolution = 256;
        Self {
            resolution,
            size: Vec2::new(100.0, 100.0),
            max_height: 50.0,
            heights: vec![0.0; (resolution * resolution) as usize],
        }
    }
}

impl Default for NavmeshRegion {
    fn default() -> Self {
        Self {
            agent_radius: 0.6,
            agent_height: 2.0,
            walkable_climb: 0.9,
            walkable_slope_degrees: 45.0,
            cell_size_fraction: 2.0,
            cell_height_fraction: 4.0,
            min_region_size: 8,
            merge_region_size: 20,
            max_simplification_error: 1.3,
            max_vertices_per_polygon: 6,
            edge_max_len_factor: 8,
            detail_sample_dist: 6.0,
            detail_sample_max_error: 1.0,
            tiling: false,
            tile_size: 32,
            connection_url: "http://127.0.0.1:15702".to_string(),
        }
    }
}
