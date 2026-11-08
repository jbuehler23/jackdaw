use bevy::app::PropagateOver;
use bevy::feathers::controls::{ButtonVariant, FeathersToolButton};
use bevy::feathers::theme::{InheritableThemeTextColor, ThemedText};
use bevy::{prelude::*, ui_widgets::observe};
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    button::{
        ButtonOperatorCall, ButtonProps, ButtonSize, ButtonVariant as EditorButtonVariant,
        IconButtonProps, button, icon_button,
    },
    icons::{EditorFont, IconFont, icon_scene},
    menu_bar, status_bar,
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
    grid_ops::{GridDecreaseOp, GridIncreaseOp, GridToggleSnapOp},
    hierarchy::{HierarchyPanel, HierarchyShowAllButton, HierarchyTreeContainer},
    inspector::Inspector,
    pie::{PiePauseOp, PiePlayOp, PieStepOp, PieStopOp, PlayPhase, PlayStatus},
    snapping::SnapSettings,
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
}

impl TabKind {
    /// Human-readable label shown on the tab strip.
    pub fn label(self) -> &'static str {
        match self {
            TabKind::Scene => "Main scene",
        }
    }

    /// Colored accent stripe drawn at the left edge of the tab.
    pub fn accent(self) -> Color {
        match self {
            TabKind::Scene => tokens::DOC_TAB_SCENE_ACCENT,
        }
    }

    /// Icon glyph rendered in the tab header.
    pub fn icon(self) -> Icon {
        match self {
            TabKind::Scene => Icon::File,
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

/// Marker on the center column container: the hook for systems that want
/// to find the main viewport-plus-bottom-panels area.
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

/// The Play transport: Play with the Play menu beside it, then Pause, Step
/// and Stop. Each button runs its operator, whose availability greys it out,
/// and [`update_play_controls`] follows the play phase on Play and Pause.
fn play_pause_controls(icon_font: Handle<Font>) -> impl Bundle {
    (
        EditorEntity,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            height: px(tokens::HEADER_CONTROL_HEIGHT),
            padding: UiRect::horizontal(px(6.5)),
            column_gap: px(4.0),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_LG)),
            ..Default::default()
        },
        BackgroundColor(tokens::HEADER_CONTROL_BG),
        BorderColor::all(tokens::HEADER_CONTROL_BORDER),
        children![
            (
                PlayButton,
                TransportButton,
                EditorEntity,
                button(
                    ButtonProps::new(PlayPhase::Stopped.play_caption())
                        .with_left_icon(Icon::Play)
                        .with_variant(EditorButtonVariant::Ghost)
                        .call_operator(PiePlayOp::ID),
                ),
            ),
            pie_menu_button(icon_font.clone()),
            (
                PauseButton,
                transport_button(PiePauseOp::ID, Icon::Pause, icon_font.clone()),
            ),
            transport_button(PieStepOp::ID, Icon::StepForward, icon_font.clone()),
            transport_button(PieStopOp::ID, Icon::Square, icon_font),
        ],
    )
}

/// Caret button next to Play that opens the Play menu. Carries the
/// `PieMenuButton` marker, which `PieMenuPlugin` observes to open the menu.
fn pie_menu_button(icon_font: Handle<Font>) -> impl Bundle {
    (
        crate::pie_menu::PieMenuButton,
        TransportButton,
        EditorEntity,
        jackdaw_feathers::tooltip::Tooltip::title("Play options")
            .with_description("Run configurations, Rebuild, Restart and the game window."),
        icon_button(
            IconButtonProps::new(Icon::ChevronDown)
                .variant(EditorButtonVariant::Ghost)
                .with_size(ButtonSize::IconSM),
            &icon_font,
        ),
    )
}

/// An icon button in the Play transport that runs `op_id`. Lucide glyphs
/// live in the Private Use Area, so the icon font handle is passed
/// explicitly.
fn transport_button(op_id: &'static str, icon: Icon, icon_font: Handle<Font>) -> impl Bundle {
    (
        EditorEntity,
        TransportButton,
        ButtonOperatorCall::new(op_id),
        icon_button(
            IconButtonProps::new(icon)
                .variant(EditorButtonVariant::Ghost)
                .with_size(ButtonSize::IconSM),
            &icon_font,
        ),
    )
}

/// Marker on every button in the Play transport.
#[derive(Component)]
pub struct TransportButton;

/// Let a transport button's icon and caption take the feathers button text
/// colour, so a disabled one greys and the Primary one reads on its fill like
/// any other feathers button. Only the colour is taken: the font stays, or the
/// icon glyphs would lose the icon font. The button's colour is put back
/// afterwards so it reaches the text just marked.
pub fn theme_transport_text(
    added: Query<Entity, Added<TransportButton>>,
    children: Query<&Children>,
    colors: Query<&InheritableThemeTextColor>,
    mut commands: Commands,
) {
    for button in &added {
        let mut stack: Vec<Entity> = children
            .get(button)
            .map(|kids| kids.iter().collect())
            .unwrap_or_default();
        while let Some(entity) = stack.pop() {
            commands
                .entity(entity)
                .insert((ThemedText, PropagateOver::<TextFont>::default()));
            if let Ok(kids) = children.get(entity) {
                stack.extend(kids.iter());
            }
        }
        if let Ok(color) = colors.get(button) {
            commands.entity(button).insert(color.clone());
        }
    }
}

/// Marker on the toolbar's Play button.
#[derive(Component)]
pub struct PlayButton;

/// Marker on the toolbar's Pause button.
#[derive(Component)]
pub struct PauseButton;

/// Marker on the Play button's icon, which spins while a launch builds.
#[derive(Component)]
pub struct PlayButtonIcon;

/// Wide enough for the longest Play caption, so the buttons after it stay put
/// as it changes. The button also takes the icon buttons' height to fit the
/// pill.
const PLAY_BUTTON_MIN_WIDTH: f32 = 104.0;

/// How fast the Play icon turns while a launch builds, in turns per second.
const BUILDING_SPIN_TURNS: f32 = 1.0;

/// Show the play phase on the Play and Pause buttons: Play reads Play, Resume
/// or Building..., and fills with the accent while a game runs; Pause reads as
/// pressed while paused. Whether each can be clicked is its operator's
/// availability, which the operator buttons follow on their own.
pub fn update_play_controls(
    status: Res<PlayStatus>,
    added: Query<(), Added<PlayButton>>,
    mut play: Query<(Entity, &mut EditorButtonVariant, &mut Node), With<PlayButton>>,
    mut pause: Query<&mut EditorButtonVariant, (With<PauseButton>, Without<PlayButton>)>,
    children: Query<&Children>,
    captions: Query<(), With<jackdaw_feathers::button::ButtonContentText>>,
    icons: Query<
        (),
        (
            With<Text>,
            Without<jackdaw_feathers::button::ButtonContentText>,
        ),
    >,
    mut texts: Query<&mut Text>,
    mut commands: Commands,
) {
    if !status.is_changed() && added.is_empty() {
        return;
    }
    let phase = status.phase;
    let play_variant = if phase == PlayPhase::Playing {
        EditorButtonVariant::Primary
    } else {
        EditorButtonVariant::Ghost
    };
    let glyph = String::from(if phase == PlayPhase::Building {
        Icon::LoaderCircle.unicode()
    } else {
        Icon::Play.unicode()
    });
    for (entity, mut variant, mut node) in &mut play {
        if *variant != play_variant {
            *variant = play_variant;
        }
        if node.min_width != px(PLAY_BUTTON_MIN_WIDTH) {
            node.min_width = px(PLAY_BUTTON_MIN_WIDTH);
            node.height = ButtonSize::IconSM.height();
        }
        if let Some(caption) =
            jackdaw_feathers::button::button_caption(entity, &children, &captions)
            && let Ok(mut text) = texts.get_mut(caption)
            && text.0 != phase.play_caption()
        {
            text.0 = phase.play_caption().to_string();
        }
        let icon = children
            .get(entity)
            .into_iter()
            .flat_map(Children::iter)
            .find(|kid| icons.contains(*kid));
        if let Some(icon) = icon {
            commands.entity(icon).insert(PlayButtonIcon);
            if let Ok(mut text) = texts.get_mut(icon)
                && text.0 != glyph
            {
                text.0.clone_from(&glyph);
            }
        }
    }
    let pause_variant = if phase == PlayPhase::Paused {
        EditorButtonVariant::Active
    } else {
        EditorButtonVariant::Ghost
    };
    for mut variant in &mut pause {
        if *variant != pause_variant {
            *variant = pause_variant;
        }
    }
}

/// Turn the Play icon while a launch builds, and set it straight otherwise.
pub fn spin_play_icon(
    time: Res<Time<Real>>,
    status: Res<PlayStatus>,
    mut icons: Query<(Entity, Option<&mut UiTransform>), With<PlayButtonIcon>>,
    mut commands: Commands,
) {
    let building = status.phase == PlayPhase::Building;
    if !building && !status.is_changed() {
        return;
    }
    let rotation = if building {
        Rot2::radians(time.elapsed_secs() * BUILDING_SPIN_TURNS * std::f32::consts::TAU)
    } else {
        Rot2::IDENTITY
    };
    for (entity, transform) in &mut icons {
        match transform {
            Some(mut transform) => transform.rotation = rotation,
            None => {
                commands.entity(entity).insert(UiTransform {
                    rotation,
                    ..UiTransform::IDENTITY
                });
            }
        }
    }
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
        // The main editor toolbar and the terrain
        // toolbar are bsn! Scenes, which can't nest inside this Bundle
        // `children!` tree. They're spawned separately and slotted in above
        // the viewport by `build_viewport_panel`.
        children![scene_view()],
    )
}

/// The viewport's main editor toolbar as a `bsn!` Scene. Holds the
/// Select/Translate/Rotate/Scale buttons, gizmo-space toggle, draw-brush,
/// measure, brush edit modes, physics, and the grid stepper and snap.
///
/// Spawned standalone via `spawn_scene`; see
/// [`crate::viewport::build_viewport_panel`]. A Scene can't nest inside the
/// `viewport_with_toolbar` Bundle `children!` tree, and the spawn site
/// attaches the [`Toolbar`] and [`EditorEntity`] markers. Active-tool
/// highlighting is driven by [`update_toolbar_button_variants`] flipping
/// each button's [`ButtonVariant`] by operator id, so this never mutates
/// `BackgroundColor` directly.
///
/// Sizing: 30px tall, 1px border, top corners rounded against the panel
/// below.
pub(crate) fn toolbar() -> impl Scene {
    bsn! {
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
            border_radius: BorderRadius::top(px(tokens::TOOLBAR_RADIUS)),
            flex_shrink: 0.0,
        }
        BackgroundColor(tokens::PANEL_HEADER_BG)
        BorderColor::all(tokens::TOOLBAR_BORDER)
        Children [
            @toolbar_tools()
            --
            // Spacer pushes the grid / snap widget to the right edge.
            @toolbar_spacer()
            --
            // Grid-size stepper: current size between decrease / increase.
            @toolbar_op_button(GridDecreaseOp::ID, Icon::Minus)
            --
            @grid_size_label()
            --
            @toolbar_op_button(GridIncreaseOp::ID, Icon::Plus)
            --
            @toolbar_separator()
            --
            // Grid-snap toggle; highlights while snapping is on.
            @toolbar_op_button(GridToggleSnapOp::ID, Icon::Magnet)
        ]
    }
}

/// The tool and mode buttons at the toolbar's left. In a viewport too narrow
/// for the whole toolbar they are cut off at the right, rather than the grid,
/// settings and mode controls past the spacer, and the ones cut off are
/// listed in [`crate::toolbar_overflow`]'s menu.
fn toolbar_tools() -> impl Scene {
    bsn! {
        crate::toolbar_overflow::ToolbarTools
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(tokens::TOOLBAR_GAP),
            flex_shrink: 1.0,
            min_width: px(0.0),
            overflow: Overflow::clip_x(),
        }
        Children [
            @toolbar_op_button(ToolSelectOp::ID, Icon::MousePointer)
            --
            @toolbar_op_button(ToolTranslateOp::ID, Icon::Move3d)
            --
            @toolbar_op_button(ToolRotateOp::ID, Icon::Rotate3d)
            --
            @toolbar_op_button(ToolScaleOp::ID, Icon::Scale3d)
            --
            @toolbar_separator()
            --
            // Gizmo space toggle. Active highlight = `Local`; default
            // = `World`. Tooltip is the discoverability path.
            @toolbar_op_button(GizmoSpaceToggleOp::ID, Icon::Globe)
            --
            @toolbar_separator()
            --
            @toolbar_op_button(ActivateDrawBrushModalOp::ID, Icon::Box)
            --
            @toolbar_op_button(crate::modelling_mode::MEASURE_DISTANCE_OP, Icon::RulerDimensionLine)
            --
            @toolbar_op_button(EditModeVertexOp::ID, Icon::CircleDot)
            --
            @toolbar_op_button(EditModeEdgeOp::ID, Icon::GitCommitHorizontal)
            --
            @toolbar_op_button(EditModeFaceOp::ID, Icon::Hexagon)
            --
            @toolbar_op_button(EditModeClipOp::ID, Icon::ScissorsLineDashed)
        ]
    }
}

/// Font size for the toolbar's icon glyphs.
const TOOLBAR_ICON_PX: f32 = 16.0;

/// Thin vertical rule separating groups of toolbar buttons.
pub(crate) fn toolbar_separator() -> impl Scene {
    bsn! {
        Node {
            width: px(1),
            height: px(16),
            align_self: AlignSelf::Center,
            margin: UiRect::horizontal(px(2)),
        }
        BackgroundColor(tokens::TOOLBAR_BORDER)
    }
}

/// Flexible gap that pushes the grid / snap controls to the toolbar's
/// right edge.
fn toolbar_spacer() -> impl Scene {
    bsn! {
        crate::toolbar_overflow::ToolbarSpacer
        Node {
            flex_grow: 1.0,
        }
    }
}

/// Marker for the live grid-size readout in the viewport toolbar.
#[derive(Component, Default, Clone)]
pub struct GridSizeLabel;

/// A text readout of the current grid size, updated by
/// [`update_grid_size_label`]. The font is filled in by that system from
/// the editor font resource (the toolbar scene has none to hand).
fn grid_size_label() -> impl Scene {
    bsn! {
        GridSizeLabel
        Text("1")
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
        }
        TextColor(tokens::TEXT_SECONDARY)
        Node {
            align_self: AlignSelf::Center,
            min_width: px(34.0),
        }
    }
}

/// Format a grid size for the toolbar readout, trimming a trailing
/// `.0` so whole sizes show as `1`, `2` rather than `1.0`.
fn format_grid_size(size: f32) -> String {
    if size.fract() == 0.0 {
        format!("{size:.0}")
    } else {
        // Default `f32` formatting prints the shortest string that reads
        // back as the same value, so it renders both the power-of-two
        // sizes (0.25, 0.0625) and an explicit metric increment (1.5,
        // 2.5) without a trailing tail of digits.
        format!("{size}")
    }
}

/// Keep the toolbar grid readout in sync with the snap settings and give
/// it the editor font (the toolbar bundle is built without one).
pub fn update_grid_size_label(
    snap: Res<SnapSettings>,
    editor_font: Res<jackdaw_feathers::icons::EditorFont>,
    mut labels: Query<(&mut Text, &mut TextFont), With<GridSizeLabel>>,
) {
    let text = format_grid_size(snap.grid_size());
    for (mut label, mut font) in &mut labels {
        if label.0 != text {
            label.0 = text.clone();
        }
        if font.font != editor_font.0.clone().into() {
            font.font = editor_font.0.clone().into();
        }
    }
}

/// An icon-only toolbar button bound to operator `op_id`. It is a
/// [`FeathersToolButton`] whose caption is a single Lucide glyph rendered
/// with the icon font via [`icon_scene`]. The [`ButtonOperatorCall`] hooks
/// it into the operator-button glue in `core_extension`, which dispatches
/// on `Activate`, auto-disables via `InteractionDisabled` when the
/// operator is unavailable, and attaches the operator tooltip on hover
/// through the `On<Add<ButtonOperatorCall>>` observer.
///
/// Starts in the `Plain` variant so idle buttons read flat against the
/// toolbar panel; [`update_toolbar_button_variants`] flips the active
/// tool/mode/modal button to `Primary`.
#[doc(hidden)]
pub fn toolbar_op_button(op_id: &'static str, icon: Icon) -> impl Scene {
    let glyph = String::from(icon.unicode());
    bsn! {
        @FeathersToolButton {
            @caption: bsn! { @icon_scene(glyph, TOOLBAR_ICON_PX) bevy::feathers::theme::ThemedText },
            @variant: {ButtonVariant::Plain}
        }
        bevy::feathers::font_styles::InheritableFont {
            font: bevy::text::FontSourceTemplate::Handle(jackdaw_feathers::icons::font_paths::LUCIDE),
            font_size: bevy::text::FontSize::Px(TOOLBAR_ICON_PX),
        }
        ButtonOperatorCall::new(op_id)
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
                    padding: UiRect::vertical(px(tokens::SPACING_XS)),
                    border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
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
                        jackdaw_feathers::tooltip::Tooltip::title("Show All Entities")
                            .with_description(
                                "Toggle visibility of editor-internal entities and \
                                 hidden objects in the hierarchy.",
                            ),
                        icon_button(
                            IconButtonProps::new(Icon::Eye)
                                .variant(EditorButtonVariant::Ghost)
                                .color(tokens::TEXT_SECONDARY),
                            &icon_font,
                        ),
                    ),
                ],
            ),
            (
                crate::add_entity_picker::AddEntityButton,
                button(ButtonProps::new("").align_left()),
                observe(|mut click: On<PointerClick>, mut commands: Commands| {
                    click.propagate(false);
                    commands.queue(|world: &mut World| {
                        world.run_system_cached(crate::add_entity_picker::open_add_entity_picker)
                    });
                },),
                children![
                    (
                        Text::new(String::from(Icon::PackagePlus.unicode())),
                        TextFont {
                            font: add_entity_icon_font.into(),
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
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                TextLayout::justify(Justify::Center),
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
            // Width reserved permanently so the playing edge can be toggled by
            // color alone without shifting the viewport bounds.
            border: UiRect::all(px(2.0)),
            ..Default::default()
        },
        BorderColor::all(Color::NONE),
    )
}

/// Operator ids the main viewport toolbar owns. The terrain contextual
/// toolbar spawns the same `ButtonOperatorCall` and `ButtonVariant`
/// component pair and drives its own highlighter, so
/// [`update_toolbar_button_variants`] flips the variant only on these ids. The
/// grid stepper actions `GridDecreaseOp` and `GridIncreaseOp` are absent: they
/// never highlight, so they stay at their spawn variant.
fn is_main_toolbar_op(id: &str) -> bool {
    id == ToolSelectOp::ID
        || id == ToolTranslateOp::ID
        || id == ToolRotateOp::ID
        || id == ToolScaleOp::ID
        || id == GizmoSpaceToggleOp::ID
        || id == ActivateDrawBrushModalOp::ID
        || id == crate::modelling_mode::MEASURE_DISTANCE_OP
        || id == EditModeVertexOp::ID
        || id == EditModeEdgeOp::ID
        || id == EditModeFaceOp::ID
        || id == EditModeClipOp::ID
        || id == EditModeKnifeOp::ID
        || id == GridToggleSnapOp::ID
}

/// Flip each main-toolbar button's [`ButtonVariant`] between `Plain`
/// (idle, transparent) and `Primary` (active) based on the matching editor
/// state, identified by operator id. The feathers `update_button_styles`
/// system reads the variant to compute the background, so this is the only
/// place toolbar active-state lives; `BackgroundColor` is never mutated
/// directly.
///
/// Buttons whose id isn't a main-toolbar op are skipped, so the terrain
/// contextual row and this one do not write the same `ButtonVariant` even
/// though they share the component pair.
///
/// This is an [`On<RefreshOperatorButtons>`] observer. Every editor-state
/// change it reads from -- active tool, edit mode, gizmo space, snap, and
/// the active modal -- is mutated by an operator, which announces through
/// dispatch. Seeding on a freshly-spawned toolbar comes from the same event
/// fired when each `ButtonOperatorCall` is added. The loop is O(toolbar
/// buttons) and only writes on an actual change.
pub fn update_toolbar_button_variants(
    _: On<RefreshOperatorButtons>,
    edit_mode: Res<EditMode>,
    active_tool: Res<ActiveTool>,
    gizmo_space: Res<GizmoSpace>,
    snap: Res<SnapSettings>,
    active_modal: ActiveModalQuery,
    mut buttons: Query<(
        &ButtonOperatorCall,
        &mut ButtonVariant,
        Has<crate::extension_slots::ExtensionToolbarItem>,
    )>,
) {
    let modal_running = active_modal.is_modal_running();
    for (call, mut variant, from_extension) in &mut buttons {
        let id = call.id.as_ref();
        if !from_extension && !is_main_toolbar_op(id) {
            continue;
        }
        // While any modal is running only the modal's own button
        // highlights. Gizmo and mode buttons go quiet so the user sees a
        // single active tool at a time. Draw Brush and Measure highlight
        // here via their modal id; they have no non-modal arm below.
        let active = if modal_running {
            active_modal.is_operator(id)
        } else if id == ToolTranslateOp::ID {
            *active_tool == ActiveTool::Translate
        } else if id == ToolRotateOp::ID {
            *active_tool == ActiveTool::Rotate
        } else if id == ToolScaleOp::ID {
            *active_tool == ActiveTool::Scale
        } else if id == GizmoSpaceToggleOp::ID {
            *gizmo_space == GizmoSpace::Local
        } else if id == ToolSelectOp::ID {
            *active_tool == ActiveTool::Select
        } else if id == EditModeVertexOp::ID {
            *edit_mode == EditMode::BrushEdit(BrushEditMode::Vertex)
        } else if id == EditModeEdgeOp::ID {
            *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge)
        } else if id == EditModeFaceOp::ID {
            *edit_mode == EditMode::BrushEdit(BrushEditMode::Face)
        } else if id == EditModeClipOp::ID {
            *edit_mode == EditMode::BrushEdit(BrushEditMode::Clip)
        } else if id == EditModeKnifeOp::ID {
            *edit_mode == EditMode::BrushEdit(BrushEditMode::Knife)
        } else if id == GridToggleSnapOp::ID {
            snap.translate_snap
        } else {
            false
        };
        let target = if active {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Plain
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
/// tabs go transparent.
pub fn update_tab_strip_highlights(
    active: Res<ActiveDocument>,
    mut tabs: Query<(
        &DocumentTabButton,
        &mut BackgroundColor,
        &mut BorderColor,
        &Children,
    )>,
    mut texts: Query<&mut TextColor>,
) {
    if !active.is_changed() {
        return;
    }
    for (tab, mut tab_bg, mut tab_border, children) in &mut tabs {
        let is_active = tab.0 == active.kind;

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

        let label_color = if is_active {
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
pub fn editor_status_bar() -> impl Bundle {
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
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                bevy::feathers::theme::ThemedText,
            ),
            // Gives up room before the right side does, cutting the path off.
            (
                crate::status_bar::StatusBarCenterSlot,
                Node {
                    flex_shrink: 1.0,
                    min_width: Val::Px(0.0),
                    margin: UiRect::horizontal(Val::Px(tokens::SPACING_MD)),
                    overflow: Overflow::clip(),
                    ..Default::default()
                },
                children![(
                    status_bar::StatusBarCenter,
                    Text::default(),
                    TextLayout {
                        linebreak: bevy::text::LineBreak::NoWrap,
                        ..Default::default()
                    },
                    TextFont {
                        font_size: tokens::TEXT_SIZE_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                )],
            ),
            (
                crate::status_bar::StatusBarInspected,
                Text::default(),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
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
                    // Fixed-width clipped box: the build status text changes
                    // length as crates compile, and a bare text node would
                    // reflow the rest of the footer on every frame. Which end
                    // is clipped is decided per message by
                    // [`status_bar::align_status_right`], which is why the
                    // box is marked.
                    (
                        crate::status_bar::StatusBarRightBox,
                        Node {
                            width: Val::Px(210.0),
                            overflow: Overflow::clip(),
                            justify_content: JustifyContent::FlexEnd,
                            align_items: AlignItems::Center,
                            ..Default::default()
                        },
                        children![(
                            status_bar::StatusBarRight,
                            Text::default(),
                            TextLayout {
                                linebreak: bevy::text::LineBreak::NoWrap,
                                ..Default::default()
                            },
                            TextFont {
                                font_size: tokens::TEXT_SIZE_SM,
                                ..Default::default()
                            },
                            TextColor(tokens::TEXT_SECONDARY),
                        )],
                    ),
                    // Connection indicator
                    (crate::remote_mode::ConnectionIndicatorSlot, Node::default())
                ],
            )
        ],
    )
}

pub fn inspector_components_content() -> impl Bundle {
    // Outer horizontal row: [strip | content column]
    (
        Node {
            flex_direction: FlexDirection::Row,
            flex_grow: 1.0,
            min_height: px(0.0),
            ..Default::default()
        },
        children![
            // Strip mount: the category tab rail is spawned here by the
            // On<Add<InspectorCategoryStripMount>> observer in InspectorPlugin.
            (crate::inspector::category_strip::InspectorCategoryStripMount,),
            // Content column: add-header + search header + scrollable card list.
            (
                Node {
                    flex_direction: FlexDirection::Column,
                    flex_grow: 1.0,
                    min_height: px(0.0),
                    // The dock sets the panel's width. Without this floor the
                    // column takes its own min-content width, so one
                    // unbreakable component title widens the whole column and
                    // the surplus is clipped at the panel's edge.
                    min_width: px(0.0),
                    ..Default::default()
                },
                children![
                    // Add-header mount: per-category add UI populated by
                    // `rebuild_add_header` whenever `ActiveInspectorCategory` changes.
                    (crate::inspector::add_header::InspectorAddHeaderMount,),
                    (
                        Node {
                            flex_direction: FlexDirection::Column,
                            width: percent(100),
                            padding: UiRect::all(px(tokens::SPACING_SM)),
                            row_gap: px(tokens::SPACING_XS),
                            flex_shrink: 0.0,
                            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
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
                            (crate::tags::InspectorTagsMount,),
                            crate::inspector::unavailable::unavailable_notice(),
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
            ),
        ],
    )
}

/// Draw the viewport's edge in the accent while a game runs. Runs every frame so a
/// viewport node respawned by a dock rearrange picks it back up; the writes are
/// guarded so unchanged frames do not dirty the UI.
pub fn update_playing_edge(
    play_state: Res<State<PlayState>>,
    mut viewport_border: Query<&mut BorderColor, With<SceneViewport>>,
) {
    let color = if *play_state.get() == PlayState::Stopped {
        Color::NONE
    } else {
        crate::default_style::PLAYING_ACCENT
    };
    let target = BorderColor::all(color);
    for mut border in &mut viewport_border {
        if *border != target {
            *border = target;
        }
    }
}

#[cfg(test)]
mod playing_edge_tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    #[test]
    fn the_viewport_edge_shows_while_a_game_runs_or_is_paused() {
        let mut world = World::new();
        let viewport = world
            .spawn((SceneViewport, BorderColor::all(Color::NONE)))
            .id();
        for (state, edge) in [
            (PlayState::Playing, crate::default_style::PLAYING_ACCENT),
            (PlayState::Paused, crate::default_style::PLAYING_ACCENT),
            (PlayState::Stopped, Color::NONE),
        ] {
            world.insert_resource(State::new(state));
            world
                .run_system_once(update_playing_edge)
                .expect("system runs");
            assert_eq!(world.get::<BorderColor>(viewport).unwrap().top, edge);
        }
    }
}

#[cfg(test)]
mod grid_readout_tests {
    use super::*;

    #[test]
    fn whole_sizes_lose_their_trailing_zero() {
        assert_eq!(format_grid_size(1.0), "1");
        assert_eq!(format_grid_size(8.0), "8");
    }

    #[test]
    fn power_of_two_fractions_read_exactly() {
        assert_eq!(format_grid_size(0.25), "0.25");
        assert_eq!(format_grid_size(0.0625), "0.0625");
    }

    #[test]
    fn an_explicit_metric_increment_reads_as_itself() {
        assert_eq!(format_grid_size(1.5), "1.5");
        assert_eq!(format_grid_size(2.5), "2.5");
        assert_eq!(format_grid_size(0.75), "0.75");
    }
}
