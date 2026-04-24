use std::borrow::Cow;

use bevy::{feathers::theme::ThemedText, picking::hover::Hovered, prelude::*, ui_widgets::observe};
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    icons::{Icon, IconFont},
    menu_bar, popover, separator, split_panel, status_bar,
    text_edit::{self, TextEditProps},
    tokens,
    tooltip::Tooltip,
    tree_view::tree_container_drop_observers,
};

use crate::{
    EditorEntity,
    brush::{BrushEditMode, EditMode},
    draw_brush::{ActivateDrawBrushModalOp, DrawBrushState},
    edit_mode_ops::{
        EditModeClipOp, EditModeEdgeOp, EditModeFaceOp, EditModeObjectOp, EditModePhysicsOp,
        EditModeVertexOp,
    },
    gizmo_ops::{GizmoModeRotateOp, GizmoModeScaleOp, GizmoModeTranslateOp, GizmoSpaceToggleOp},
    gizmos::{GizmoMode, GizmoSpace},
    hierarchy::{HierarchyPanel, HierarchyShowAllButton, HierarchyTreeContainer},
    inspector::Inspector,
    remote::ConnectionManager,
    viewport::SceneViewport,
};

/// Discriminator for the header tab kinds the editor knows how to host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TabKind {
    /// The live scene being edited. There's exactly one Scene tab.
    #[default]
    Scene,
    /// The Schedule Explorer / remote debug view (replaces the old
    /// "Remote Debug" workspace). There's exactly one Schedule Explorer
    /// tab.
    ScheduleExplorer,
}

impl TabKind {
    /// Human-readable label shown on the tab strip.
    pub fn label(self) -> &'static str {
        match self {
            TabKind::Scene => "Main scene",
            TabKind::ScheduleExplorer => "Schedule Explorer",
        }
    }

    /// Colored accent stripe drawn at the left edge of the tab.
    pub fn accent(self) -> Color {
        match self {
            TabKind::Scene => tokens::DOC_TAB_SCENE_ACCENT,
            TabKind::ScheduleExplorer => tokens::DOC_TAB_TOOL_ACCENT,
        }
    }

    /// Icon glyph rendered in the tab header.
    pub fn icon(self) -> Icon {
        match self {
            TabKind::Scene => Icon::File,
            TabKind::ScheduleExplorer => Icon::CalendarSearch,
        }
    }
}

/// Layout preset for the Scene document tab.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SceneViewPreset {
    #[default]
    Scene,
}

/// The tab the editor is currently showing.
#[derive(Resource, Default, Clone, Copy)]
pub struct ActiveDocument {
    pub kind: TabKind,
}

/// Marker on the tab strip row container so the tab styling system can
/// find its children.
#[derive(Component)]
pub struct DocumentTabStrip;

/// Marker on an individual document tab button, tagged with the
/// `TabKind` it activates when clicked.
#[derive(Component)]
pub struct DocumentTabButton(pub TabKind);

/// Marker on a document content container. The per-frame
/// `update_active_document_display` system toggles `Node::display` on
/// these so only the matching-kind container is visible.
#[derive(Component)]
pub struct DocumentRoot(pub TabKind);

/// Marker on the center column container. Retained as a hook for
/// systems that want to find the main viewport-plus-bottom-panels
/// area. Formerly driven by `SceneViewPreset`; now unconditional.
#[derive(Component)]
pub struct SceneCenter;

/// Marker on the hierarchy filter text input
#[derive(Component)]
pub struct HierarchyFilter;

/// Marker for the toolbar
#[derive(Component)]
pub struct Toolbar;

/// Marker for gizmo mode buttons
#[derive(Component)]
pub struct GizmoModeButton(pub GizmoMode);

/// Marker for gizmo space toggle
#[derive(Component)]
pub struct GizmoSpaceButton;

/// Marker for edit mode/tool buttons in the toolbar
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum EditToolButton {
    Object,
    Vertex,
    Edge,
    Face,
    Clip,
    Physics,
    Operator(&'static str),
}

/// Marker for keybind helper button
#[derive(Component)]
pub struct KeybindHelpButton;

/// Resource tracking the keybind help popover entity
#[derive(Resource, Default)]
pub struct KeybindHelpPopover {
    pub entity: Option<Entity>,
}

pub fn editor_layout(icon_font: &IconFont) -> impl Bundle {
    let font = icon_font.0.clone();
    (
        EditorEntity,
        // Outer shell: dark background with padding (Figma: 10px padding, bg #171717)
        BackgroundColor(tokens::WINDOW_BG),
        Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            padding: UiRect::all(px(tokens::PANEL_GAP)),
            ..Default::default()
        },
        children![(
            // Inner container: the editor workspace with rounded corners and border.
            EditorEntity,
            Node {
                width: percent(100),
                flex_grow: 1.0,
                min_height: px(0.0),
                flex_direction: FlexDirection::Column,
                border: UiRect::all(px(1.0)),
                border_radius: BorderRadius::all(px(8.0)),
                overflow: Overflow::clip(),
                ..Default::default()
            },
            BackgroundColor(tokens::WINDOW_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            children![
                // Integrated window header: menu bar + scene tabs + controls
                window_header(),
                // Content container (flex grow). Holds both workspaces.
                // Figma: Editor (Rows) has padding: 0px 4px
                (
                    EditorEntity,
                    Node {
                        width: percent(100),
                        flex_grow: 1.0,
                        min_height: px(0.0),
                        flex_direction: FlexDirection::Column,
                        padding: UiRect::horizontal(px(tokens::PANEL_GAP)),
                        row_gap: px(tokens::PANEL_GAP),
                        ..Default::default()
                    },
                    children![
                    // Scene document (visible by default).
                    (
                        DocumentRoot(TabKind::Scene),
                        EditorEntity,
                        Node {
                            width: percent(100),
                            flex_grow: 1.0,
                            min_height: px(0.0),
                            display: Display::Flex,
                            ..Default::default()
                        },
                        children![(
                            // Three-column layout: left (hierarchy) | center (viewport+bottom) | right (inspector)
                            // Must have its own Node with Row direction for horizontal split
                            Node {
                                width: percent(100),
                                height: percent(100),
                                flex_direction: FlexDirection::Row,
                                ..Default::default()
                            },
                            split_panel::panel_group(
                                0.1,
                                (
                                    // Left column: single anchor host, pre-split by default
                                    Spawn((split_panel::panel(1), left_dock_area())),
                                    Spawn(split_panel::panel_handle()),
                                    // Center column: viewport + bottom dock (ratio 4).
                                    Spawn((
                                        split_panel::panel(4),
                                        center_column(font.clone()),
                                    )),
                                    Spawn(split_panel::panel_handle()),
                                    // Right column: inspector (~310px default, ratio 1)
                                    Spawn((split_panel::panel(1), right_dock_area())),
                                ),
                            ),
                        )],
                    ),
                    // Schedule Explorer document (hidden by default).
                    // Formerly the Remote Debug workspace; same content
                    // repackaged as a document tab.
                    (
                        DocumentRoot(TabKind::ScheduleExplorer),
                        EditorEntity,
                        Node {
                            width: percent(100),
                            flex_grow: 1.0,
                            min_height: px(0.0),
                            flex_direction: FlexDirection::Column,
                            display: Display::None,
                            ..Default::default()
                        },
                        split_panel::panel_group(
                            0.2,
                            (
                                Spawn((
                                    split_panel::panel(1),
                                    crate::remote::entity_browser::remote_debug_workspace_content(),
                                )),
                                Spawn(split_panel::panel_handle()),
                                Spawn((
                                    split_panel::panel(1),
                                    crate::remote::remote_inspector::remote_inspector(),
                                )),
                            ),
                        ),
                    )
                ],
                ),
                // Status bar (fixed height) with connection indicator
                editor_status_bar()
            ],
        )],
    )
}

/// Integrated window header. Two groups separated by a flexible spacer:
/// the **left group** owns the menu bar and the document tab strip (so
/// tabs sit right after the `Add` menu, matching the Figma mock), and
/// the **right group** owns the Scene View combobox and the Play/Pause
/// pill. A flex-grow spacer between them absorbs the slack, so resizing
/// the dropdown label (e.g. `Scene View ▾` → `Animation View ▾`) can't
/// shift the tabs.
fn window_header() -> impl Bundle {
    (
        EditorEntity,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            width: percent(100),
            height: px(34.0),
            flex_shrink: 0.0,
            border_radius: BorderRadius::top(Val::Px(7.0)),
            ..Default::default()
        },
        BackgroundColor(tokens::WINDOW_BG),
        children![
            // Left: menu bar + tab strip, sitting flush to the left
            // edge. `column_gap` pushes the tabs slightly away from the
            // last menu item ("Add").
            (
                EditorEntity,
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    height: percent(100),
                    column_gap: px(tokens::SPACING_LG),
                    ..Default::default()
                },
                children![
                    menu_bar::menu_bar_shell(),
                    (
                        jackdaw_panels::WorkspaceTabStrip,
                        DocumentTabStrip,
                        EditorEntity,
                        Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            height: percent(100),
                            column_gap: px(4.0),
                            ..Default::default()
                        },
                    ),
                ],
            ),
            // Flexible spacer; absorbs leftover horizontal space
            // between the left group and the right group.
            (
                EditorEntity,
                Node {
                    flex_grow: 1.0,
                    ..Default::default()
                },
            ),
            // Right: Scene View combobox + Play/Pause transport.
            (
                EditorEntity,
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    padding: UiRect::horizontal(px(tokens::SPACING_MD)),
                    column_gap: px(6.0),
                    ..Default::default()
                },
                children![play_pause_controls(),],
            ),
        ],
    )
}

/// Play / Pause / Stop transport pill. Clicking a button triggers
/// the corresponding `PiePlugin` handler. The plugin installs a
/// click observer on each `PieButton` via an `On<Add, PieButton>`
/// observer, so wiring here is purely presentation.
fn play_pause_controls() -> impl Bundle {
    (
        EditorEntity,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            height: px(22.0),
            padding: UiRect::horizontal(px(6.5)),
            column_gap: px(9.0),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_LG)),
            ..Default::default()
        },
        BackgroundColor(tokens::HEADER_CONTROL_BG),
        BorderColor::all(tokens::HEADER_CONTROL_BORDER),
        children![
            pie_transport_button(crate::pie::PieButton::Play, Icon::Play),
            pie_transport_button(crate::pie::PieButton::Pause, Icon::Pause),
            pie_transport_button(crate::pie::PieButton::Stop, Icon::Square),
        ],
    )
}

/// Single clickable glyph. The `PieButton` marker is the hook the
/// `PiePlugin` uses to attach the click observer.
fn pie_transport_button(kind: crate::pie::PieButton, icon: Icon) -> impl Bundle {
    (
        kind,
        EditorEntity,
        Node {
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding: UiRect::horizontal(px(2.0)),
            ..Default::default()
        },
        children![(
            Text::new(String::from(icon.unicode())),
            TextFont {
                font_size: 13.0,
                ..Default::default()
            },
            TextColor(tokens::HEADER_CONTROL_LABEL),
            Pickable::IGNORE,
        )],
    )
}

/// Left column: a single anchor host the user can split like the right
/// sidebar. The default layout pre-splits it vertically (Scene Tree +
/// Import on top, Project Files on bottom) on first launch via
/// `apply_default_splits` in the editor crate.
fn left_dock_area() -> impl Bundle {
    (
        jackdaw_panels::reconcile::AnchorHost {
            anchor_id: "left".into(),
            default_style: jackdaw_panels::DockAreaStyle::TabBar,
        },
        jackdaw_panels::DockArea {
            id: "left".into(),
            style: jackdaw_panels::DockAreaStyle::TabBar,
        },
        EditorEntity,
        Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            overflow: Overflow::clip(),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_LG)),
            ..Default::default()
        },
        BackgroundColor(tokens::PANEL_BG),
    )
}

/// Project Files panel. File tree browser.
pub fn project_files_panel_content() -> impl Bundle {
    (
        EditorEntity,
        Node {
            flex_direction: FlexDirection::Column,
            width: percent(100),
            height: percent(100),
            ..Default::default()
        },
        children![
            // Search input
            (
                Node {
                    flex_direction: FlexDirection::Column,
                    width: percent(100),
                    padding: UiRect::all(px(tokens::SPACING_SM)),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
                children![(text_edit::text_edit(
                    TextEditProps::default()
                        .with_placeholder("Search...")
                        .allow_empty()
                ),)],
            ),
            // File tree content, populated by ProjectFilesPlugin.
            (
                crate::project_files::ProjectFilesTree,
                EditorEntity,
                Node {
                    flex_direction: FlexDirection::Column,
                    width: percent(100),
                    flex_grow: 1.0,
                    min_height: px(0.0),
                    overflow: Overflow::scroll_y(),
                    padding: UiRect::all(px(tokens::SPACING_SM)),
                    ..Default::default()
                },
            ),
        ],
    )
}

/// Build the center column: a vertical split with the 3D viewport on
/// top and the tabbable bottom-panels area (Assets / Timeline / ...)
/// underneath. The timeline is a regular tab in the bottom panel, so
/// animating no longer requires switching into an "Animation View".
fn center_column(icon_font: Handle<Font>) -> impl Bundle {
    (
        EditorEntity,
        Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            ..Default::default()
        },
        split_panel::panel_group(
            0.15,
            (
                Spawn((
                    split_panel::panel(4),
                    viewport_with_toolbar(icon_font.clone()),
                )),
                Spawn(split_panel::panel_handle()),
                Spawn((split_panel::panel(1), bottom_dock_area())),
            ),
        ),
    )
}

fn viewport_with_toolbar(icon_font: Handle<Font>) -> impl Bundle {
    (
        EditorEntity,
        jackdaw_panels::drag::ViewportDropTarget,
        Node {
            height: percent(100),
            flex_direction: FlexDirection::Column,
            overflow: Overflow::clip(),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_LG)),
            ..Default::default()
        },
        BackgroundColor(tokens::PANEL_BG),
        children![
            toolbar(icon_font),
            crate::navmesh::toolbar::navmesh_toolbar(),
            crate::terrain::toolbar::terrain_toolbar(),
            scene_view(),
        ],
    )
}

fn toolbar(icon_font: Handle<Font>) -> impl Bundle {
    let f = icon_font.clone();
    (
        Toolbar,
        EditorEntity,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding: UiRect::axes(px(tokens::SPACING_MD), px(tokens::SPACING_SM)),
            column_gap: px(tokens::SPACING_SM),
            width: percent(100),
            height: px(32.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::PANEL_HEADER_BG),
        children![
            // Gizmo mode buttons
            toolbar_button(
                Icon::Move3d,
                "",
                GizmoMode::Translate,
                icon_font.clone(),
                "Move (Esc)"
            ),
            toolbar_button(
                Icon::Rotate3d,
                "R",
                GizmoMode::Rotate,
                icon_font.clone(),
                "Rotate (R)"
            ),
            toolbar_button(
                Icon::Scale3d,
                "T",
                GizmoMode::Scale,
                icon_font.clone(),
                "Scale (T)"
            ),
            // Separator
            separator::separator(separator::SeparatorProps::vertical()),
            // Space toggle
            toolbar_space_button(f.clone()),
            // Separator
            separator::separator(separator::SeparatorProps::vertical()),
            // Edit mode buttons
            toolbar_edit_button(
                Icon::MousePointer2,
                EditToolButton::Object,
                f.clone(),
                "Object Mode"
            ),
            toolbar_edit_button(
                Icon::Box,
                EditToolButton::Operator(ActivateDrawBrushModalOp::ID),
                f.clone(),
                // todo: add keybind
                ActivateDrawBrushModalOp::LABEL
            ),
            toolbar_edit_button(
                Icon::CircleDot,
                EditToolButton::Vertex,
                f.clone(),
                "Vertex Mode (1)"
            ),
            toolbar_edit_button(
                Icon::GitCommitHorizontal,
                EditToolButton::Edge,
                f.clone(),
                "Edge Mode (2)"
            ),
            toolbar_edit_button(
                Icon::Hexagon,
                EditToolButton::Face,
                f.clone(),
                "Face Mode (3)"
            ),
            toolbar_edit_button(
                Icon::ScissorsLineDashed,
                EditToolButton::Clip,
                f.clone(),
                "Clip Mode (4)"
            ),
            // Separator
            separator::separator(separator::SeparatorProps::vertical()),
            toolbar_edit_button(
                Icon::Zap,
                EditToolButton::Physics,
                f.clone(),
                "Physics Tool"
            ),
            // Spacer pushes help button to the right
            (Node {
                flex_grow: 1.0,
                ..Default::default()
            },),
            // Keybind help button
            toolbar_help_button(f),
        ],
    )
}

fn toolbar_button(
    icon: Icon,
    label: &str,
    mode: GizmoMode,
    font: Handle<Font>,
    tooltip: &str,
) -> impl Bundle {
    let label = label.to_string();
    let op_id: &'static str = match mode {
        GizmoMode::Translate => GizmoModeTranslateOp::ID,
        GizmoMode::Rotate => GizmoModeRotateOp::ID,
        GizmoMode::Scale => GizmoModeScaleOp::ID,
    };
    (
        GizmoModeButton(mode),
        Hovered::default(),
        Tooltip(tooltip.into()),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(tokens::SPACING_XS),
            padding: UiRect::axes(px(tokens::SPACING_MD), px(tokens::SPACING_XS)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            ..Default::default()
        },
        BackgroundColor(tokens::TOOLBAR_BUTTON_BG),
        children![
            (
                Text::new(String::from(icon.unicode())),
                TextFont {
                    font,
                    font_size: 15.0,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
            (
                Text::new(label),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                ThemedText,
            )
        ],
        observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands
                .operator(op_id)
                .settings(CallOperatorSettings {
                    execution_context: ExecutionContext::Invoke,
                    creates_history_entry: true,
                })
                .call();
        }),
    )
}

fn toolbar_space_button(icon_font: Handle<Font>) -> impl Bundle {
    (
        GizmoSpaceButton,
        Hovered::default(),
        Tooltip("Toggle World/Local (X)".into()),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(tokens::SPACING_XS),
            padding: UiRect::axes(px(tokens::SPACING_MD), px(tokens::SPACING_XS)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            ..Default::default()
        },
        BackgroundColor(tokens::TOOLBAR_BUTTON_BG),
        children![
            (
                Text::new(String::from(Icon::Globe.unicode())),
                TextFont {
                    font: icon_font,
                    font_size: tokens::FONT_MD,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
            (
                Text::new("World"),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                ThemedText,
            )
        ],
        observe(|_: On<Pointer<Click>>, mut commands: Commands| {
            commands
                .operator(GizmoSpaceToggleOp::ID)
                .settings(CallOperatorSettings {
                    execution_context: ExecutionContext::Invoke,
                    creates_history_entry: true,
                })
                .call();
        }),
    )
}

fn toolbar_edit_button(
    icon: Icon,
    tool: EditToolButton,
    font: Handle<Font>,
    tooltip: &str,
) -> impl Bundle {
    (
        tool,
        Hovered::default(),
        Tooltip(tooltip.into()),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding: UiRect::axes(px(tokens::SPACING_MD), px(tokens::SPACING_XS)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            ..Default::default()
        },
        BackgroundColor(tokens::TOOLBAR_BUTTON_BG),
        children![(
            Text::new(String::from(icon.unicode())),
            TextFont {
                font,
                font_size: 15.0,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
        observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let op_id: Cow<'static, str> = match tool {
                EditToolButton::Object => EditModeObjectOp::ID.into(),
                EditToolButton::Vertex => EditModeVertexOp::ID.into(),
                EditToolButton::Edge => EditModeEdgeOp::ID.into(),
                EditToolButton::Face => EditModeFaceOp::ID.into(),
                EditToolButton::Clip => EditModeClipOp::ID.into(),
                EditToolButton::Physics => EditModePhysicsOp::ID.into(),
                EditToolButton::Operator(op) => op.into(),
            };
            commands
                .operator(op_id)
                .settings(CallOperatorSettings {
                    execution_context: ExecutionContext::Invoke,
                    creates_history_entry: true,
                })
                .call();
        }),
    )
}

fn toolbar_help_button(icon_font: Handle<Font>) -> impl Bundle {
    (
        KeybindHelpButton,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding: UiRect::axes(px(tokens::SPACING_MD), px(tokens::SPACING_XS)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            ..Default::default()
        },
        BackgroundColor(tokens::TOOLBAR_BUTTON_BG),
        children![(
            Text::new(String::from(Icon::Keyboard.unicode())),
            TextFont {
                font: icon_font,
                font_size: tokens::FONT_MD,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
        observe(
            |trigger: On<Pointer<Click>>,
             mut commands: Commands,
             mut popover_state: ResMut<KeybindHelpPopover>,
             registry: Res<crate::keybinds::KeybindRegistry>| {
                // Toggle: if popover exists, despawn it
                if let Some(entity) = popover_state.entity.take() {
                    if let Ok(mut ec) = commands.get_entity(entity) {
                        ec.despawn();
                    }
                    return;
                }

                let anchor = trigger.event_target();

                let popover_entity = commands
                    .spawn(popover::popover(
                        popover::PopoverProps::new(anchor)
                            .with_placement(popover::PopoverPlacement::BottomEnd)
                            .with_padding(12.0)
                            .with_z_index(200),
                    ))
                    .with_children(|parent| {
                        parent
                            .spawn(Node {
                                flex_direction: FlexDirection::Column,
                                max_height: px(500.0),
                                overflow: Overflow::scroll_y(),
                                ..Default::default()
                            })
                            .with_children(|scroll_parent| {
                                spawn_keybind_help_content(scroll_parent, &registry);
                            });
                    })
                    .id();

                popover_state.entity = Some(popover_entity);
            },
        ),
    )
}

fn spawn_keybind_help_content(
    parent: &mut ChildSpawnerCommands,
    registry: &crate::keybinds::KeybindRegistry,
) {
    use jackdaw_commands::keybinds::EditorAction;

    // Mouse-only entries that can't be expressed as keybinds, grouped by category
    let mouse_entries: &[(&str, &[(&str, &str)])] = &[
        (
            "Navigation",
            &[
                ("RMB + Drag", "Look around"),
                ("Shift", "Double speed"),
                ("Scroll", "Dolly forward/back"),
                ("RMB + Scroll", "Adjust move speed"),
            ],
        ),
        (
            "Selection",
            &[
                ("LMB", "Select entity"),
                ("Ctrl+Click", "Toggle multi-select"),
                ("Shift+LMB Drag", "Box select"),
            ],
        ),
        (
            "Transform",
            &[
                ("MMB", "Toggle snap"),
                ("Ctrl", "Toggle snap (during drag)"),
            ],
        ),
        (
            "Brush Edit",
            &[
                ("Shift+Click", "Multi-select"),
                ("Click+Drag", "Move selected"),
            ],
        ),
        (
            "Draw Brush",
            &[
                ("Click", "Place vertex / advance"),
                ("Right-click", "Cancel"),
            ],
        ),
        ("View", &[("Ctrl+Alt+Scroll", "Grid size")]),
    ];

    // Build sections dynamically from registry
    let category_order = [
        "File",
        "Entity",
        "Transform",
        "Brush Edit",
        "Draw Brush",
        "CSG",
        "Gizmo",
        "Navigation",
        "View",
    ];

    // Also include Selection between Navigation and View
    let display_order = [
        "Navigation",
        "Selection",
        "Transform",
        "Entity",
        "Brush Edit",
        "CSG",
        "Draw Brush",
        "Gizmo",
        "View",
        "File",
    ];

    for &section in &display_order {
        let mut entries: Vec<(String, String)> = Vec::new();

        // Add mouse entries for this section
        for (cat, mouse_binds) in mouse_entries {
            if *cat == section {
                for (key, desc) in *mouse_binds {
                    entries.push((key.to_string(), desc.to_string()));
                }
            }
        }

        // Add keybind entries for this section (if it's a real category)
        if category_order.contains(&section) {
            for &action in EditorAction::all() {
                if action.category() != section {
                    continue;
                }
                let bindings = registry.bindings.get(&action).cloned().unwrap_or_default();
                if bindings.is_empty() {
                    continue;
                }
                let key_str = bindings
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" / ");
                entries.push((key_str, action.to_string()));
            }
        }

        if entries.is_empty() {
            continue;
        }

        // Section header
        parent.spawn((
            Text::new(section),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_PRIMARY),
            Node {
                margin: UiRect::top(px(tokens::SPACING_SM)),
                ..Default::default()
            },
        ));

        for (key, desc) in &entries {
            parent.spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::SpaceBetween,
                    column_gap: px(tokens::SPACING_LG),
                    width: px(260.0),
                    ..Default::default()
                },
                children![
                    (
                        Text::new(key.clone()),
                        TextFont {
                            font_size: tokens::FONT_SM,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_PRIMARY),
                    ),
                    (
                        Text::new(desc.clone()),
                        TextFont {
                            font_size: tokens::FONT_SM,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_SECONDARY),
                    )
                ],
            ));
        }
    }
}

pub fn hierarchy_content(icon_font: Handle<Font>) -> impl Bundle {
    let add_entity_icon_font = icon_font.clone();
    (
        HierarchyPanel,
        Node {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: px(0.0),
            padding: UiRect::all(px(tokens::SPACING_SM)),
            ..Default::default()
        },
        children![
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(tokens::SPACING_XS),
                    width: percent(100),
                    ..Default::default()
                },
                children![
                    (
                        Node {
                            flex_grow: 1.0,
                            ..Default::default()
                        },
                        children![(
                            HierarchyFilter,
                            text_edit::text_edit(
                                TextEditProps::default()
                                    .with_placeholder("Filter...")
                                    .allow_empty()
                            ),
                        )],
                    ),
                    (
                        HierarchyShowAllButton,
                        Interaction::default(),
                        Node {
                            width: px(24.0),
                            height: px(24.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
                            ..Default::default()
                        },
                        children![(
                            Text::new(String::from(Icon::Eye.unicode())),
                            TextFont {
                                font: icon_font,
                                font_size: 14.0,
                                ..Default::default()
                            },
                            TextColor(tokens::TEXT_SECONDARY),
                        )],
                    ),
                ],
            ),
            (
                crate::add_entity_picker::AddEntityButton,
                Interaction::default(),
                Hovered::default(),
                Node {
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    width: percent(100),
                    height: px(tokens::ROW_HEIGHT),
                    column_gap: px(tokens::SPACING_SM),
                    border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_MD)),
                    margin: UiRect::vertical(px(tokens::SPACING_XS)),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
                BackgroundColor(tokens::ELEVATED_BG),
                observe(
                    |hover: On<Pointer<Over>>, mut bg: Query<&mut BackgroundColor>| {
                        if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
                            bg.0 = tokens::TOOLBAR_ACTIVE_BG;
                        }
                    },
                ),
                observe(
                    |out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor>| {
                        if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                            bg.0 = tokens::ELEVATED_BG;
                        }
                    },
                ),
                observe(|mut click: On<Pointer<Click>>, mut commands: Commands| {
                    click.propagate(false);
                    commands.queue(|world: &mut World| {
                        world.run_system_cached(crate::add_entity_picker::open_add_entity_picker)
                    });
                },),
                children![
                    (
                        Text::new(String::from(Icon::PackagePlus.unicode())),
                        TextFont {
                            font: add_entity_icon_font,
                            font_size: tokens::ICON_SM,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_PRIMARY),
                    ),
                    (
                        Text::new("Add Entity"),
                        TextFont {
                            font_size: tokens::TEXT_SIZE,
                            weight: FontWeight::MEDIUM,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_PRIMARY),
                    ),
                ],
            ),
            (
                HierarchyTreeContainer,
                Node {
                    flex_direction: FlexDirection::Column,
                    width: percent(100),
                    flex_grow: 1.0,
                    min_height: px(0.0),
                    overflow: Overflow::scroll_y(),
                    margin: UiRect::top(px(tokens::SPACING_SM)),
                    ..Default::default()
                },
                BackgroundColor(Color::NONE),
                tree_container_drop_observers(),
            ),
            (
                crate::status_bar::SceneStatsText,
                Text::new(""),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                TextLayout::new_with_justify(Justify::Center),
                Node {
                    padding: UiRect::all(px(tokens::SPACING_XS)),
                    flex_shrink: 0.0,
                    width: percent(100),
                    ..Default::default()
                },
            )
        ],
    )
}

fn scene_view() -> impl Bundle {
    (
        EditorEntity,
        SceneViewport,
        Node {
            width: percent(100),
            flex_grow: 1.0,
            ..Default::default()
        },
    )
}

/// Updates toolbar button backgrounds to highlight the active gizmo mode.
pub fn update_toolbar_highlights(
    mode: Res<GizmoMode>,
    mut buttons: Query<(&GizmoModeButton, &mut BackgroundColor)>,
) {
    if !mode.is_changed() {
        return;
    }
    for (button, mut bg) in &mut buttons {
        bg.0 = if button.0 == *mode {
            tokens::TOOLBAR_ACTIVE_BG
        } else {
            tokens::TOOLBAR_BUTTON_BG
        };
    }
}

/// Updates the gizmo space toggle button label.
pub fn update_space_toggle_label(
    space: Res<GizmoSpace>,
    buttons: Query<&Children, With<GizmoSpaceButton>>,
    mut texts: Query<&mut Text, With<ThemedText>>,
) {
    if !space.is_changed() {
        return;
    }
    let label = match *space {
        GizmoSpace::World => "World",
        GizmoSpace::Local => "Local",
    };
    for children in &buttons {
        for child in children.iter() {
            if let Ok(mut text) = texts.get_mut(child) {
                text.0 = label.to_string();
                return;
            }
        }
    }
}

/// Updates edit tool button backgrounds to highlight the active edit mode/draw state.
pub(crate) fn update_edit_tool_highlights(
    edit_mode: Res<EditMode>,
    draw_state: Res<DrawBrushState>,
    active_modal: ActiveModalQuery,
    mut buttons: Query<(&EditToolButton, &mut BackgroundColor)>,
) {
    if !edit_mode.is_changed() && !draw_state.is_changed() {
        return;
    }
    let draw_active = draw_state.active.is_some();
    for (button, mut bg) in &mut buttons {
        let active = match button {
            EditToolButton::Object => !draw_active && *edit_mode == EditMode::Object,
            EditToolButton::Operator(op) => active_modal.is_operator(op),
            EditToolButton::Vertex => {
                !draw_active && *edit_mode == EditMode::BrushEdit(BrushEditMode::Vertex)
            }
            EditToolButton::Edge => {
                !draw_active && *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge)
            }
            EditToolButton::Face => {
                !draw_active && *edit_mode == EditMode::BrushEdit(BrushEditMode::Face)
            }
            EditToolButton::Clip => {
                !draw_active && *edit_mode == EditMode::BrushEdit(BrushEditMode::Clip)
            }
            EditToolButton::Physics => !draw_active && *edit_mode == EditMode::Physics,
        };
        bg.0 = if active {
            tokens::TOOLBAR_ACTIVE_BG
        } else {
            tokens::TOOLBAR_BUTTON_BG
        };
    }
}

/// Toggle document-root visibility when the active tab changes.
pub fn update_active_document_display(
    active: Res<ActiveDocument>,
    mut roots: Query<(&DocumentRoot, &mut Node)>,
) {
    if !active.is_changed() {
        return;
    }
    for (root, mut node) in &mut roots {
        node.display = if root.0 == active.kind {
            Display::Flex
        } else {
            Display::None
        };
    }
}

/// Refresh tab-strip styling. Active tab gets its bg + border; inactive
/// tabs go transparent. Schedule Explorer dims when Remote is
/// disconnected.
pub fn update_tab_strip_highlights(
    active: Res<ActiveDocument>,
    manager: Res<ConnectionManager>,
    mut tabs: Query<(
        &DocumentTabButton,
        &mut BackgroundColor,
        &mut BorderColor,
        &Children,
    )>,
    mut texts: Query<&mut TextColor>,
) {
    if !active.is_changed() && !manager.is_changed() {
        return;
    }
    let connected = manager.is_connected();
    for (tab, mut tab_bg, mut tab_border, children) in &mut tabs {
        let is_active = tab.0 == active.kind;
        let is_disabled = tab.0 == TabKind::ScheduleExplorer && !connected;

        tab_bg.0 = if is_active {
            tokens::DOC_TAB_ACTIVE_BG
        } else {
            Color::NONE
        };
        *tab_border = BorderColor::all(if is_active {
            tokens::DOC_TAB_ACTIVE_BORDER
        } else {
            Color::NONE
        });

        let label_color = if is_disabled {
            Color::srgba(0.4, 0.4, 0.4, 0.5)
        } else if is_active {
            tokens::DOC_TAB_ACTIVE_LABEL
        } else {
            tokens::DOC_TAB_INACTIVE_LABEL
        };

        // First child is the accent strip; skip it (its color is
        // type-fixed). Second and third children are the icon and
        // label text; refresh their colors.
        for child in children.iter().skip(1) {
            if let Ok(mut tc) = texts.get_mut(child) {
                tc.0 = label_color;
            }
        }
    }
}

fn bottom_dock_area() -> impl Bundle {
    (
        jackdaw_panels::reconcile::AnchorHost {
            anchor_id: "bottom_dock".into(),
            default_style: jackdaw_panels::DockAreaStyle::IconSidebar,
        },
        jackdaw_panels::DockArea {
            id: "bottom_dock".into(),
            style: jackdaw_panels::DockAreaStyle::IconSidebar,
        },
        EditorEntity,
        Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Row,
            overflow: Overflow::clip(),
            ..Default::default()
        },
        BackgroundColor(tokens::PANEL_BG),
    )
}

/// Custom status bar that wraps the feathers status bar sections and adds
/// a connection indicator on the far right.
fn editor_status_bar() -> impl Bundle {
    (
        status_bar::StatusBar,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            width: Val::Percent(100.0),
            height: Val::Px(tokens::STATUS_BAR_HEIGHT),
            padding: UiRect::horizontal(Val::Px(tokens::SPACING_MD)),
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::WINDOW_BG),
        children![
            (
                status_bar::StatusBarLeft,
                Text::new("Ready"),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                bevy::feathers::theme::ThemedText,
            ),
            (
                status_bar::StatusBarCenter,
                Text::new(""),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
            // Right side: gizmo info + connection indicator
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(tokens::SPACING_LG),
                    ..Default::default()
                },
                children![
                    (
                        status_bar::StatusBarRight,
                        Text::new(""),
                        TextFont {
                            font_size: tokens::FONT_SM,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_SECONDARY),
                    ),
                    // Connection indicator
                    crate::remote::panel::connection_indicator()
                ],
            )
        ],
    )
}

fn right_dock_area() -> impl Bundle {
    (
        jackdaw_panels::reconcile::AnchorHost {
            anchor_id: "right_sidebar".into(),
            default_style: jackdaw_panels::DockAreaStyle::TabBar,
        },
        jackdaw_panels::DockArea {
            id: "right_sidebar".into(),
            style: jackdaw_panels::DockAreaStyle::TabBar,
        },
        EditorEntity,
        Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            overflow: Overflow::clip(),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_LG)),
            ..Default::default()
        },
        BackgroundColor(tokens::PANEL_BG),
    )
}

pub fn inspector_components_content(icon_font: Handle<Font>) -> impl Bundle {
    (
        Node {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: px(0.0),
            ..Default::default()
        },
        children![
            (
                Node {
                    flex_direction: FlexDirection::Column,
                    width: percent(100),
                    padding: UiRect::all(px(tokens::SPACING_SM)),
                    row_gap: px(tokens::SPACING_XS),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
                children![
                    (
                        crate::inspector::InspectorSearch,
                        text_edit::text_edit(
                            TextEditProps::default()
                                .with_placeholder("Filter...")
                                .allow_empty()
                        ),
                    ),
                    (
                        crate::inspector::AddComponentButton,
                        Interaction::default(),
                        Node {
                            flex_direction: FlexDirection::Row,
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            width: percent(100),
                            height: px(tokens::ROW_HEIGHT),
                            column_gap: px(tokens::SPACING_SM),
                            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_MD)),
                            flex_shrink: 0.0,
                            ..Default::default()
                        },
                        BackgroundColor(tokens::ELEVATED_BG),
                        observe(
                            |hover: On<Pointer<Over>>, mut bg: Query<&mut BackgroundColor>| {
                                if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
                                    bg.0 = tokens::TOOLBAR_ACTIVE_BG;
                                }
                            },
                        ),
                        observe(
                            |out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor>| {
                                if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                                    bg.0 = tokens::ELEVATED_BG;
                                }
                            },
                        ),
                        children![
                            (
                                Text::new(String::from(Icon::PackagePlus.unicode())),
                                TextFont {
                                    font: icon_font,
                                    font_size: tokens::ICON_SM,
                                    ..Default::default()
                                },
                                TextColor(tokens::TEXT_PRIMARY),
                            ),
                            (
                                Text::new("Add Component"),
                                TextFont {
                                    font_size: tokens::TEXT_SIZE,
                                    weight: FontWeight::MEDIUM,
                                    ..Default::default()
                                },
                                TextColor(tokens::TEXT_PRIMARY),
                            ),
                        ],
                        observe(|click: On<Pointer<Click>>, mut commands: Commands| {
                            commands.trigger(jackdaw_feathers::button::ButtonClickEvent {
                                entity: click.event_target(),
                            });
                        },),
                    ),
                ],
            ),
            (
                Inspector,
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(tokens::SPACING_SM),
                    overflow: Overflow::scroll_y(),
                    flex_grow: 1.0,
                    min_height: px(0.0),
                    padding: UiRect::all(px(tokens::SPACING_SM)),
                    ..Default::default()
                }
            ),
        ],
    )
}
