use std::f32::consts::FRAC_PI_2;

use crate::brush::{self, BrushMeshCache};
use crate::entity_ops::EmptyEntity;
use crate::selection::Selected;
use crate::viewport::{AxisIndicator, SceneViewport};
use crate::{JackdawDrawSystems, default_style};
use avian3d::parry::transformation::convex_hull;
use bevy::prelude::*;
use bevy::ui::widget::ViewportNode;
use jackdaw_jsn::BrushGroup;

#[derive(Component)]
struct AxisLabel;

/// Per-viewport storage for the X/Y/Z axis labels. One of these
/// components is attached to every `SceneViewport` entity so labels
/// stay scoped to the panel they belong to.
#[derive(Component)]
struct ViewportAxisLabels {
    labels: [Entity; 3],
}

pub struct ViewportOverlaysPlugin;

impl Plugin for ViewportOverlaysPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OverlaySettings>()
            .add_systems(
                Update,
                ensure_axis_labels.run_if(in_state(crate::AppState::Editor)),
            )
            .add_systems(
                PostUpdate,
                draw_selection_bounding_boxes.in_set(JackdawDrawSystems),
            )
            .add_systems(
                PostUpdate,
                (
                    draw_point_light_gizmo,
                    draw_spot_light_gizmo,
                    draw_dir_light_gizmo,
                    draw_camera_gizmo,
                    draw_empty_entity_marker,
                )
                    .after(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate)
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_systems(
                PostUpdate,
                (draw_coordinate_indicator, draw_navmesh_region_bounds).in_set(JackdawDrawSystems),
            );
    }
}

#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum BoundingBoxMode {
    /// Simple axis-aligned bounding box (12 edges).
    #[default]
    Aabb,
    /// Full convex hull wireframe showing all geometry edges.
    ConvexHull,
}

#[derive(Resource, Clone, PartialEq)]
pub struct OverlaySettings {
    pub show_bounding_boxes: bool,
    pub show_coordinate_indicator: bool,
    pub bounding_box_mode: BoundingBoxMode,
    pub show_face_grid: bool,
    /// Whether all visible brushes should show a wireframe outline.
    pub show_brush_wireframe: bool,
    /// Whether all visible brushes should show an outline.
    /// Note that regardless of this setting, the current selection will always show an outline.
    pub show_brush_outline: bool,
    pub show_alignment_guides: bool,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        Self {
            show_bounding_boxes: false,
            show_coordinate_indicator: true,
            bounding_box_mode: BoundingBoxMode::default(),
            show_face_grid: false,
            show_brush_wireframe: false,
            show_brush_outline: true,
            show_alignment_guides: true,
        }
    }
}

/// Draw bounding boxes around selected entities with geometry.
fn draw_selection_bounding_boxes(
    mut gizmos: Gizmos,
    settings: Res<OverlaySettings>,
    selected: Query<
        (
            Entity,
            &GlobalTransform,
            Option<&BrushMeshCache>,
            &InheritedVisibility,
        ),
        With<Selected>,
    >,
    children_query: Query<&Children>,
    mesh_query: Query<(&Mesh3d, &GlobalTransform)>,
    meshes: Res<Assets<Mesh>>,
    parents: Query<&ChildOf>,
    brush_groups: Query<(), With<BrushGroup>>,
) {
    let color = default_style::SELECTION_BBOX;

    for (entity, global_tf, maybe_brush_cache, inherited_vis) in &selected {
        if !inherited_vis.get() {
            continue;
        }
        // Brushes get their selection highlight via face-material
        // swap (`ensure_brush_face_materials`), so their AABB is
        // gated behind the explicit "show bounding boxes" overlay
        // setting to avoid double visual feedback. Non-brush mesh
        // entities (Cube/Sphere prefabs, user gltf imports, etc.)
        // have no other selection cue and ALWAYS draw the AABB.
        let is_brush = maybe_brush_cache.is_some();
        if is_brush && !settings.show_bounding_boxes {
            continue;
        }
        // Skip children of BrushGroups (the group itself gets a bounding box)
        if parents
            .get(entity)
            .is_ok_and(|c| brush_groups.contains(c.0))
        {
            continue;
        }
        // Collect world-space vertices
        let world_verts = if let Some(cache) = maybe_brush_cache {
            if cache.vertices.is_empty() {
                continue;
            }
            match settings.bounding_box_mode {
                BoundingBoxMode::ConvexHull => {
                    // Convex hull mode for brushes: use face polygons directly
                    let verts: Vec<Vec3> = cache
                        .vertices
                        .iter()
                        .map(|v| global_tf.transform_point(*v))
                        .collect();
                    draw_hull_wireframe(&mut gizmos, &verts, &cache.face_polygons, color);
                    continue;
                }
                BoundingBoxMode::Aabb => cache
                    .vertices
                    .iter()
                    .map(|v| global_tf.transform_point(*v))
                    .collect::<Vec<Vec3>>(),
            }
        } else {
            let mut verts = Vec::new();
            collect_descendant_mesh_world_vertices(
                entity,
                &children_query,
                &mesh_query,
                &meshes,
                &mut verts,
            );
            if verts.is_empty() {
                continue;
            }
            verts
        };

        match settings.bounding_box_mode {
            BoundingBoxMode::Aabb => {
                let (min, max) = aabb_from_points(&world_verts);
                draw_aabb_wireframe(&mut gizmos, min, max, color);
            }
            BoundingBoxMode::ConvexHull => {
                // parry 0.26 (avian 0.6) takes / returns plain `Vec3`.
                let (hull_positions, hull_tris) = convex_hull(&world_verts);
                if hull_positions.is_empty() || hull_tris.is_empty() {
                    continue;
                }

                let hull_faces = brush::merge_hull_triangles(&hull_positions, &hull_tris);
                let face_polygons: Vec<Vec<usize>> =
                    hull_faces.into_iter().map(|f| f.vertex_indices).collect();
                draw_hull_wireframe(&mut gizmos, &hull_positions, &face_polygons, color);
            }
        }
    }
}

/// Compute axis-aligned bounding box from a set of points.
pub(crate) fn aabb_from_points(points: &[Vec3]) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for &p in points {
        min = min.min(p);
        max = max.max(p);
    }
    (min, max)
}

/// Draw 12 edges of an axis-aligned bounding box.
fn draw_aabb_wireframe(gizmos: &mut Gizmos, min: Vec3, max: Vec3, color: Color) {
    let corners = [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ];
    // Bottom face
    gizmos.line(corners[0], corners[1], color);
    gizmos.line(corners[1], corners[2], color);
    gizmos.line(corners[2], corners[3], color);
    gizmos.line(corners[3], corners[0], color);
    // Top face
    gizmos.line(corners[4], corners[5], color);
    gizmos.line(corners[5], corners[6], color);
    gizmos.line(corners[6], corners[7], color);
    gizmos.line(corners[7], corners[4], color);
    // Vertical edges
    gizmos.line(corners[0], corners[4], color);
    gizmos.line(corners[1], corners[5], color);
    gizmos.line(corners[2], corners[6], color);
    gizmos.line(corners[3], corners[7], color);
}

/// Draw unique edges from face polygons as wireframe lines.
fn draw_hull_wireframe(
    gizmos: &mut Gizmos,
    world_verts: &[Vec3],
    face_polygons: &[Vec<usize>],
    color: Color,
) {
    let mut drawn: Vec<(usize, usize)> = Vec::new();
    for polygon in face_polygons {
        if polygon.len() < 2 {
            continue;
        }
        for i in 0..polygon.len() {
            let a = polygon[i];
            let b = polygon[(i + 1) % polygon.len()];
            let edge = (a.min(b), a.max(b));
            if !drawn.contains(&edge) {
                drawn.push(edge);
                gizmos.line(world_verts[edge.0], world_verts[edge.1], color);
            }
        }
    }
}

/// Recursively collect world-space vertex positions from Mesh3d components.
pub(crate) fn collect_descendant_mesh_world_vertices(
    entity: Entity,
    children_query: &Query<&Children>,
    mesh_query: &Query<(&Mesh3d, &GlobalTransform)>,
    meshes: &Assets<Mesh>,
    out: &mut Vec<Vec3>,
) {
    if let Ok((mesh3d, global_tf)) = mesh_query.get(entity)
        && let Some(mesh) = meshes.get(&mesh3d.0)
        && let Some(positions) = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|attr| attr.as_float3())
    {
        for pos in positions {
            out.push(global_tf.transform_point(Vec3::from_array(*pos)));
        }
    }
    if let Ok(children) = children_query.get(entity) {
        for child in children.iter() {
            collect_descendant_mesh_world_vertices(child, children_query, mesh_query, meshes, out);
        }
    }
}

/// Bright bounding-box color when selected, dim marker color otherwise.
fn marker_color(is_selected: bool) -> Color {
    if is_selected {
        default_style::SELECTION_BBOX
    } else {
        default_style::ENTITY_MARKER_UNSELECTED
    }
}

/// Point light: three axis-aligned circles at range radius. Filtered
/// by the [`SceneLight`](crate::entity_ops::SceneLight) marker so
/// editor-local lights (e.g. the material-preview rig) stay out of
/// the main viewport.
fn draw_point_light_gizmo(
    mut gizmos: Gizmos,
    settings: Res<OverlaySettings>,
    query: Query<
        (
            Entity,
            &PointLight,
            &GlobalTransform,
            &InheritedVisibility,
            Has<Selected>,
        ),
        With<crate::entity_ops::SceneLight>,
    >,
) {
    if !settings.show_bounding_boxes {
        return;
    }
    for (_entity, light, tf, inherited_vis, selected) in &query {
        if !inherited_vis.get() {
            continue;
        }
        let color = marker_color(selected);
        let pos = tf.translation();
        gizmos.circle(
            Isometry3d::new(pos, Quat::from_rotation_x(FRAC_PI_2)),
            light.range,
            color,
        );
        gizmos.circle(Isometry3d::new(pos, Quat::IDENTITY), light.range, color);
        gizmos.circle(
            Isometry3d::new(pos, Quat::from_rotation_y(FRAC_PI_2)),
            light.range,
            color,
        );
    }
}

/// Spot light cone: `outer_angle` and `range`.
fn draw_spot_light_gizmo(
    mut gizmos: Gizmos,
    settings: Res<OverlaySettings>,
    query: Query<
        (
            &SpotLight,
            &GlobalTransform,
            &InheritedVisibility,
            Has<Selected>,
        ),
        With<crate::entity_ops::SceneLight>,
    >,
) {
    if !settings.show_bounding_boxes {
        return;
    }
    for (light, tf, inherited_vis, selected) in &query {
        if !inherited_vis.get() {
            continue;
        }
        let color = marker_color(selected);
        let pos = tf.translation();
        let fwd = tf.forward().as_vec3();
        let right = tf.right().as_vec3();
        let up = tf.up().as_vec3();
        let r = light.range * light.outer_angle.tan();
        let tip = pos + fwd * light.range;
        // Circle at cone end
        gizmos.circle(
            Isometry3d::new(tip, tf.compute_transform().rotation),
            r,
            color,
        );
        // 4 lines from origin to circle edges
        gizmos.line(pos, tip + right * r, color);
        gizmos.line(pos, tip - right * r, color);
        gizmos.line(pos, tip + up * r, color);
        gizmos.line(pos, tip - up * r, color);
    }
}

/// Directional light: arrow along the forward direction.
fn draw_dir_light_gizmo(
    mut gizmos: Gizmos,
    settings: Res<OverlaySettings>,
    query: Query<
        (&GlobalTransform, &InheritedVisibility, Has<Selected>),
        (With<DirectionalLight>, With<crate::entity_ops::SceneLight>),
    >,
) {
    if !settings.show_bounding_boxes {
        return;
    }
    for (tf, inherited_vis, selected) in &query {
        if !inherited_vis.get() {
            continue;
        }
        let color = marker_color(selected);
        let pos = tf.translation();
        let dir = tf.forward().as_vec3();
        gizmos.arrow(pos, pos + dir * 2.0, color);
    }
}

/// Camera frustum. Filtered by
/// [`SceneCamera`](crate::entity_ops::SceneCamera) so the main
/// viewport camera and the material-preview camera don't get a
/// frustum gizmo.
fn draw_camera_gizmo(
    mut gizmos: Gizmos,
    settings: Res<OverlaySettings>,
    query: Query<
        (
            &Projection,
            &GlobalTransform,
            &InheritedVisibility,
            Has<Selected>,
        ),
        With<crate::entity_ops::SceneCamera>,
    >,
) {
    if !settings.show_bounding_boxes {
        return;
    }
    for (projection, tf, inherited_vis, selected) in &query {
        if !inherited_vis.get() {
            continue;
        }
        let Projection::Perspective(proj) = projection else {
            continue;
        };
        let color = marker_color(selected);
        let depth = 2.0;
        let half_v = depth * (proj.fov / 2.0).tan();
        let half_h = half_v * proj.aspect_ratio;
        let fwd = tf.forward().as_vec3();
        let right = tf.right().as_vec3();
        let up = tf.up().as_vec3();
        let origin = tf.translation();
        let far_center = origin + fwd * depth;
        let corners = [
            far_center + right * half_h + up * half_v,
            far_center - right * half_h + up * half_v,
            far_center - right * half_h - up * half_v,
            far_center + right * half_h - up * half_v,
        ];
        // 4 lines from origin to far corners
        for &c in &corners {
            gizmos.line(origin, c, color);
        }
        // Far rectangle
        for i in 0..4 {
            gizmos.line(corners[i], corners[(i + 1) % 4], color);
        }
    }
}

/// Always-on gizmo marker for entities tagged with
/// [`crate::entity_ops::EmptyEntity`]: small wireframe cube at the
/// origin so the entity is findable and selectable. Independent of
/// the `show_bounding_boxes` setting because Empty entities have no
/// other geometry to anchor the user's eye.
fn draw_empty_entity_marker(
    mut gizmos: Gizmos,
    query: Query<(&GlobalTransform, &InheritedVisibility, Has<Selected>), With<EmptyEntity>>,
) {
    // Fixed 0.5-unit cube so the marker is visible at any camera
    // distance. Not the world AABB: nothing to compute one from.
    const SIZE: f32 = 0.25;
    for (tf, inherited_vis, selected) in &query {
        if !inherited_vis.get() {
            continue;
        }
        let color = marker_color(selected);
        let pos = tf.translation();
        draw_aabb_wireframe(
            &mut gizmos,
            pos - Vec3::splat(SIZE),
            pos + Vec3::splat(SIZE),
            color,
        );
    }
}

/// Each `SceneViewport` gets its own X/Y/Z axis label trio parented
/// to the viewport's UI node. Runs each frame so freshly-spawned
/// viewport panels (quad-view, drag-drop, workspace switch) pick up
/// labels without a startup-only initialiser.
fn ensure_axis_labels(
    mut commands: Commands,
    viewports: Query<Entity, (With<SceneViewport>, Without<ViewportAxisLabels>)>,
) {
    for viewport_entity in &viewports {
        let labels = [
            ("X", default_style::AXIS_X_BRIGHT),
            ("Y", default_style::AXIS_Y_BRIGHT),
            ("Z", default_style::AXIS_Z_BRIGHT),
        ];
        let mut entities = [Entity::PLACEHOLDER; 3];
        for (i, (letter, color)) in labels.iter().enumerate() {
            entities[i] = commands
                .spawn((
                    AxisLabel,
                    crate::EditorEntity,
                    crate::NonSerializable,
                    Text::new(*letter),
                    TextFont {
                        font_size: 14.0,
                        ..default()
                    },
                    TextColor(*color),
                    Node {
                        position_type: PositionType::Absolute,
                        ..default()
                    },
                    ChildOf(viewport_entity),
                ))
                .id();
        }
        commands
            .entity(viewport_entity)
            .insert(ViewportAxisLabels { labels: entities });
    }
}

/// Position each viewport's axis indicator in front of its own camera
/// and update the matching label positions.
///
/// The indicator itself is a retained [`bevy::gizmos::retained::Gizmo`]
/// entity carrying the viewport's private `RenderLayers`, spawned in
/// `viewport::build_viewport_panel`. Per-camera `RenderLayers` gating
/// is what keeps each indicator scoped to one viewport: an immediate
/// `Gizmos` line at a world-space position would otherwise leak into
/// every camera that has the same point in its frustum, which is what
/// produced ghost indicators in adjacent panels with overlapping
/// world-space positions.
///
/// We write both `Transform` and `GlobalTransform` so the render
/// extraction (which reads `GlobalTransform`) sees the updated pose
/// the same frame, regardless of where the system slots relative to
/// `TransformSystems::Propagate`.
fn draw_coordinate_indicator(
    settings: Res<OverlaySettings>,
    cameras: Query<
        (&Camera, &GlobalTransform, &Projection),
        (
            With<crate::viewport::MainViewportCamera>,
            Without<AxisIndicator>,
        ),
    >,
    viewports: Query<(&ComputedNode, &ViewportNode, &ViewportAxisLabels), With<SceneViewport>>,
    mut label_query: Query<(&mut Node, &mut Visibility), With<AxisLabel>>,
    mut indicators: Query<
        (
            &AxisIndicator,
            &mut Transform,
            &mut GlobalTransform,
            &mut Visibility,
        ),
        (
            Without<AxisLabel>,
            Without<crate::viewport::MainViewportCamera>,
        ),
    >,
) {
    if !settings.show_coordinate_indicator {
        for (_, mut vis) in &mut label_query {
            *vis = Visibility::Hidden;
        }
        for (_, _, _, mut vis) in &mut indicators {
            *vis = Visibility::Hidden;
        }
        return;
    }

    // Place each indicator entity in front of its camera and resize
    // it for the camera's fov so it occupies a stable on-screen
    // fraction; labels follow at the projected tip positions.
    for (computed, viewport_node, axis_labels) in &viewports {
        let Ok((camera, cam_tf, projection)) = cameras.get(viewport_node.camera) else {
            continue;
        };
        let Projection::Perspective(proj) = projection else {
            continue;
        };

        let depth = 0.5;
        let half_height = depth * (proj.fov / 2.0).tan();
        let half_width = half_height * proj.aspect_ratio;
        let ndc_x = -0.85;
        let ndc_y = -0.80;

        let indicator_pos = cam_tf.translation()
            + cam_tf.forward().as_vec3() * depth
            + cam_tf.right().as_vec3() * (ndc_x * half_width)
            + cam_tf.up().as_vec3() * (ndc_y * half_height);
        let size = half_height * 0.07;
        let pose = Transform {
            translation: indicator_pos,
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(size),
        };

        for (link, mut tf, mut gtf, mut vis) in &mut indicators {
            if link.camera != viewport_node.camera {
                continue;
            }
            *tf = pose;
            *gtf = GlobalTransform::from(pose);
            *vis = Visibility::Inherited;
        }

        let axes = [Vec3::X, Vec3::Y, Vec3::Z];
        let vp_node_size = computed.size();
        let render_target_size = camera.logical_viewport_size().unwrap_or(vp_node_size);

        for (i, label_entity) in axis_labels.labels.iter().enumerate() {
            if let Ok((mut node, mut vis)) = label_query.get_mut(*label_entity) {
                let tip_pos = indicator_pos + axes[i] * size * 1.35;
                if let Ok(vp_coords) = camera.world_to_viewport(cam_tf, tip_pos) {
                    let ui_pos = vp_coords * vp_node_size / render_target_size;
                    node.left = Val::Px(ui_pos.x - 4.0);
                    node.top = Val::Px(ui_pos.y - 7.0);
                    *vis = Visibility::Inherited;
                } else {
                    *vis = Visibility::Hidden;
                }
            }
        }
    }
}

/// Draw wireframe cuboid for `NavmeshRegion` entities showing their AABB bounds.
fn draw_navmesh_region_bounds(
    mut gizmos: Gizmos,
    regions: Query<&GlobalTransform, With<jackdaw_jsn::NavmeshRegion>>,
) {
    let color = default_style::NAVMESH_REGION_BOUNDS;
    for global_tf in &regions {
        let transform = global_tf.compute_transform();
        gizmos.cube(transform, color);
    }
}
