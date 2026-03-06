use bevy::prelude::*;

pub const EPSILON: f32 = 1e-4;

#[derive(Clone, Debug, Reflect, Default)]
pub struct BrushPlane {
    pub normal: Vec3,
    pub distance: f32,
}

#[derive(Clone, Debug, Reflect, Default)]
pub struct BrushFaceData {
    pub plane: BrushPlane,
    pub material_index: usize,
    /// Asset-relative texture path (e.g. "textures/brick.png"). Overrides material_index when set.
    pub texture_path: Option<String>,
    /// References a MaterialDefinition by name. Takes priority over texture_path when set.
    pub material_name: Option<String>,
    pub uv_offset: Vec2,
    pub uv_scale: Vec2,
    pub uv_rotation: f32,
}

/// Solve the intersection of three planes. Returns None if degenerate.
pub fn plane_triple_intersection(
    p1: &BrushPlane,
    p2: &BrushPlane,
    p3: &BrushPlane,
) -> Option<Vec3> {
    let n1 = p1.normal;
    let n2 = p2.normal;
    let n3 = p3.normal;

    let det = n1.dot(n2.cross(n3));
    if det.abs() < EPSILON {
        return None;
    }

    let point =
        (n2.cross(n3) * p1.distance + n3.cross(n1) * p2.distance + n1.cross(n2) * p3.distance)
            / det;
    Some(point)
}

/// Check if a point is inside (or on the boundary of) all half-planes.
pub fn point_inside_all_planes(point: Vec3, faces: &[BrushFaceData]) -> bool {
    for face in faces {
        if face.plane.normal.dot(point) > face.plane.distance + EPSILON {
            return false;
        }
    }
    true
}

/// Compute brush geometry from face planes.
/// Returns (unique vertices, per-face polygon vertex indices).
pub fn compute_brush_geometry(faces: &[BrushFaceData]) -> (Vec<Vec3>, Vec<Vec<usize>>) {
    let n = faces.len();
    let mut vertices: Vec<Vec3> = Vec::new();

    // Find all valid intersection points from triples of planes
    for i in 0..n {
        for j in (i + 1)..n {
            for k in (j + 1)..n {
                if let Some(point) =
                    plane_triple_intersection(&faces[i].plane, &faces[j].plane, &faces[k].plane)
                {
                    // Keep only if inside all planes
                    if point_inside_all_planes(point, faces) {
                        // Deduplicate
                        let already = vertices.iter().any(|v| (*v - point).length() < EPSILON);
                        if !already {
                            vertices.push(point);
                        }
                    }
                }
            }
        }
    }

    // For each face, collect vertices that lie on that face and sort by winding
    let mut face_polygons = Vec::with_capacity(n);
    for face in faces {
        let mut face_verts: Vec<usize> = Vec::new();
        for (vi, v) in vertices.iter().enumerate() {
            let d = face.plane.normal.dot(*v) - face.plane.distance;
            if d.abs() < EPSILON {
                face_verts.push(vi);
            }
        }

        // Sort by winding order around face normal
        if face_verts.len() >= 3 {
            sort_face_vertices_by_winding(&vertices, &mut face_verts, face.plane.normal);
        }

        face_polygons.push(face_verts);
    }

    (vertices, face_polygons)
}

/// Sort face vertex indices by winding order around the face normal.
pub fn sort_face_vertices_by_winding(vertices: &[Vec3], indices: &mut [usize], normal: Vec3) {
    if indices.len() < 3 {
        return;
    }

    // Compute centroid of face vertices
    let centroid: Vec3 = indices.iter().map(|&i| vertices[i]).sum::<Vec3>() / indices.len() as f32;

    // Build a local 2D coordinate system on the face plane
    let (u_axis, v_axis) = compute_face_tangent_axes(normal);

    indices.sort_by(|&a, &b| {
        let da = vertices[a] - centroid;
        let db = vertices[b] - centroid;
        let angle_a = da.dot(v_axis).atan2(da.dot(u_axis));
        let angle_b = db.dot(v_axis).atan2(db.dot(u_axis));
        angle_a
            .partial_cmp(&angle_b)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Fan triangulation from vertex 0. Valid for convex polygons.
pub fn triangulate_face(indices: &[usize]) -> Vec<[u32; 3]> {
    let mut triangles = Vec::new();
    if indices.len() < 3 {
        return triangles;
    }
    for i in 1..(indices.len() - 1) {
        triangles.push([indices[0] as u32, indices[i] as u32, indices[i + 1] as u32]);
    }
    triangles
}

/// Compute tangent axes for a face from its normal (paraxial projection).
pub fn compute_face_tangent_axes(normal: Vec3) -> (Vec3, Vec3) {
    let abs_n = normal.abs();
    let up = if abs_n.y >= abs_n.x && abs_n.y >= abs_n.z {
        // Normal is mostly Y — use Z as reference
        Vec3::Z
    } else {
        Vec3::Y
    };
    let u = normal.cross(up).normalize_or_zero();
    let v = normal.cross(u).normalize_or_zero();
    (u, v)
}

/// Compute UVs for vertices on a face using paraxial projection.
pub fn compute_face_uvs(
    vertices: &[Vec3],
    indices: &[usize],
    normal: Vec3,
    uv_offset: Vec2,
    uv_scale: Vec2,
    uv_rotation: f32,
) -> Vec<[f32; 2]> {
    let (u_axis, v_axis) = compute_face_tangent_axes(normal);
    let cos_r = uv_rotation.cos();
    let sin_r = uv_rotation.sin();

    indices
        .iter()
        .map(|&vi| {
            let pos = vertices[vi];
            let u = pos.dot(u_axis);
            let v = pos.dot(v_axis);
            // Apply rotation
            let ru = u * cos_r - v * sin_r;
            let rv = u * sin_r + v * cos_r;
            // Apply scale and offset
            let su = ru / uv_scale.x.max(0.001) + uv_offset.x;
            let sv = rv / uv_scale.y.max(0.001) + uv_offset.y;
            [su, sv]
        })
        .collect()
}

/// Transform brush face planes from local space to world space.
pub fn brush_planes_to_world(
    faces: &[BrushFaceData],
    rotation: Quat,
    translation: Vec3,
) -> Vec<BrushFaceData> {
    faces
        .iter()
        .map(|face| {
            let world_normal = (rotation * face.plane.normal).normalize();
            let world_distance = face.plane.distance + world_normal.dot(translation);
            BrushFaceData {
                plane: BrushPlane {
                    normal: world_normal,
                    distance: world_distance,
                },
                material_index: face.material_index,
                texture_path: face.texture_path.clone(),
                material_name: face.material_name.clone(),
                uv_offset: face.uv_offset,
                uv_scale: face.uv_scale,
                uv_rotation: face.uv_rotation,
            }
        })
        .collect()
}

/// Check whether two convex volumes (defined by face planes) overlap.
pub fn brushes_intersect(a_faces: &[BrushFaceData], b_faces: &[BrushFaceData]) -> bool {
    let mut combined: Vec<BrushFaceData> = a_faces.to_vec();
    combined.extend_from_slice(b_faces);
    let (verts, _) = compute_brush_geometry(&combined);
    verts.len() >= 4
}

/// Subtract a cutter volume from a target brush. Both face sets must be in the same
/// coordinate space (typically world space). Returns the fragment face sets representing
/// the target minus the cutter.
pub fn subtract_brush(
    target_faces: &[BrushFaceData],
    cutter_faces: &[BrushFaceData],
) -> Vec<Vec<BrushFaceData>> {
    let mut result_fragments: Vec<Vec<BrushFaceData>> = Vec::new();
    let mut remaining: Vec<Vec<BrushFaceData>> = vec![target_faces.to_vec()];

    for cutter_face in cutter_faces {
        let n = cutter_face.plane.normal;
        let d = cutter_face.plane.distance;

        let mut next_remaining = Vec::new();

        for fragment in &remaining {
            // Outside half: keeps the part outside the cutter through this face
            let mut outside_faces = fragment.clone();
            outside_faces.push(BrushFaceData {
                plane: BrushPlane {
                    normal: -n,
                    distance: -d,
                },
                uv_scale: Vec2::ONE,
                ..default()
            });
            let (outside_verts, _) = compute_brush_geometry(&outside_faces);
            if outside_verts.len() >= 4 {
                result_fragments.push(outside_faces);
            }

            // Inside half: keeps the part inside the cutter through this face
            let mut inside_faces = fragment.clone();
            inside_faces.push(BrushFaceData {
                plane: BrushPlane {
                    normal: n,
                    distance: d,
                },
                uv_scale: Vec2::ONE,
                ..default()
            });
            let (inside_verts, _) = compute_brush_geometry(&inside_faces);
            if inside_verts.len() >= 4 {
                next_remaining.push(inside_faces);
            }
        }

        remaining = next_remaining;
    }

    // remaining = pieces fully inside the cutter → discard
    result_fragments
}

/// Remove faces that produce no vertices (degenerate) from a face set.
pub fn clean_degenerate_faces(faces: &[BrushFaceData]) -> Vec<BrushFaceData> {
    let (_, polys) = compute_brush_geometry(faces);
    faces
        .iter()
        .enumerate()
        .filter(|(i, _)| polys.get(*i).is_some_and(|p| p.len() >= 3))
        .map(|(_, f)| f.clone())
        .collect()
}
