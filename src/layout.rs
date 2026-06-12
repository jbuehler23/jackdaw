use bevy::{picking::hover::Hovered, prelude::*, ui_widgets::observe};
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    button::{self, ButtonOperatorCall, ButtonSize, ButtonVariant},
    icons::{EditorFont, IconFont},
    menu_bar, separator, split_panel, status_bar,
    text_edit::{self, TextEditProps},
    tokens,
    tree_view::tree_container_drop_observers,
};
use jackdaw_localization::LocalizedText;

use jackdaw_api::pie::PlayState;

use crate::{
    EditorEntity,
    active_tool::ActiveTool,
    brush::{BrushEditMode, EditMode},
    draw_brush::ActivateDrawBrushModalOp,
    edit_mode_ops::{
        EditModeClipOp, EditModeEdgeOp, EditModeFaceOp, EditModeKnifeOp, EditModeVertexOp,
    },
    gizmo_ops::GizmoSpaceToggleOp,
    gizmos::GizmoSpace,
    hierarchy::{HierarchyPanel, HierarchyShowAllButton, HierarchyTreeContainer},
    inspector::Inspector,
    measure_tool::MeasureDistanceOp,
    physics_tool::PhysicsActivateOp,
    pie::PieWindowModeToggleOp,
    pie_mirror::{PieViewHeader, PieViewMode, PieViewSegment},
    remote::ConnectionManager,
    tool_ops::{ToolRotateOp, ToolScaleOp, ToolSelectOp, ToolTranslateOp},
    viewport::SceneViewport,
    windowing::{JackdawIcon, title_bar_repo_link},
};
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
use bevy_window_chrome::CaptionFont;
use bevy_window_chrome::{WindowChromeTheme, spawn_window_shell};

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

fn spawn_editor_main_area(parent: &mut ChildSpawnerCommands) {
    // Scene document (visible by default).
    //
    // The dock tree is materialised by `jackdaw_panels`' reconciler
    // under this single host. The default tree (left | (center over
    // bottom) | right) is built in `init_layout` from registered
    // windows; the user can drag panels anywhere within it.
    parent.spawn((
        DocumentRoot(TabKind::Scene),
        EditorEntity,
        Node {
            width: percent(100),
            flex_grow: 1.0,
            min_height: px(0.0),
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            ..Default::default()
        },
        children![(
            jackdaw_panels::reconcile::DockTreeHost::default(),
            EditorEntity,
            Node {
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Row,
                overflow: Overflow::clip(),
                ..Default::default()
            },
        )],
    ));
    // Schedule Explorer document (hidden by default).
    parent.spawn((
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
    ));
    parent.spawn(editor_status_bar());
}

/// Fills a [`spawn_window_shell`] title bar/body pair with editor UI.
pub fn spawn_editor(
    commands: &mut Commands,
    title_bar: Entity,
    body: Entity,
    icon_font: Handle<Font>,
    editor_font: Handle<Font>,
    jackdaw_icon: Handle<Image>,
) {
    commands
        .entity(title_bar)
        .with_children(|title_bar_parent| {
            title_bar_parent.spawn(window_title_bar_content(
                icon_font.clone(),
                editor_font.clone(),
                jackdaw_icon,
            ));
        });
    commands.entity(body).insert((
        EditorEntity,
        Node {
            width: percent(100),
            height: percent(100),
            flex_grow: 1.0,
            min_height: px(0.0),
            flex_direction: FlexDirection::Column,
            padding: UiRect::horizontal(px(tokens::PANEL_GAP)),
            row_gap: px(tokens::PANEL_GAP),
            overflow: Overflow::clip(),
            ..Default::default()
        },
    ));
    commands.entity(body).with_children(spawn_editor_main_area);
}

/// Editor entry: UI camera, window shell, then editor chrome/content.
pub fn spawn_editor_layout(
    mut commands: Commands,
    theme: Res<WindowChromeTheme>,
    icon_font: Res<IconFont>,
    editor_font: Res<EditorFont>,
    jackdaw_icon: Res<JackdawIcon>,
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    caption_font: Res<CaptionFont>,
) {
    let slots = spawn_window_shell(
        &mut commands,
        &theme,
        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        caption_font,
        EditorEntity,
    );
    spawn_editor(
        &mut commands,
        slots.title_bar,
        slots.body,
        icon_font.0.clone(),
        editor_font.0.clone(),
        jackdaw_icon.0.clone(),
    );
}

fn window_title_bar_content(
    icon_font: Handle<Font>,
    editor_font: Handle<Font>,
    jackdaw_icon: Handle<Image>,
) -> impl Bundle {
    (
        EditorEntity,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            width: percent(100),
            height: percent(100),
            padding: UiRect::horizontal(px(tokens::SPACING_MD)),
            column_gap: px(tokens::SPACING_MD),
            ..Default::default()
        },
        Pickable::IGNORE,
        children![
            title_bar_repo_link(jackdaw_icon),
            menu_bar::menu_bar_shell(),
            (
                crate::scenes::ui::SceneTabStrip,
                EditorEntity,
                Pickable::IGNORE,
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    height: percent(100),
                    column_gap: px(4.0),
                    flex_shrink: 1.0,
                    flex_grow: 1.0,
                    min_width: px(0.0),
                    overflow: Overflow::scroll_x(),
                    ..Default::default()
                },
                ScrollPosition::default(),
            ),
            crate::workspace_dropdown::workspace_dropdown_trigger(editor_font, icon_font.clone()),
            play_pause_controls(icon_font),
        ],
    )
}

/// Play / Pause / Stop transport pill. Clicking a button triggers
/// the corresponding `PiePlugin` handler. The plugin installs a
/// click observer on each `PieButton` via an `On<Add, PieButton>`
/// observer, so wiring here is purely presentation.
fn play_pause_controls(icon_font: Handle<Font>) -> impl Bundle {
    (
        EditorEntity,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            height: px(tokens::HEADER_CONTROL_HEIGHT),
            padding: UiRect::horizontal(px(6.5)),
            column_gap: px(9.0),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_LG)),
            ..Default::default()
        },
        BackgroundColor(tokens::HEADER_CONTROL_BG),
        BorderColor::all(tokens::HEADER_CONTROL_BORDER),
        children![
            pie_transport_button(crate::pie::PieButton::Play, Icon::Play, icon_font.clone(),),
            pie_menu_button(icon_font.clone()),
            pie_transport_button(crate::pie::PieButton::Pause, Icon::Pause, icon_font.clone(),),
            pie_transport_button(crate::pie::PieButton::Stop, Icon::Square, icon_font.clone(),),
            pie_transport_button(crate::pie::PieButton::Reload, Icon::RefreshCw, icon_font),
            window_mode_button(),
        ],
    )
}

/// Caret button next to Play that opens the run-config dropdown. Shares
/// `pie_transport_button`'s glyph shape but carries the `PieMenuButton`
/// marker, which `PieMenuPlugin` observes to open the menu.
fn pie_menu_button(icon_font: Handle<Font>) -> impl Bundle {
    (
        crate::pie_menu::PieMenuButton,
        EditorEntity,
        Interaction::default(),
        Node {
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding: UiRect::horizontal(px(2.0)),
            ..Default::default()
        },
        children![(
            Text::new(String::from(Icon::ChevronDown.unicode())),
            TextFont {
                font: icon_font,
                font_size: 10.0,
                ..Default::default()
            },
            TextColor(tokens::HEADER_CONTROL_LABEL),
            Pickable::IGNORE,
        )],
    )
}

/// Single clickable glyph. The `PieButton` marker is the hook the
/// `PiePlugin` uses to attach the click observer. Lucide glyphs live
/// in the Private Use Area, so the icon font handle must be passed
/// explicitly: without it the default font (`FiraSans`) renders the
/// codepoints as tofu/`?`.
fn pie_transport_button(
    kind: crate::pie::PieButton,
    icon: Icon,
    icon_font: Handle<Font>,
) -> impl Bundle {
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
                font: icon_font,
                font_size: 13.0,
                ..Default::default()
            },
            TextColor(tokens::HEADER_CONTROL_LABEL),
            Pickable::IGNORE,
        )],
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

/// Bundle the editor toolbar and the `SceneViewport` node together so
/// `setup_viewport` can mount the whole thing inside the dock tree's
/// "center" leaf in one go. Public to the crate because it's spawned
/// by the viewport plugin, not by the editor body layout directly.
pub(crate) fn viewport_with_toolbar() -> impl Bundle {
    (
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
        children![
            toolbar(),
            crate::navmesh::toolbar::navmesh_toolbar(),
            crate::terrain::toolbar::terrain_toolbar(),
            scene_view(),
        ],
    )
}

fn toolbar() -> impl Bundle {
    // Every toolbar entry below goes through `feathers::button(...)`,
    // the same constructor extensions use. Active-state highlighting
    // is driven by [`update_toolbar_button_variants`] flipping
    // `ButtonVariant::Active` on the owning entity, so we never
    // mutate `BackgroundColor` directly and `handle_hover` stays the
    // sole bg writer.
    //
    // Sizing matches the Figma viewport-toolbar spec: 30px tall, 1px
    // border, top corners rounded against the panel below.
    (
        Toolbar,
        EditorEntity,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding: UiRect {
                left: px(tokens::TOOLBAR_PADDING_LEFT),
                right: px(tokens::TOOLBAR_PADDING_RIGHT),
                top: px(0.0),
                bottom: px(0.0),
            },
            column_gap: px(tokens::TOOLBAR_GAP),
            width: percent(100),
            height: px(tokens::TOOLBAR_HEIGHT),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius {
                top_left: px(tokens::TOOLBAR_RADIUS),
                top_right: px(tokens::TOOLBAR_RADIUS),
                bottom_left: px(0.0),
                bottom_right: px(0.0),
            },
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::PANEL_HEADER_BG),
        BorderColor::all(tokens::TOOLBAR_BORDER),
        children![
            toolbar_op_button::<ToolSelectOp>(Icon::MousePointer),
            toolbar_op_button::<ToolTranslateOp>(Icon::Move3d),
            toolbar_op_button::<ToolRotateOp>(Icon::Rotate3d),
            toolbar_op_button::<ToolScaleOp>(Icon::Scale3d),
            separator::separator(separator::SeparatorProps::vertical()),
            // Gizmo space toggle. Active highlight = `Local`; default
            // = `World`. Tooltip is the discoverability path.
            toolbar_op_button::<GizmoSpaceToggleOp>(Icon::Globe),
            separator::separator(separator::SeparatorProps::vertical()),
            toolbar_op_button::<ActivateDrawBrushModalOp>(Icon::Box),
            toolbar_op_button::<MeasureDistanceOp>(Icon::RulerDimensionLine),
            toolbar_op_button::<EditModeVertexOp>(Icon::CircleDot),
            toolbar_op_button::<EditModeEdgeOp>(Icon::GitCommitHorizontal),
            toolbar_op_button::<EditModeFaceOp>(Icon::Hexagon),
            toolbar_op_button::<EditModeClipOp>(Icon::ScissorsLineDashed),
            separator::separator(separator::SeparatorProps::vertical()),
            toolbar_op_button::<PhysicsActivateOp>(Icon::Zap),
        ],
    )
}

/// Spawn a square icon-only toolbar button bound to operator `Op`.
/// Identical to what an extension would write. The icon is the only
/// visible glyph; `ButtonSize::Icon` suppresses the content text
/// label, and the operator's label and description show in the rich
/// operator tooltip on hover via [`OperatorTooltipPlugin`].
///
/// Initial variant is `Ghost` so idle buttons render transparent
/// against the toolbar's `#1F1F24` panel; the
/// [`update_toolbar_button_variants`] system flips them to `Active`
/// when the matching mode/modal is current. Without this, every
/// button would sit on the muted `Default` grey and the toolbar
/// would lose the "one currently-active tool" reading.
///
/// [`OperatorTooltipPlugin`]: crate::operator_tooltip::OperatorTooltipPlugin
fn toolbar_op_button<Op: Operator>(icon: Icon) -> impl Bundle {
    button::button(
        ButtonProps::from_operator::<Op>()
            .with_variant(ButtonVariant::Ghost)
            .icon(icon)
            .with_size(ButtonSize::Icon),
    )
}

pub fn hierarchy_content(icon_font: Handle<Font>) -> impl Bundle {
    let add_entity_icon_font = icon_font.clone();
    let toggle_font = icon_font.clone();
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
                PieViewHeader,
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(tokens::SPACING_XS),
                    width: percent(100),
                    padding: UiRect::vertical(px(tokens::SPACING_XS)),
                    border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
                    ..Default::default()
                },
                BackgroundColor(Color::NONE),
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
                    pie_view_toggle(toggle_font),
                    live_badge(),
                    pie_instance_cycle_button(),
                    crate::live_edits_ui::live_edits_badge(),
                    (
                        HierarchyShowAllButton,
                        Interaction::default(),
                        Hovered::default(),
                        jackdaw_feathers::tooltip::Tooltip::title("Show All Entities")
                            .with_description(
                                "Toggle visibility of editor-internal entities and \
                                 hidden objects in the hierarchy.",
                            ),
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
                        LocalizedText::new("add-entity"),
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
                Text::default(),
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
            // Width reserved permanently so the Live accent border can be
            // toggled by color alone without shifting the viewport bounds.
            border: UiRect::all(px(2.0)),
            ..Default::default()
        },
        BorderColor::all(Color::NONE),
    )
}

/// Flip every toolbar button's [`ButtonVariant`] between `Default`
/// and `Active` based on the matching editor state. The feathers
/// `handle_hover` system reads the variant to compute the
/// background, so this is the only place toolbar active-state lives;
/// `BackgroundColor` is never mutated directly. New toolbar buttons
/// just need to register their operator id below to opt in.
///
/// Runs every frame: `ActiveModalOperator` is added/removed via
/// observers that don't trigger `Res::is_changed()` on any of the
/// scalar resources, so a change-detection short-circuit would miss
/// the start of a Draw Brush / Measure Distance / etc. session. The
/// loop is O(toolbar buttons), trivially cheap.
pub fn update_toolbar_button_variants(
    edit_mode: Res<EditMode>,
    active_tool: Res<ActiveTool>,
    gizmo_space: Res<GizmoSpace>,
    active_modal: ActiveModalQuery,
    mut buttons: Query<(&ButtonOperatorCall, &mut ButtonVariant)>,
) {
    let modal_running = active_modal.is_modal_running();
    for (call, mut variant) in &mut buttons {
        // While any modal is running only the modal's own button
        // highlights. Gizmo / mode buttons go quiet so the user sees
        // a single active tool at a time, matching how mature 3D editors
        // surfaces the current mode. New extension modal operators
        // pick this up automatically through the fall-through arm.
        let active = if modal_running {
            active_modal.is_operator(&call.id)
        } else if call.id == ToolTranslateOp::ID {
            *active_tool == ActiveTool::Translate
        } else if call.id == ToolRotateOp::ID {
            *active_tool == ActiveTool::Rotate
        } else if call.id == ToolScaleOp::ID {
            *active_tool == ActiveTool::Scale
        } else if call.id == GizmoSpaceToggleOp::ID {
            *gizmo_space == GizmoSpace::Local
        } else if call.id == ToolSelectOp::ID {
            *active_tool == ActiveTool::Select
        } else if call.id == EditModeVertexOp::ID {
            *edit_mode == EditMode::BrushEdit(BrushEditMode::Vertex)
        } else if call.id == EditModeEdgeOp::ID {
            *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge)
        } else if call.id == EditModeFaceOp::ID {
            *edit_mode == EditMode::BrushEdit(BrushEditMode::Face)
        } else if call.id == EditModeClipOp::ID {
            *edit_mode == EditMode::BrushEdit(BrushEditMode::Clip)
        } else if call.id == EditModeKnifeOp::ID {
            *edit_mode == EditMode::BrushEdit(BrushEditMode::Knife)
        } else if call.id == PhysicsActivateOp::ID {
            *edit_mode == EditMode::Physics
        } else {
            false
        };
        // Inactive toolbar buttons fall back to `Ghost` (transparent)
        // so only the active one stands out as solid grey. Using
        // `Default` here would tint every idle button with the muted
        // ZINC_700 fill at ~50% alpha and they'd all read as
        // "highlighted" against the toolbar's dark panel.
        let target = if active {
            ButtonVariant::Active
        } else {
            ButtonVariant::Ghost
        };
        if *variant != target {
            *variant = target;
        }
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
                LocalizedText::new("ready"),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                bevy::feathers::theme::ThemedText,
            ),
            (
                status_bar::StatusBarCenter,
                Text::default(),
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
                        Text::default(),
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

pub fn inspector_components_content(icon_font: Handle<Font>) -> impl Bundle {
    let save_font = icon_font.clone();
    (
        Node {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: px(0.0),
            ..Default::default()
        },
        children![
            (
                PieViewHeader,
                Node {
                    flex_direction: FlexDirection::Column,
                    width: percent(100),
                    padding: UiRect::all(px(tokens::SPACING_SM)),
                    row_gap: px(tokens::SPACING_XS),
                    flex_shrink: 0.0,
                    border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
                    ..Default::default()
                },
                BackgroundColor(Color::NONE),
                children![
                    (
                        Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: px(tokens::SPACING_XS),
                            width: percent(100),
                            ..Default::default()
                        },
                        children![(
                            Node {
                                flex_grow: 1.0,
                                ..Default::default()
                            },
                            children![(
                                crate::inspector::InspectorSearch,
                                text_edit::text_edit(
                                    TextEditProps::default()
                                        .with_placeholder("Filter...")
                                        .allow_empty()
                                ),
                            )],
                        ),],
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
                                LocalizedText::new("add-component"),
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
                    save_to_scene_button(save_font),
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

/// "Save to Scene" button for the inspector header.
///
/// Promotes the selected running entity's runtime component values into its
/// authored scene node. Hidden in Scene mode; in Live mode it is shown and
/// enabled only when the selection maps back to an authored node (see
/// [`update_save_to_scene_button`]). Click is gated the same way, so a
/// dimmed button is inert.
fn save_to_scene_button(icon_font: Handle<Font>) -> impl Bundle {
    (
        crate::inspector::SaveToSceneButton,
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
            // Hidden until Live mode; the appearance system flips this.
            display: Display::None,
            ..Default::default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        observe(|hover: On<Pointer<Over>>, mut commands: Commands| {
            // Only the enabled button reacts to hover; a dimmed one stays
            // at its base color (the same condition the click path uses).
            let target = hover.event_target();
            commands.queue(move |world: &mut World| {
                if !crate::pie::can_save_live_to_scene(world) {
                    return;
                }
                if let Ok(mut e) = world.get_entity_mut(target)
                    && let Some(mut bg) = e.get_mut::<BackgroundColor>()
                {
                    bg.0 = tokens::TOOLBAR_ACTIVE_BG;
                }
            });
        }),
        observe(
            |out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor>| {
                if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                    bg.0 = tokens::ELEVATED_BG;
                }
            },
        ),
        children![
            (
                Text::new(String::from(Icon::Save.unicode())),
                TextFont {
                    font: icon_font,
                    font_size: tokens::ICON_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            ),
            (
                Text::new("Save to Scene"),
                TextFont {
                    font_size: tokens::TEXT_SIZE,
                    weight: FontWeight::MEDIUM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            ),
        ],
        observe(|_: On<Pointer<Click>>, mut commands: Commands| {
            commands.queue(|world: &mut World| {
                if crate::pie::can_save_live_to_scene(world) {
                    crate::pie::save_live_entity_to_scene(world);
                }
            });
        }),
    )
}

/// Show/enable the inspector's "Save to Scene" button.
///
/// Hidden in Scene mode. In Live mode it is shown; enabled (full color) when
/// `can_save_live_to_scene` is true (a projected entity is selected), otherwise
/// dimmed (the click and hover paths gate on the same condition, so dimmed is inert).
pub fn update_save_to_scene_button(world: &mut World) {
    let mode = *world.resource::<PieViewMode>();
    let live = mode == PieViewMode::Live;
    let enabled = live && crate::pie::can_save_live_to_scene(world);

    let text_color = if enabled {
        tokens::TEXT_PRIMARY
    } else {
        tokens::TEXT_DISABLED
    };

    let mut buttons: Vec<(Entity, Vec<Entity>)> = world
        .query_filtered::<(Entity, &Children), With<crate::inspector::SaveToSceneButton>>()
        .iter(world)
        .map(|(e, c)| (e, c.iter().collect()))
        .collect();

    for (button, children) in buttons.drain(..) {
        if let Ok(mut e) = world.get_entity_mut(button) {
            if let Some(mut node) = e.get_mut::<Node>() {
                node.display = if live { Display::Flex } else { Display::None };
            }
            if let Some(mut bg) = e.get_mut::<BackgroundColor>() {
                // Reset to the base color; the hover observer only brightens the
                // enabled button, and `Out` restores this same value.
                bg.0 = tokens::ELEVATED_BG;
            }
        }
        for child in children {
            if let Ok(mut e) = world.get_entity_mut(child)
                && let Some(mut tc) = e.get_mut::<TextColor>()
            {
                tc.0 = text_color;
            }
        }
    }
}

/// Build the two-segment Scene/Live toggle pill.
///
/// Each segment carries [`PieViewSegment`]. The click observer and
/// appearance system handle activation; only presentation lives here.
fn pie_view_toggle(icon_font: Handle<Font>) -> impl Bundle {
    (
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            overflow: Overflow::clip(),
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        children![
            pie_view_segment(PieViewSegment::Scene, "Scene", icon_font.clone()),
            pie_view_segment(PieViewSegment::Live, "Live", icon_font),
        ],
    )
}

/// One clickable segment inside the Scene/Live toggle.
fn pie_view_segment(
    segment: PieViewSegment,
    label: &'static str,
    icon_font: Handle<Font>,
) -> impl Bundle {
    (
        segment,
        Interaction::default(),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            column_gap: px(tokens::SPACING_XS),
            padding: UiRect::axes(px(tokens::SPACING_SM), px(2.0)),
            ..Default::default()
        },
        BackgroundColor(Color::NONE),
        observe(
            move |click: On<Pointer<Click>>,
                  mut commands: Commands,
                  play_state: Res<State<PlayState>>| {
                let _ = click;
                if segment == PieViewSegment::Live && *play_state.get() == PlayState::Stopped {
                    return;
                }
                commands.queue(move |world: &mut World| {
                    let new_mode = match segment {
                        PieViewSegment::Scene => PieViewMode::Scene,
                        PieViewSegment::Live => PieViewMode::Live,
                    };
                    let current = *world.resource::<PieViewMode>();
                    if current == new_mode {
                        return;
                    }
                    // Both directions despawn and replace the previewed
                    // entities (revert respawns authored entities with new
                    // ids; reproject despawns the ephemerals), so any
                    // selected entity becomes invalid across the toggle.
                    // Drop the selection before the teardown runs so the
                    // `On<Remove, Selected>` -> `on_entity_deselected`
                    // handler never tries to clear `TreeRowSelected` off a
                    // row that `teardown_outliner_rows` already despawned.
                    crate::selection::clear_selection_in_world(world);
                    match new_mode {
                        PieViewMode::Live => {
                            crate::pie::enter_live_view(world);
                        }
                        PieViewMode::Scene => {
                            *world.resource_mut::<PieViewMode>() = PieViewMode::Scene;
                            crate::pie_projection::revert_preview(world);
                        }
                    }
                });
            },
        ),
        children![
            (
                Text::new(label),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
            // Live-dot: only visible when this is the Live segment and mode is Live.
            // Shown as a small Radio icon glyph; hidden via display toggle.
            (
                PieViewLiveDot,
                Text::new(String::from(Icon::Radio.unicode())),
                TextFont {
                    font: icon_font,
                    font_size: 9.0,
                    ..Default::default()
                },
                TextColor(tokens::CATEGORY_SCENE),
                Node {
                    display: Display::None,
                    ..Default::default()
                },
            ),
        ],
    )
}

/// Marker on the live-dot glyph inside the Live segment.
#[derive(Component)]
pub struct PieViewLiveDot;

/// Update the appearance of all Scene/Live toggle segments across both panels.
///
/// Active segment gets primary text color and a filled background.
/// Inactive segment gets secondary text. Live segment is dimmed when
/// `PlayState` is `Stopped`.
pub fn update_pie_view_toggle_appearance(
    mode: Res<PieViewMode>,
    play_state: Res<State<PlayState>>,
    mut segments: Query<(&PieViewSegment, &mut BackgroundColor, &Children)>,
    mut texts: Query<(&mut TextColor, Option<&PieViewLiveDot>, Option<&mut Node>)>,
) {
    if !mode.is_changed() && !play_state.is_changed() {
        return;
    }
    let stopped = *play_state.get() == PlayState::Stopped;
    for (segment, mut bg, children) in &mut segments {
        let is_active = (*segment == PieViewSegment::Scene && *mode == PieViewMode::Scene)
            || (*segment == PieViewSegment::Live && *mode == PieViewMode::Live);
        let is_live_seg = *segment == PieViewSegment::Live;
        let disabled = is_live_seg && stopped;

        bg.0 = if is_active {
            tokens::TOOLBAR_ACTIVE_BG
        } else {
            Color::NONE
        };

        for child in children.iter() {
            if let Ok((mut tc, dot, mut node_opt)) = texts.get_mut(child) {
                if dot.is_some() {
                    // Live-dot glyph: show only when Live is active.
                    if let Some(ref mut node) = node_opt {
                        node.display = if is_active && is_live_seg {
                            Display::Flex
                        } else {
                            Display::None
                        };
                    }
                } else {
                    // Label text.
                    tc.0 = if disabled {
                        tokens::TEXT_DISABLED
                    } else if is_active {
                        tokens::TEXT_PRIMARY
                    } else {
                        tokens::TEXT_SECONDARY
                    };
                }
            }
        }
    }
}

/// Signal Live mode with a subtle ambient tint: wash both panel header
/// containers toward the accent and draw the viewport border in the accent.
/// Restores both to their Scene-mode appearance on return.
///
/// Runs every frame so a header or viewport node respawned by a dock
/// rearrange picks the tint back up; the writes are guarded so unchanged
/// frames do not dirty the UI.
pub fn update_pie_view_header_accent(
    mode: Res<PieViewMode>,
    mut headers: Query<&mut BackgroundColor, With<PieViewHeader>>,
    mut viewport_border: Query<&mut BorderColor, With<SceneViewport>>,
) {
    let live = *mode == PieViewMode::Live;
    let header_color = if live {
        crate::default_style::LIVE_HEADER_TINT
    } else {
        Color::NONE
    };
    for mut bg in &mut headers {
        if bg.0 != header_color {
            bg.0 = header_color;
        }
    }
    // The border width is reserved permanently on the viewport node, so only
    // the color flips here; no layout shift on toggle.
    let border_color = if live {
        crate::default_style::LIVE_ACCENT
    } else {
        Color::NONE
    };
    let target_border = BorderColor::all(border_color);
    for mut border in &mut viewport_border {
        if *border != target_border {
            *border = target_border;
        }
    }
}

/// Marker on the bold `LIVE` badge in the hierarchy header. Visible only
/// while [`PieViewMode`] is `Live`; its text names the focused instance.
#[derive(Component)]
pub struct LiveBadge;

/// Render the badge label for the focused instance: a bare `LIVE` when no
/// instance is focused, otherwise `LIVE  <instance>` (the same instance
/// label the picker shows, e.g. `LIVE  Client #1`).
fn live_badge_label(focused: Option<&crate::pie::InstanceKey>) -> String {
    match focused {
        Some(key) => format!("LIVE  {key}"),
        None => "LIVE".to_string(),
    }
}

/// Build the bold `LIVE` badge that sits next to the consolidated mode
/// control. Hidden outside Live mode; [`update_live_badge`] flips its
/// display and keeps the focused-instance name current.
fn live_badge() -> impl Bundle {
    (
        LiveBadge,
        Node {
            align_items: AlignItems::Center,
            padding: UiRect::axes(px(tokens::SPACING_SM), px(2.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            display: Display::None,
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        children![(
            Text::new("LIVE"),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(crate::default_style::LIVE_ACCENT),
        )],
    )
}

/// Keep the `LIVE` badge's label and visibility in sync with the view mode
/// and focused instance. Shown only in Live mode; the label tracks the
/// focused instance the same way the instance picker renders it.
///
/// Runs every frame so a badge respawned by a dock rearrange recovers its
/// state; the writes are guarded so unchanged frames do not dirty the UI.
pub fn update_live_badge(
    mode: Res<PieViewMode>,
    instances: Res<crate::pie_mirror::PieInstances>,
    mut badges: Query<(&mut Node, &Children), With<LiveBadge>>,
    mut labels: Query<&mut Text>,
) {
    let display = if *mode == PieViewMode::Live {
        Display::Flex
    } else {
        Display::None
    };
    let label = live_badge_label(instances.focused.as_ref());
    for (mut node, children) in &mut badges {
        if node.display != display {
            node.display = display;
        }
        for child in children.iter() {
            if let Ok(mut text) = labels.get_mut(child)
                && text.0 != label
            {
                text.0 = label.clone();
            }
        }
    }
}

/// Marker on the compact cycling button in the hierarchy header that steps
/// through focused instances in Live mode.
#[derive(Component)]
pub struct PieInstanceCycleButton;

/// Marker on the text node inside the cycle button that shows the focused
/// instance label.
#[derive(Component)]
pub struct PieFocusedInstanceLabel;

/// Build the compact cycling button that shows the focused instance label.
///
/// Hidden when not in Live mode. In Live mode, clicking it advances focus
/// to the next running instance via [`crate::pie_projection::set_focused_instance`].
/// Only visible/relevant when a play session is active.
fn pie_instance_cycle_button() -> impl Bundle {
    (
        PieInstanceCycleButton,
        Interaction::default(),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding: UiRect::axes(px(tokens::SPACING_SM), px(2.0)),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            // Hidden until Live mode; the appearance system flips this.
            display: Display::None,
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        observe(|_: On<Pointer<Click>>, mut commands: Commands| {
            commands.queue(|world: &mut World| {
                cycle_focused_instance(world);
            });
        }),
        children![(
            PieFocusedInstanceLabel,
            Text::new(String::new()),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
    )
}

/// Advance focus to the next running instance, wrapping around. Called on
/// cycle button click. A no-op when fewer than two instances are running.
fn cycle_focused_instance(world: &mut World) {
    let instances = world.resource::<crate::pie_mirror::PieInstances>();
    let focused = instances.focused.clone();
    let mut keys: Vec<crate::pie::InstanceKey> = instances.buffers.keys().cloned().collect();
    if keys.len() <= 1 {
        return;
    }
    keys.sort_by(|a, b| a.config.cmp(&b.config).then(a.instance.cmp(&b.instance)));
    let next = match &focused {
        None => keys.into_iter().next(),
        Some(current) => {
            let pos = keys.iter().position(|k| k == current);
            match pos {
                None => keys.into_iter().next(),
                Some(idx) => {
                    let next_idx = (idx + 1) % keys.len();
                    keys.into_iter().nth(next_idx)
                }
            }
        }
    };
    if let Some(key) = next {
        crate::pie_projection::set_focused_instance(world, key);
    }
}

/// Keep the instance cycle button's label and visibility in sync with the
/// current [`PieViewMode`] and [`PieInstances`](crate::pie_mirror::PieInstances) state.
///
/// Hidden in Scene mode. In Live mode, shows the focused instance label;
/// dims it when only one instance is running (cycling would be a no-op).
pub fn update_pie_instance_cycle_button(
    mode: Res<PieViewMode>,
    instances: Res<crate::pie_mirror::PieInstances>,
    play_state: Res<State<jackdaw_api::pie::PlayState>>,
    mut buttons: Query<(&mut Node, &mut BackgroundColor, &Children), With<PieInstanceCycleButton>>,
    mut labels: Query<(&mut Text, &mut TextColor), With<PieFocusedInstanceLabel>>,
) {
    if !mode.is_changed() && !instances.is_changed() && !play_state.is_changed() {
        return;
    }
    let live =
        *mode == PieViewMode::Live && *play_state.get() != jackdaw_api::pie::PlayState::Stopped;

    let running_count = instances.buffers.len();
    let label_text = instances
        .focused
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();

    for (mut node, mut bg, children) in &mut buttons {
        node.display = if live && running_count >= 1 {
            Display::Flex
        } else {
            Display::None
        };
        bg.0 = tokens::ELEVATED_BG;
        for child in children.iter() {
            if let Ok((mut text, mut tc)) = labels.get_mut(child) {
                text.0 = label_text.clone();
                tc.0 = if running_count > 1 {
                    tokens::TEXT_PRIMARY
                } else {
                    tokens::TEXT_SECONDARY
                };
            }
        }
    }
}

/// Marker on the button that picks whether the next launched game renders
/// into the viewport or opens a separate window.
#[derive(Component)]
pub struct WindowModeButton;

/// Marker on the text node inside [`WindowModeButton`] that names the current
/// [`PieWindowMode`](crate::pie::PieWindowMode).
#[derive(Component)]
pub struct WindowModeLabel;

/// Build the button that flips the next launch between an embedded
/// (viewport) game and a separate game window.
///
/// Always visible. Clicking dispatches `pie.window_mode_toggle`;
/// [`update_window_mode_button`] keeps the label naming the current mode.
fn window_mode_button() -> impl Bundle {
    (
        WindowModeButton,
        Interaction::default(),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding: UiRect::axes(px(tokens::SPACING_SM), px(2.0)),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            display: Display::Flex,
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        jackdaw_feathers::tooltip::Tooltip::title("Game window: embedded or separate window"),
        observe(|_: On<Pointer<Click>>, mut commands: Commands| {
            commands
                .operator(PieWindowModeToggleOp::ID)
                .settings(CallOperatorSettings {
                    execution_context: ExecutionContext::Invoke,
                    creates_history_entry: false,
                })
                .call();
        }),
        children![(
            WindowModeLabel,
            Text::new(String::new()),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
    )
}

/// Keep the window-mode button label current.
pub fn update_window_mode_button(
    mode: Res<crate::pie::PieWindowMode>,
    buttons: Query<&Children, With<WindowModeButton>>,
    mut labels: Query<&mut Text, With<WindowModeLabel>>,
) {
    let label = match *mode {
        crate::pie::PieWindowMode::Embedded => "Embedded",
        crate::pie::PieWindowMode::Windowed => "Windowed",
    };
    for children in &buttons {
        for child in children.iter() {
            if let Ok(mut text) = labels.get_mut(child)
                && text.0 != label
            {
                text.0 = label.to_string();
            }
        }
    }
}

#[cfg(test)]
mod live_badge_tests {
    use super::*;
    use crate::pie::InstanceKey;

    #[test]
    fn label_without_focus_is_bare_live() {
        assert_eq!(live_badge_label(None), "LIVE");
    }

    #[test]
    fn label_with_focus_names_the_instance() {
        let key = InstanceKey {
            config: "Client".to_string(),
            instance: 1,
        };
        assert_eq!(live_badge_label(Some(&key)), "LIVE  Client #1");
    }

    #[test]
    fn header_accent_tracks_view_mode() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        let header = world
            .spawn((PieViewHeader, BackgroundColor(Color::NONE)))
            .id();
        let viewport = world
            .spawn((SceneViewport, BorderColor::all(Color::NONE)))
            .id();

        world.insert_resource(PieViewMode::Live);
        world
            .run_system_once(update_pie_view_header_accent)
            .expect("system runs");

        assert_eq!(
            world.get::<BackgroundColor>(header).unwrap().0,
            crate::default_style::LIVE_HEADER_TINT,
            "the header tints to the live wash"
        );
        assert_eq!(
            world.get::<BorderColor>(viewport).unwrap().top,
            crate::default_style::LIVE_ACCENT,
            "the viewport border picks up the live accent"
        );

        world.insert_resource(PieViewMode::Scene);
        world
            .run_system_once(update_pie_view_header_accent)
            .expect("system runs");

        assert_eq!(
            world.get::<BackgroundColor>(header).unwrap().0,
            Color::NONE,
            "the header clears back to transparent in Scene mode"
        );
        assert_eq!(
            world.get::<BorderColor>(viewport).unwrap().top,
            Color::NONE,
            "the viewport border clears back to transparent in Scene mode"
        );
    }
}
