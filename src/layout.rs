use bevy::{
    feathers::{
        theme::{ThemeBackgroundColor, ThemedText},
        tokens as bevy_tokens,
    },
    prelude::*,
    ui_widgets::observe,
};
use jackdaw_feathers::{
    icons::{Icon, IconFont},
    menu_bar, panel_header, popover, separator, split_panel, status_bar,
    text_edit::{self, TextEditProps},
    tokens,
    tree_view::tree_container_drop_observers,
};

use crate::{
    EditorEntity, asset_browser,
    brush::{BrushEditMode, BrushSelection, EditMode},
    draw_brush::DrawBrushState,
    gizmos::{GizmoMode, GizmoSpace},
    hierarchy::{HierarchyPanel, HierarchyTreeContainer},
    inspector::Inspector,
    material_browser,
    selection::Selection,
    viewport::SceneViewport,
};

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
    Draw,
    Vertex,
    Edge,
    Face,
    Clip,
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
        ThemeBackgroundColor(bevy_tokens::WINDOW_BG),
        Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            ..Default::default()
        },
        children![
            // Menu bar (fixed height, populated in spawn_layout)
            menu_bar::menu_bar_shell(),
            // Main content (flex grow)
            (
                EditorEntity,
                Node {
                    width: percent(100),
                    flex_grow: 1.0,
                    min_height: px(0.0),
                    flex_direction: FlexDirection::Column,
                    ..Default::default()
                },
                // Vertical split: main area (top) + bottom panels (bottom)
                split_panel::panel_group(
                    0.15,
                    (
                        Spawn((split_panel::panel(4), main_area(font.clone()))),
                        Spawn(split_panel::panel_handle()),
                        Spawn((split_panel::panel(1), bottom_panels(font))),
                    ),
                ),
            ),
            // Status bar (fixed height)
            status_bar::status_bar()
        ],
    )
}

fn main_area(icon_font: Handle<Font>) -> impl Bundle {
    (
        EditorEntity,
        Node {
            width: percent(100),
            height: percent(100),
            ..Default::default()
        },
        // Horizontal split: hierarchy | viewport | inspector
        split_panel::panel_group(
            0.2,
            (
                Spawn((split_panel::panel(1), entity_heiarchy())),
                Spawn(split_panel::panel_handle()),
                Spawn((split_panel::panel(4), viewport_with_toolbar(icon_font))),
                Spawn(split_panel::panel_handle()),
                Spawn((split_panel::panel(1), entity_inspector())),
            ),
        ),
    )
}

fn viewport_with_toolbar(icon_font: Handle<Font>) -> impl Bundle {
    (
        EditorEntity,
        Node {
            height: percent(100),
            flex_direction: FlexDirection::Column,
            ..Default::default()
        },
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
        BackgroundColor(tokens::TOOLBAR_BG),
        children![
            // Gizmo mode buttons
            toolbar_button(Icon::Move, "", GizmoMode::Translate, icon_font.clone()),
            toolbar_button(Icon::RotateCw, "R", GizmoMode::Rotate, icon_font.clone()),
            toolbar_button(Icon::Scaling, "T", GizmoMode::Scale, icon_font.clone()),
            // Separator
            separator::separator(separator::SeparatorProps::vertical()),
            // Space toggle
            toolbar_space_button(f.clone()),
            // Separator
            separator::separator(separator::SeparatorProps::vertical()),
            // Edit mode buttons
            toolbar_edit_button(Icon::MousePointer, EditToolButton::Object, f.clone()),
            toolbar_edit_button(Icon::Box, EditToolButton::Draw, f.clone()),
            toolbar_edit_button(Icon::CircleDot, EditToolButton::Vertex, f.clone()),
            toolbar_edit_button(Icon::GitCommitHorizontal, EditToolButton::Edge, f.clone()),
            toolbar_edit_button(Icon::Hexagon, EditToolButton::Face, f.clone()),
            toolbar_edit_button(Icon::ScissorsLineDashed, EditToolButton::Clip, f.clone()),
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

fn toolbar_button(icon: Icon, label: &str, mode: GizmoMode, font: Handle<Font>) -> impl Bundle {
    let label = label.to_string();
    (
        GizmoModeButton(mode),
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
                    font_size: tokens::FONT_MD,
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
        observe(
            move |_: On<Pointer<Click>>, mut gizmo_mode: ResMut<GizmoMode>| {
                *gizmo_mode = mode;
            },
        ),
    )
}

fn toolbar_space_button(icon_font: Handle<Font>) -> impl Bundle {
    (
        GizmoSpaceButton,
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
        observe(|_: On<Pointer<Click>>, mut space: ResMut<GizmoSpace>| {
            *space = match *space {
                GizmoSpace::World => GizmoSpace::Local,
                GizmoSpace::Local => GizmoSpace::World,
            };
        }),
    )
}

fn toolbar_edit_button(icon: Icon, tool: EditToolButton, font: Handle<Font>) -> impl Bundle {
    (
        tool,
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
                font_size: tokens::FONT_MD,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
        observe(
            move |_: On<Pointer<Click>>,
                  mut edit_mode: ResMut<EditMode>,
                  mut brush_selection: ResMut<BrushSelection>,
                  mut draw_state: ResMut<DrawBrushState>,
                  selection: Res<Selection>,
                  brushes: Query<(), With<jackdaw_jsn::Brush>>| {
                match tool {
                    EditToolButton::Object => {
                        *edit_mode = EditMode::Object;
                        brush_selection.entity = None;
                        brush_selection.faces.clear();
                        brush_selection.vertices.clear();
                        brush_selection.edges.clear();
                        draw_state.active = None;
                    }
                    EditToolButton::Draw => {
                        // Toggle draw mode
                        if draw_state.active.is_some() {
                            draw_state.active = None;
                        } else {
                            // Exit brush edit mode if active
                            if *edit_mode != EditMode::Object {
                                *edit_mode = EditMode::Object;
                                brush_selection.entity = None;
                                brush_selection.faces.clear();
                                brush_selection.vertices.clear();
                                brush_selection.edges.clear();
                            }
                            // Check if a brush is selected for append mode
                            let append_target =
                                selection.primary().filter(|&e| brushes.contains(e));
                            draw_state.active = Some(crate::draw_brush::ActiveDraw {
                                corner1: Vec3::ZERO,
                                corner2: Vec3::ZERO,
                                depth: 0.0,
                                phase: crate::draw_brush::DrawPhase::PlacingFirstCorner,
                                mode: crate::draw_brush::DrawMode::Add,
                                plane: crate::draw_brush::DrawPlane {
                                    origin: Vec3::ZERO,
                                    normal: Vec3::Y,
                                    axis_u: Vec3::X,
                                    axis_v: Vec3::Z,
                                },
                                extrude_start_cursor: Vec2::ZERO,
                                plane_locked: false,
                                cursor_on_plane: None,
                                append_target,
                                drag_footprint: false,
                                press_screen_pos: None,
                                polygon_vertices: Vec::new(),
                                polygon_cursor: None,
                            });
                        }
                    }
                    EditToolButton::Vertex
                    | EditToolButton::Edge
                    | EditToolButton::Face
                    | EditToolButton::Clip => {
                        // Cancel draw mode if active
                        draw_state.active = None;

                        let target_mode = match tool {
                            EditToolButton::Vertex => BrushEditMode::Vertex,
                            EditToolButton::Edge => BrushEditMode::Edge,
                            EditToolButton::Face => BrushEditMode::Face,
                            EditToolButton::Clip => BrushEditMode::Clip,
                            _ => unreachable!(),
                        };

                        if let EditMode::BrushEdit(current) = *edit_mode {
                            if current == target_mode {
                                // Same mode: toggle off
                                *edit_mode = EditMode::Object;
                                brush_selection.entity = None;
                                brush_selection.faces.clear();
                                brush_selection.vertices.clear();
                                brush_selection.edges.clear();
                            } else {
                                // Switch sub-mode
                                *edit_mode = EditMode::BrushEdit(target_mode);
                                brush_selection.faces.clear();
                                brush_selection.vertices.clear();
                                brush_selection.edges.clear();
                                brush_selection.temporary_mode = false;
                            }
                        } else {
                            // Enter edit on primary if it's a brush
                            if let Some(entity) =
                                selection.primary().filter(|&e| brushes.contains(e))
                            {
                                *edit_mode = EditMode::BrushEdit(target_mode);
                                brush_selection.entity = Some(entity);
                                brush_selection.faces.clear();
                                brush_selection.vertices.clear();
                                brush_selection.edges.clear();
                                brush_selection.temporary_mode = false;
                            }
                        }
                    }
                }
            },
        ),
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
             mut popover_state: ResMut<KeybindHelpPopover>| {
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
                                spawn_keybind_help_content(scroll_parent);
                            });
                    })
                    .id();

                popover_state.entity = Some(popover_entity);
            },
        ),
    )
}

fn spawn_keybind_help_content(parent: &mut ChildSpawnerCommands) {
    let sections: &[(&str, &[(&str, &str)])] = &[
        (
            "Navigation",
            &[
                ("RMB + Drag", "Look around"),
                ("W/A/S/D", "Move"),
                ("Q/E", "Up / Down"),
                ("Shift", "Double speed"),
                ("Scroll", "Dolly forward/back"),
                ("RMB + Scroll", "Adjust move speed"),
                ("F", "Focus selected"),
                ("Ctrl+1-9", "Save camera bookmark"),
                ("1-9", "Restore bookmark"),
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
                ("Esc", "Translate mode"),
                ("R", "Rotate mode"),
                ("T", "Scale mode"),
                ("X", "Toggle local/world"),
                ("MMB", "Toggle snap"),
                ("Ctrl", "Toggle snap (during drag)"),
                ("Arrows", "Nudge (grid-unit)"),
                ("Alt+Arrows", "90° rotate"),
                ("PgUp / PgDn", "Nudge vertical"),
            ],
        ),
        (
            "Entity",
            &[
                ("Delete", "Delete"),
                ("Ctrl+D", "Duplicate"),
                ("Ctrl+C / Ctrl+V", "Copy / Paste components"),
                ("H", "Toggle visibility"),
                ("Alt+G", "Reset position"),
                ("Alt+R", "Reset rotation"),
                ("Alt+S", "Reset scale"),
            ],
        ),
        (
            "Brush Edit",
            &[
                ("1", "Vertex mode (toggle)"),
                ("2", "Edge mode (toggle)"),
                ("3", "Face mode (toggle)"),
                ("4", "Clip mode (toggle)"),
                ("Shift+Click", "Multi-select"),
                ("Click+Drag", "Move selected"),
                ("X/Y/Z", "Constrain axis (during drag)"),
                ("Delete", "Delete selected"),
                ("Enter", "Apply clip"),
                ("Esc", "Exit edit / Cancel drag"),
            ],
        ),
        (
            "Brush Draw",
            &[
                ("B", "Draw brush (add)"),
                ("C", "Draw brush (cut)"),
                ("Tab", "Toggle Add/Cut"),
                ("Click", "Place vertex / advance"),
                ("Enter", "Close polygon"),
                ("Backspace", "Remove last vertex"),
                ("Esc / Right-click", "Cancel"),
            ],
        ),
        (
            "View",
            &[
                ("Ctrl+Shift+W", "Toggle wireframe"),
                ("[  /  ]", "Grid size down/up"),
                ("Ctrl+Alt+Scroll", "Grid size"),
            ],
        ),
        (
            "File",
            &[
                ("Ctrl+S", "Save scene"),
                ("Ctrl+O", "Open scene"),
                ("Ctrl+Shift+N", "New scene"),
                ("Ctrl+Z", "Undo"),
                ("Ctrl+Shift+Z", "Redo"),
            ],
        ),
    ];

    for (section_title, bindings) in sections {
        // Section header
        parent.spawn((
            Text::new(*section_title),
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

        for (key, desc) in *bindings {
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
                        Text::new(*key),
                        TextFont {
                            font_size: tokens::FONT_SM,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_PRIMARY),
                    ),
                    (
                        Text::new(*desc),
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

fn entity_heiarchy() -> impl Bundle {
    (
        HierarchyPanel,
        Node {
            height: percent(100),
            flex_direction: FlexDirection::Column,
            ..Default::default()
        },
        BackgroundColor(tokens::PANEL_BG),
        children![
            panel_header::panel_header("Hierarchy"),
            (
                Node {
                    flex_direction: FlexDirection::Column,
                    flex_grow: 1.0,
                    min_height: px(0.0),
                    padding: UiRect::all(px(tokens::SPACING_SM)),
                    ..Default::default()
                },
                children![
                    (
                        HierarchyFilter,
                        text_edit::text_edit(
                            TextEditProps::default()
                                .with_placeholder("Filter entities")
                                .allow_empty()
                        )
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
                    )
                ],
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
            tokens::SELECTED_BG
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
pub fn update_edit_tool_highlights(
    edit_mode: Res<EditMode>,
    draw_state: Res<DrawBrushState>,
    mut buttons: Query<(&EditToolButton, &mut BackgroundColor)>,
) {
    if !edit_mode.is_changed() && !draw_state.is_changed() {
        return;
    }
    let draw_active = draw_state.active.is_some();
    for (button, mut bg) in &mut buttons {
        let active = match button {
            EditToolButton::Object => !draw_active && *edit_mode == EditMode::Object,
            EditToolButton::Draw => draw_active,
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
        };
        bg.0 = if active {
            tokens::SELECTED_BG
        } else {
            tokens::TOOLBAR_BUTTON_BG
        };
    }
}

fn bottom_panels(icon_font: Handle<Font>) -> impl Bundle {
    (
        EditorEntity,
        Node {
            width: percent(100),
            height: percent(100),
            ..Default::default()
        },
        // Horizontal split: asset browser | material browser
        split_panel::panel_group(
            0.15,
            (
                Spawn((
                    split_panel::panel(2),
                    asset_browser::asset_browser_panel(icon_font.clone()),
                )),
                Spawn(split_panel::panel_handle()),
                Spawn((
                    split_panel::panel(1),
                    material_browser::material_browser_panel(icon_font),
                )),
            ),
        ),
    )
}

fn entity_inspector() -> impl Bundle {
    (
        Node {
            height: percent(100),
            flex_direction: FlexDirection::Column,
            ..Default::default()
        },
        BackgroundColor(tokens::PANEL_BG),
        children![
            panel_header::panel_header("Inspector"),
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
