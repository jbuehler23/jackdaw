use bevy::{
    gizmos::config::{GizmoLineConfig, GizmoLineJoint, GizmoLineStyle},
    prelude::Color,
};

// ── Axis colors (Blender default theme, X = red, Y = green, Z = blue) ──
pub const AXIS_X: Color = Color::srgba(1.0, 0.2, 0.32, 0.6);
pub const AXIS_Y: Color = Color::srgba(0.545, 0.863, 0.0, 0.6);
pub const AXIS_Z: Color = Color::srgba(0.157, 0.565, 1.0, 0.6);
pub const AXIS_X_BRIGHT: Color = Color::srgba(1.0, 0.2, 0.32, 1.0);
pub const AXIS_Y_BRIGHT: Color = Color::srgba(0.545, 0.863, 0.0, 1.0);
pub const AXIS_Z_BRIGHT: Color = Color::srgba(0.157, 0.565, 1.0, 1.0);

// ── Brush wireframe + outline ──
pub const WIREFRAME_OUTLINE_SELECTED: Color = Color::srgba(1.0, 0.4196, 0.10196, 1.0);
pub const WIREFRAME_OUTLINE_SELECTED_CLIP: Color = Color::srgba(0.133, 0.827, 0.933, 0.25);
pub const WIREFRAME_OUTLINE_GROUP_EDIT: Color = Color::srgba(0.133, 0.827, 0.933, 0.35);
pub const WIREFRAME_OUTLINE_UNSELECTED: Color = Color::srgba(1.0, 1.0, 1.0, 1.0);
pub const WIREFRAME_OUTLINE_CUT_PREVIEW: Color = Color::srgba(0.133, 0.827, 0.933, 0.25);

// ── Brush wireframe ──
pub const WIREFRAME_LINE_UNSELECTED: GizmoLineConfig = GizmoLineConfig {
    width: 0.3,
    ..DEFAULT_LINE_CONFIG
};
pub const WIREFRAME_LINE_SELECTED: GizmoLineConfig = GizmoLineConfig {
    width: 0.4,
    ..DEFAULT_LINE_CONFIG
};

// ── Brush outline ──
pub const OUTLINE_LINE_UNSELECTED: GizmoLineConfig = GizmoLineConfig {
    width: 0.5,
    ..DEFAULT_LINE_CONFIG
};
pub const OUTLINE_LINE_SELECTED: GizmoLineConfig = GizmoLineConfig {
    width: 2.0,
    ..DEFAULT_LINE_CONFIG
};

// ── Face grid ──
pub const FACE_GRID_SELECTED: Color = Color::srgba(0.2, 0.2, 0.2, 0.6);
pub const FACE_GRID_UNSELECTED: Color = FACE_GRID_SELECTED;
pub const FACE_GRID_LINE: GizmoLineConfig = GizmoLineConfig {
    perspective: true,
    width: 1.8,
    ..DEFAULT_LINE_CONFIG
};

// ── Selection & bounding boxes ──
pub const SELECTION_BBOX: Color = Color::srgba(1.0, 1.0, 0.0, 0.8);
/// Dim variant for marker gizmos on unselected lights, cameras, and
/// empty entities. Keeps invisible entities locatable in the viewport
/// without overpowering selection highlights.
pub const ENTITY_MARKER_UNSELECTED: Color = Color::srgba(0.55, 0.55, 0.6, 0.35);
pub const SELECTION_MARQUEE_BG: Color = Color::srgba(0.3, 0.5, 1.0, 0.1);
pub const SELECTION_MARQUEE_BORDER: Color = Color::srgba(0.3, 0.5, 1.0, 0.7);

// ── Brush edit mode ──
/// Things that are currently being edited
pub const EDIT_SELECTED_COLOR: Color = Color::WHITE;
/// For things that are not currently being edited, but could be if the user e.g. clicked on them
pub const EDIT_AVAILABLE_COLOR: Color = Color::srgba(0.996, 0.996, 0.133333, 1.0);
pub const EDIT_VERTEX_RADIUS: f32 = 0.04;
pub const FACE_EXTRUDE_PREVIEW: Color = Color::srgb(0.0, 1.0, 0.5);

// ── Draw / cut mode ──
pub const DRAW_MODE: Color = WIREFRAME_OUTLINE_SELECTED;
pub const CUT_MODE: Color = Color::srgb(1.0, 0.2, 0.2);
pub const DRAW_PREVIEW_MESH: Color = Color::srgba(1.0, 0.6, 0.0, 0.25);
pub const CUT_PREVIEW_MESH: Color = Color::srgba(1.0, 0.2, 0.2, 0.15);
pub const DRAW_PLANE_GRID: Color = Color::srgba(0.5, 0.5, 0.5, 1.0);

// ── Clip mode ──
pub const CLIP_POINT: Color = Color::srgb(1.0, 0.3, 0.3);
pub const CLIP_KEEP: Color = Color::srgba(0.3, 1.0, 0.5, 0.8);
pub const CLIP_DISCARD: Color = Color::srgba(1.0, 0.2, 0.2, 0.4);
pub const CLIP_SPLIT_BACK: Color = Color::srgba(0.3, 0.5, 1.0, 0.8);

// ── Alignment guides ──
pub const ALIGNMENT_GUIDE: Color = Color::srgba(1.0, 0.65, 0.0, 0.85);

// ── Measure tool ──
pub const MARKER_SIZE: f32 = 0.008;
pub const MEASURE_TOOL_LINE: Color = Color::srgb(1.0, 0.84, 0.0);
pub const MEASURE_TOOL_LABEL: Color = Color::srgba(1.0, 0.84, 0.0, 1.0);

// ── Navmesh visualization ──
pub const NAVMESH_DETAIL_WIREFRAME: Color = Color::srgb(0.204, 0.827, 0.600);
pub const NAVMESH_POLYGON_WIREFRAME: Color = Color::srgb(0.984, 0.749, 0.141);
pub const NAVMESH_POLYGON_FILL: Color = Color::srgba(0.145, 0.388, 0.922, 0.2);
pub const NAVMESH_OBSTACLE_WIREFRAME: Color = Color::srgb(0.761, 0.255, 0.047);
pub const NAVMESH_REGION_BOUNDS: Color = Color::srgba(0.2, 0.8, 0.4, 0.6);
pub const NAVMESH_AREA_0: Color = Color::srgba(0.0, 0.4, 0.8, 0.25);
pub const NAVMESH_AREA_1: Color = Color::srgba(0.8, 0.4, 0.0, 0.25);
pub const NAVMESH_AREA_2: Color = Color::srgba(0.8, 0.0, 0.4, 0.25);
pub const NAVMESH_AREA_3: Color = Color::srgba(0.4, 0.0, 0.8, 0.25);
pub const NAVMESH_AREA_DEFAULT: Color = Color::srgba(0.5, 0.5, 0.5, 0.25);

// ── Grid ──
pub const GRID_MAJOR_LINE: Color = Color::srgb(0.3, 0.3, 0.3);
pub const GRID_MINOR_LINE: Color = Color::srgb(0.25, 0.25, 0.25);

// ── Terrain ──
pub const TERRAIN_SCULPT_GIZMO: Color = Color::srgb(1.0, 0.8, 0.2);

// ── Material preview ──
pub const MATERIAL_PREVIEW_BG: Color = Color::srgba(0.15, 0.15, 0.15, 1.0);

// ── Brush default material variants ──
pub const DEFAULT_MATERIAL_COLOR: Color = Color::srgba(0.980, 0.8549, 0.3686, 0.5);
pub const DEFAULT_MATERIAL_SELECTED_COLOR: Color = Color::srgba(0.980, 0.8549, 0.3686, 0.75);

// ── Brush material palette ──
pub const BRUSH_PALETTE: [Color; 8] = [
    Color::srgb(0.7, 0.7, 0.7), // default grey
    Color::srgb(0.5, 0.5, 0.5), // gray
    Color::srgb(0.3, 0.3, 0.3), // dark gray
    Color::srgb(0.7, 0.3, 0.2), // brick red
    Color::srgb(0.3, 0.5, 0.7), // steel blue
    Color::srgb(0.4, 0.6, 0.3), // mossy green
    Color::srgb(0.6, 0.5, 0.3), // sandy tan
    Color::srgb(0.5, 0.3, 0.5), // purple
];

// ── Inspector UI ──
pub const INSPECTOR_AXIS_X: Color = Color::srgb(0.8, 0.3, 0.3);
pub const INSPECTOR_AXIS_Y: Color = Color::srgb(0.3, 0.7, 0.3);
pub const INSPECTOR_AXIS_Z: Color = Color::srgb(0.3, 0.5, 0.8);
pub const INSPECTOR_OVERRIDE: Color = Color::srgb(1.0, 0.6, 0.3);

// ── General ──
/// `Default::default()` is non-const, so we have to make our own.
pub const DEFAULT_LINE_CONFIG: GizmoLineConfig = GizmoLineConfig {
    width: 2.0,
    perspective: false,
    style: GizmoLineStyle::Solid,
    joints: GizmoLineJoint::None,
};
