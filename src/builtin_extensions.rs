//! Built-in Jackdaw extensions. Each feature area of the editor owns
//! its dock windows through a `JackdawExtension`, so Jackdaw uses the
//! same API third-party authors do. Disable one in File > Extensions
//! to remove its windows from the layout.

use bevy::{
    feathers::{
        controls::ButtonVariant,
        cursor::EntityCursor,
        focus::FocusIndicator,
        theme::{
            InheritableThemeTextColor, ThemeBackgroundColor, ThemeBorderColor, ThemeTextColor,
            ThemeToken, ThemedText,
        },
        tokens as feathers_tokens,
    },
    input_focus::tab_navigation::TabIndex,
    prelude::*,
    window::SystemCursorIcon,
};
use jackdaw_api::{
    DefaultArea, ExtensionPoint, HierarchyWindow, InspectorWindow, WidgetDefinition,
    prelude::{ExtensionContext, ExtensionKind, JackdawExtension, WindowDescriptor},
};
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::tokens;

/// Reflect type paths of jackdaw's authorable world components paired with
/// their outliner icon, in priority order. Type paths use each type's
/// defining module (what `TypePath` reports), not a re-export path. Tested
/// against the real `TypePath` so a typo fails loudly.
pub(crate) const WORLD_ENTITY_ICONS: &[(&str, Icon)] = &[
    ("jackdaw_scene_types::types::Brush", Icon::Cuboid),
    ("jackdaw_scene_types::types::Terrain", Icon::Mountain),
    // Ahead of the `Mesh3d` rule: an instance carries no mesh of its own.
    ("jackdaw_scene_types::types::GltfSource", Icon::Boxes),
    ("jackdaw::entity_ops::SceneFogVolume", Icon::CloudFog),
    ("jackdaw::entity_ops::SceneReflectionProbe", Icon::Sparkles),
    ("jackdaw::entity_ops::SceneAnimationPlayer", Icon::Play),
    ("jackdaw::entity_ops::SceneAudioSource", Icon::Volume2),
    (
        "jackdaw::reference_image::ReferenceImage",
        Icon::PictureInPicture,
    ),
];

/// Icon for the camera-rig component, gated to match the `camera_rig`
/// cargo feature that brings the type in.
#[cfg(feature = "camera_rig")]
pub(crate) const CAMERA_RIG_ICON: (&str, Icon) = ("jackdaw_camera_rig::CameraRig", Icon::Orbit);

/// Scene Tree, Import, and Project Files in the left dock.
#[derive(Default)]
pub struct CoreWindowsExtension;

impl JackdawExtension for CoreWindowsExtension {
    fn id(&self) -> String {
        "jackdaw.core_windows".to_string()
    }

    fn label(&self) -> String {
        "Core Windows".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        for (type_path, icon) in scene_kind_icons() {
            ctx.register_entity_icon(type_path, icon);
        }
        for (type_path, icon) in WORLD_ENTITY_ICONS {
            ctx.register_entity_icon(*type_path, *icon);
        }
        #[cfg(feature = "camera_rig")]
        ctx.register_entity_icon(CAMERA_RIG_ICON.0, CAMERA_RIG_ICON.1);
        for (type_path, icon) in world_kind_icons() {
            ctx.register_entity_icon(type_path, icon);
        }
        // Every `Node` is a container of some sort, so this must not answer
        // before an extension has named its own UI kind.
        ctx.register_entity_icon_last_resort_predicate(container_icon);

        ctx.register_window(
            WindowDescriptor::new(HierarchyWindow::ID)
                .with_name("Outliner")
                .with_default_area(DefaultArea::Left)
                .with_priority(0)
                .with_build(|window| {
                    let icon_font = window
                        .world()
                        .get_resource::<jackdaw_feathers::icons::IconFont>()
                        .map(|f| f.0.clone())
                        .unwrap_or_default();
                    window.spawn(crate::layout::hierarchy_content(icon_font));
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.import")
                .with_name("Import")
                .with_default_area(DefaultArea::Left)
                .with_priority(1)
                .with_build(|window| {
                    window.spawn((
                        Node {
                            flex_grow: 1.0,
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        children![(
                            Text::new("Import"),
                            TextFont {
                                font_size: tokens::TEXT_SIZE_SM,
                                ..default()
                            },
                            TextColor(tokens::TEXT_DISABLED),
                        )],
                    ));
                }),
        );
        ctx.register_window(
            WindowDescriptor::new("jackdaw.project_files")
                .with_name("Project Files")
                .with_default_area(DefaultArea::Left)
                .with_priority(10)
                .with_build(|window| {
                    window.spawn(crate::layout::project_files_panel_content());
                    window
                        .world_mut()
                        .resource_mut::<crate::project_files::ProjectFilesState>()
                        .needs_refresh = true;
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.remote.entities")
                .with_name("Remote Entities")
                .with_default_area(DefaultArea::Left)
                .with_priority(20)
                .with_build(|window| {
                    window.spawn(crate::remote::entity_browser::remote_debug_workspace_content());
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.debug.diagnostics")
                .with_name("Remote Diagnostics")
                .with_default_area(DefaultArea::Left)
                .with_priority(21)
                .with_build(|window| {
                    crate::remote::debug::diagnostics::build_diagnostics_window(window);
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.debug.queries")
                .with_name("Remote Queries")
                .with_default_area(DefaultArea::Center)
                .with_priority(22)
                .with_build(|window| {
                    window.spawn(crate::remote::debug::queries::queries_panel_content());
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.debug.archetypes")
                .with_name("Remote Archetypes")
                .with_default_area(DefaultArea::Center)
                .with_priority(23)
                .with_build(|window| {
                    window.spawn(crate::remote::debug::archetypes::archetypes_panel_content());
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.debug.schedules")
                .with_name("Remote Schedules")
                .with_default_area(DefaultArea::Center)
                .with_priority(24)
                .with_build(|window| {
                    window.spawn(crate::remote::debug::schedules::schedules_panel_content());
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.debug.graph")
                .with_name("Remote System Graph")
                .with_default_area(DefaultArea::Center)
                .with_priority(25)
                .with_build(|window| {
                    window.spawn(crate::remote::debug::depgraph::depgraph_panel_content());
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.debug.relationships")
                .with_name("Remote Relationships")
                .with_default_area(DefaultArea::Center)
                .with_priority(26)
                .with_build(|window| {
                    window
                        .spawn(crate::remote::debug::relationships::relationships_panel_content());
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.remote.inspector")
                .with_name("Remote Inspector")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(20)
                .with_build(|window| {
                    window.spawn(crate::remote::remote_inspector::remote_inspector());
                }),
        );
    }
}

/// The viewport, registered as a regular dock panel so multiple
/// instances (quad-view, stacked viewports for animation work, etc.)
/// can coexist in the dock tree. One panel shows either the 3D world or the
/// 2D canvas, so the canvas operators are registered here too.
#[derive(Default)]
pub struct ViewportExtension;

impl JackdawExtension for ViewportExtension {
    fn id(&self) -> String {
        "jackdaw.viewport_panel".to_string()
    }

    fn label(&self) -> String {
        "Viewport".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        crate::viewport_2d::add_to_extension(ctx);
        ctx.register_window(
            WindowDescriptor::new(crate::viewport::VIEWPORT_WINDOW_ID)
                .with_name("Viewport")
                .with_default_area(DefaultArea::Center)
                .with_priority(0)
                .with_build(|window| {
                    let parent = window.target_entity();
                    crate::viewport::build_viewport_panel(window.world_mut(), parent);
                }),
        );
    }
}

/// Assets window in the bottom dock.
#[derive(Default)]
pub struct AssetBrowserExtension;

impl JackdawExtension for AssetBrowserExtension {
    fn id(&self) -> String {
        "jackdaw.asset_browser".to_string()
    }

    fn label(&self) -> String {
        "Asset Browser".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_window(
            WindowDescriptor::new("jackdaw.assets")
                .with_name("Assets")
                .with_icon(Icon::FolderOpen.unicode())
                .with_default_area(DefaultArea::BottomDock)
                .with_priority(0)
                .with_build(|window| {
                    let icon_font = window
                        .world()
                        .get_resource::<jackdaw_feathers::icons::IconFont>()
                        .map(|f| f.0.clone())
                        .unwrap_or_default();
                    window.spawn(crate::asset_browser::asset_browser_panel(icon_font));
                    window
                        .world_mut()
                        .resource_mut::<crate::asset_browser::AssetBrowserState>()
                        .needs_refresh = true;
                }),
        );
    }
}

/// Game monitor in the bottom dock: shows the focused instance's streamed
/// frame with a Play/Select mode bar.
#[derive(Default)]
pub struct GamePanelExtension;

impl JackdawExtension for GamePanelExtension {
    fn id(&self) -> String {
        "jackdaw.game_panel".to_string()
    }

    fn label(&self) -> String {
        "Game Panel".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_window(
            WindowDescriptor::new(crate::game_panel::GAME_WINDOW_ID)
                .with_name("Game")
                .with_icon(Icon::Play.unicode())
                .with_default_area(DefaultArea::BottomDock)
                .with_priority(2)
                .with_build(|window| {
                    window.spawn(crate::game_panel::game_panel_content());
                }),
        );
    }
}

/// Clip library and keyframe timeline in the bottom dock.
///
/// The window keeps its `jackdaw.timeline` id so a saved layout still finds
/// it; only what it is called changed.
#[derive(Default)]
pub struct TimelineExtension;

impl JackdawExtension for TimelineExtension {
    fn id(&self) -> String {
        "jackdaw.timeline".to_string()
    }

    fn label(&self) -> String {
        "Animation".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_window(
            WindowDescriptor::new("jackdaw.timeline")
                .with_name("Animation")
                .with_icon(Icon::Film.unicode())
                .with_default_area(DefaultArea::BottomDock)
                .with_priority(1)
                .with_build(|window| {
                    window.spawn(crate::animation::animation_panel_content());
                }),
        );
    }
}

/// Terminal placeholder in the bottom dock.
#[derive(Default)]
pub struct TerminalExtension;

impl JackdawExtension for TerminalExtension {
    fn id(&self) -> String {
        "jackdaw.terminal".to_string()
    }

    fn label(&self) -> String {
        "Terminal".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_window(
            WindowDescriptor::new("jackdaw.terminal")
                .with_name("Terminal")
                .with_icon(Icon::Terminal.unicode())
                .with_default_area(DefaultArea::BottomDock)
                .with_priority(2)
                .with_build(|window| {
                    window.spawn((
                        Node {
                            flex_grow: 1.0,
                            width: Val::Percent(100.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        children![(
                            Text::new("Terminal window (not implemented yet)"),
                            TextFont {
                                font_size: tokens::TEXT_SIZE_SM,
                                ..default()
                            },
                            TextColor(tokens::TEXT_DISABLED),
                        )],
                    ));
                }),
        );
    }
}

/// Right-sidebar stack: Components, Terrain, Materials, Resources, Systems.
#[derive(Default)]
pub struct InspectorExtension;

impl JackdawExtension for InspectorExtension {
    fn id(&self) -> String {
        "jackdaw.inspector".to_string()
    }

    fn label(&self) -> String {
        "Inspector".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_window(
            WindowDescriptor::new(InspectorWindow::ID)
                .with_name("Components")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(0)
                .with_build(|window| {
                    let icon_font = window
                        .world()
                        .get_resource::<jackdaw_feathers::icons::IconFont>()
                        .map(|f| f.0.clone())
                        .unwrap_or_default();
                    window.spawn(crate::layout::inspector_components_content(icon_font));
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.inspector.terrain")
                .with_name("Terrain")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(1)
                .with_build(|window| {
                    window.spawn(crate::terrain::panel::terrain_panel_content());
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.inspector.materials")
                .with_name("Materials")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(3)
                .with_build(|window| {
                    let icon_font = window
                        .world()
                        .get_resource::<jackdaw_feathers::icons::IconFont>()
                        .map(|f| f.0.clone())
                        .unwrap_or_default();
                    window.spawn(crate::material_browser::material_browser_panel(icon_font));
                    window
                        .world_mut()
                        .resource_mut::<crate::material_browser::MaterialBrowserState>()
                        .needs_rescan = true;
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.inspector.resources")
                .with_name("Resources")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(4)
                .with_build(|window| {
                    window.spawn((
                        Node {
                            flex_grow: 1.0,
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        children![(
                            Text::new("Resources"),
                            TextFont {
                                font_size: tokens::TEXT_SIZE_SM,
                                ..default()
                            },
                            TextColor(tokens::TEXT_DISABLED),
                        )],
                    ));
                }),
        );

        ctx.register_window(
            WindowDescriptor::new(crate::preview_context::PREVIEW_CONTEXT_WINDOW_ID)
                .with_name("Preview")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(2)
                .with_build(crate::preview_context::build_preview_context_panel),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.inspector.systems")
                .with_name("Systems")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(5)
                .with_build(|window| {
                    window.spawn((
                        Node {
                            flex_grow: 1.0,
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        children![(
                            Text::new("Systems"),
                            TextFont {
                                font_size: tokens::TEXT_SIZE_SM,
                                ..default()
                            },
                            TextColor(tokens::TEXT_DISABLED),
                        )],
                    ));
                }),
        );
    }
}

/// The UI widget vocabulary the Add menu's UI Widgets section lists.
///
/// Definitions assemble `bevy_ui` and `bevy_ui_widgets` rather than
/// `bevy_feathers` controls, which are `SceneComponent`s and do not survive a
/// document round trip; feathers' theme-token components are plain `Reflect`
/// components and are spawned directly.
#[derive(Default)]
pub struct UiPaletteExtension;

impl JackdawExtension for UiPaletteExtension {
    fn id(&self) -> String {
        "jackdaw.ui_palette".to_string()
    }

    fn label(&self) -> String {
        "UI Widgets".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        for definition in builtin_widget_definitions() {
            ctx.register_widget(definition);
        }
        for (type_path, widget_id) in widget_kind_sources() {
            if let Some(icon) = ctx.widget_definition(widget_id).and_then(|it| it.icon) {
                ctx.register_entity_icon(type_path, icon);
            }
        }
    }
}

/// Spawn one authored widget under the resolved parent.
///
/// An entity `Name` carries no space even where the menu label does: an
/// operator clause's `name=` value has no quoting.
fn spawn_widget(world: &mut World, parent: Option<Entity>, bundle: impl Bundle) -> Entity {
    let mut entity = world.spawn(bundle);
    if let Some(parent) = parent {
        entity.insert(ChildOf(parent));
    }
    entity.id()
}

/// Scene-shaped kinds, registered first so they win over the components an
/// entity is also made of.
fn scene_kind_icons() -> Vec<(String, Icon)> {
    use bevy::reflect::TypePath;
    vec![
        (
            jackdaw_scene_types::UiSceneRoot::type_path().to_string(),
            Icon::LayoutTemplate,
        ),
        (
            jackdaw_scene_types::Scene2dRoot::type_path().to_string(),
            Icon::Frame,
        ),
        (
            jackdaw_scene_types::SceneRootTag::type_path().to_string(),
            Icon::Clapperboard,
        ),
        (
            jackdaw_prefab::components::IsA::type_path().to_string(),
            Icon::Component,
        ),
    ]
}

/// The 3D kinds, after jackdaw's own authorable components: a brush and
/// a terrain both carry `Mesh3d`, and they are the more particular thing.
fn world_kind_icons() -> Vec<(String, Icon)> {
    use bevy::reflect::TypePath;
    vec![
        (Camera::type_path().to_string(), Icon::Video),
        (DirectionalLight::type_path().to_string(), Icon::Sun),
        (PointLight::type_path().to_string(), Icon::Lightbulb),
        (SpotLight::type_path().to_string(), Icon::Flashlight),
        (Mesh3d::type_path().to_string(), Icon::Box),
    ]
}

/// The component that identifies each built-in UI widget in the outliner,
/// paired with the id of the widget whose glyph it takes. An id with no
/// definition, or a definition with no icon, contributes nothing.
fn widget_kind_sources() -> [(String, &'static str); 16] {
    use bevy::reflect::TypePath;
    use bevy::ui_widgets::{Button, Checkbox, RadioButton, ScrollArea, Slider};

    [
        (Button::type_path().to_string(), "ui.button"),
        (
            jackdaw_widgets_runtime::Spacer::type_path().to_string(),
            "ui.spacer",
        ),
        (
            jackdaw_widgets_runtime::Separator::type_path().to_string(),
            "ui.separator",
        ),
        (
            jackdaw_widgets_runtime::Progress::type_path().to_string(),
            "ui.progress",
        ),
        (
            jackdaw_widgets_runtime::Dropdown::type_path().to_string(),
            "ui.dropdown",
        ),
        (
            jackdaw_widgets_runtime::RadioOptions::type_path().to_string(),
            "ui.radio_group",
        ),
        (
            jackdaw_widgets_runtime::TabStrip::type_path().to_string(),
            "ui.tabs",
        ),
        // Before the image rule: a nine-patch is an `ImageNode` too.
        (
            jackdaw_widgets_runtime::NineSlice::type_path().to_string(),
            "ui.nine_patch",
        ),
        // Before the checkbox: a toggle switch is a `Checkbox` too.
        (
            jackdaw_widgets_runtime::ToggleSwitch::type_path().to_string(),
            "ui.toggle",
        ),
        (Checkbox::type_path().to_string(), "ui.checkbox"),
        (RadioButton::type_path().to_string(), "ui.radio"),
        (Slider::type_path().to_string(), "ui.slider"),
        (
            jackdaw_widgets_runtime::TextValue::type_path().to_string(),
            "ui.text_input",
        ),
        (ScrollArea::type_path().to_string(), "ui.scroll_area"),
        (Text::type_path().to_string(), "ui.label"),
        (ImageNode::type_path().to_string(), "ui.image"),
    ]
}

/// A `Node` that is nothing more particular is a container; its own values say
/// which kind.
fn container_icon(entity: bevy::ecs::world::EntityRef) -> Option<Icon> {
    let node = entity.get::<Node>()?;
    if node.display == Display::Grid {
        return Some(Icon::Grid3x3);
    }
    match node.flex_direction {
        FlexDirection::Row | FlexDirection::RowReverse => Some(Icon::Columns3),
        // A surface behind a column is what separates a panel from a plain one.
        FlexDirection::Column | FlexDirection::ColumnReverse => {
            if entity.contains::<ThemeBackgroundColor>() {
                Some(Icon::PanelTop)
            } else {
                Some(Icon::Rows3)
            }
        }
    }
}

/// A container preset: a `Node` plus the theme token that makes it a surface.
/// `None` spawns a transparent background instead, which the theme leaves
/// alone.
fn container_definition(
    id: &'static str,
    name: &'static str,
    icon: Icon,
    node: fn() -> Node,
    surface: Option<ThemeToken>,
) -> WidgetDefinition {
    WidgetDefinition::new(id, name, "Layout", move |world, context| {
        let entity = spawn_widget(world, context.parent, (Name::new(name), node()));
        match surface.clone() {
            Some(token) => world.entity_mut(entity).insert(ThemeBackgroundColor(token)),
            None => world
                .entity_mut(entity)
                .insert(BackgroundColor(Color::NONE)),
        };
        Ok(entity)
    })
    .with_icon(icon)
}

/// The widgets Jackdaw ships in the Add menu.
fn builtin_widget_definitions() -> Vec<WidgetDefinition> {
    use bevy::ui_widgets::{
        Button, Checkbox, RadioButton, ScrollArea, Slider, SliderRange, SliderValue,
    };

    vec![
        container_definition(
            "ui.panel",
            "Panel",
            Icon::PanelTop,
            || Node {
                min_width: px(160),
                min_height: px(120),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(8)),
                row_gap: px(6),
                ..default()
            },
            Some(feathers_tokens::PANE_BODY_BG),
        ),
        container_definition(
            "ui.row",
            "Row",
            Icon::Columns3,
            || Node {
                min_width: px(160),
                min_height: px(32),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(6),
                ..default()
            },
            None,
        ),
        container_definition(
            "ui.column",
            "Column",
            Icon::Rows3,
            || Node {
                min_width: px(120),
                min_height: px(80),
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                ..default()
            },
            None,
        ),
        container_definition(
            "ui.grid",
            "Grid",
            Icon::Grid3x3,
            || Node {
                min_width: px(160),
                min_height: px(120),
                display: Display::Grid,
                grid_template_columns: RepeatedGridTrack::flex(2, 1.0),
                row_gap: px(6),
                column_gap: px(6),
                ..default()
            },
            None,
        ),
        WidgetDefinition::new("ui.spacer", "Spacer", "Layout", |world, context| {
            Ok(spawn_widget(
                world,
                context.parent,
                (
                    Name::new("Spacer"),
                    Node {
                        flex_grow: 1.0,
                        flex_basis: px(0),
                        ..default()
                    },
                    jackdaw_widgets_runtime::Spacer,
                    BackgroundColor(Color::NONE),
                ),
            ))
        })
        .with_icon(Icon::Space),
        // Authored as a horizontal rule; `separator_follows_parent_axis` turns
        // it on its side under a row parent, keeping this thickness.
        WidgetDefinition::new("ui.separator", "Separator", "Layout", |world, context| {
            Ok(spawn_widget(
                world,
                context.parent,
                (
                    Name::new("Separator"),
                    Node {
                        width: Val::Percent(100.0),
                        height: px(1),
                        flex_shrink: 0.0,
                        ..default()
                    },
                    jackdaw_widgets_runtime::Separator,
                    ThemeBackgroundColor(feathers_tokens::PANE_HEADER_DIVIDER),
                ),
            ))
        })
        .with_icon(Icon::SeparatorHorizontal),
        // A track and the bar inside it; `progress_fill_follows_value` writes
        // the bar's width from `Progress`.
        WidgetDefinition::new(
            "ui.progress",
            "Progress Bar",
            "Display",
            |world, context| {
                let track = spawn_widget(
                    world,
                    context.parent,
                    (
                        Name::new("ProgressBar"),
                        Node {
                            width: px(180),
                            height: px(8),
                            border_radius: BorderRadius::all(px(4)),
                            overflow: Overflow::clip(),
                            ..default()
                        },
                        jackdaw_widgets_runtime::Progress { value: 0.5 },
                        ThemeBackgroundColor(feathers_tokens::SLIDER_BG),
                    ),
                );
                world.spawn((
                    Name::new("Fill"),
                    Node {
                        width: Val::Percent(50.0),
                        height: Val::Percent(100.0),
                        border_radius: BorderRadius::all(px(4)),
                        ..default()
                    },
                    jackdaw_widgets_runtime::ProgressFill,
                    ThemeBackgroundColor(feathers_tokens::SLIDER_BAR),
                    ChildOf(track),
                ));
                Ok(track)
            },
        )
        .with_icon(Icon::Gauge),
        // `ThemeTextColor` rather than the inheritable variant: this entity
        // holds the text itself.
        WidgetDefinition::new("ui.label", "Label", "Display", |world, context| {
            Ok(spawn_widget(
                world,
                context.parent,
                (
                    Name::new("Label"),
                    Text::new("Label"),
                    TextFont {
                        font_size: FontSize::Px(16.0),
                        ..default()
                    },
                    ThemeTextColor(feathers_tokens::TEXT_MAIN),
                ),
            ))
        })
        .with_icon(Icon::Type),
        WidgetDefinition::new("ui.image", "Image", "Display", |world, context| {
            Ok(spawn_widget(
                world,
                context.parent,
                (
                    Name::new("Image"),
                    Node {
                        width: px(96),
                        height: px(96),
                        ..default()
                    },
                    ImageNode::default(),
                ),
            ))
        })
        .with_icon(Icon::Image),
        // The caption is a child because the inheritable text colour
        // propagates downward only.
        WidgetDefinition::new("ui.button", "Button", "Controls", |world, context| {
            let button = spawn_widget(
                world,
                context.parent,
                (
                    Name::new("Button"),
                    Node {
                        min_width: px(96),
                        min_height: px(32),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        padding: UiRect::axes(px(12), px(6)),
                        border_radius: BorderRadius::all(tokens::CORNER_RADIUS_LG),
                        ..default()
                    },
                    Button,
                    ButtonVariant::Normal,
                    TabIndex(0),
                    FocusIndicator,
                    EntityCursor::System(SystemCursorIcon::Pointer),
                    ThemeBackgroundColor(feathers_tokens::BUTTON_BG),
                    InheritableThemeTextColor(feathers_tokens::BUTTON_TEXT),
                ),
            );
            world.spawn((
                Name::new("Caption"),
                Text::new("Button"),
                ThemedText,
                TextLayout {
                    justify: Justify::Center,
                    ..default()
                },
                TextFont {
                    font_size: FontSize::Px(14.0),
                    ..default()
                },
                ChildOf(button),
            ));
            Ok(button)
        })
        .with_icon(Icon::MousePointerClick),
        // Unchecked is the absence of `Checked`, so a fresh checkbox spawns
        // without it. The tokens match feathers' outline child, which
        // `jackdaw_widgets_runtime::authored_check_styles` swaps here instead.
        WidgetDefinition::new("ui.checkbox", "Checkbox", "Controls", |world, context| {
            Ok(spawn_widget(
                world,
                context.parent,
                (
                    Name::new("Checkbox"),
                    Node {
                        width: px(18),
                        height: px(18),
                        border: UiRect::all(px(2)),
                        border_radius: BorderRadius::all(tokens::CORNER_RADIUS),
                        ..default()
                    },
                    Checkbox,
                    TabIndex(0),
                    FocusIndicator,
                    EntityCursor::System(SystemCursorIcon::Pointer),
                    ThemeBackgroundColor(feathers_tokens::CHECKBOX_BG),
                    ThemeBorderColor(feathers_tokens::CHECKBOX_BORDER),
                ),
            ))
        })
        .with_icon(Icon::SquareCheck),
        // Spawned outside a `RadioGroup`, so it does not self-update until one
        // is added: `bevy_ui_widgets` addresses a radio change to the group.
        WidgetDefinition::new("ui.radio", "Radio Button", "Controls", |world, context| {
            Ok(spawn_widget(
                world,
                context.parent,
                (
                    Name::new("RadioButton"),
                    Node {
                        width: px(18),
                        height: px(18),
                        border: UiRect::all(px(2)),
                        border_radius: BorderRadius::all(px(9)),
                        ..default()
                    },
                    RadioButton,
                    TabIndex(0),
                    FocusIndicator,
                    EntityCursor::System(SystemCursorIcon::Pointer),
                    BackgroundColor(Color::NONE),
                    ThemeBorderColor(feathers_tokens::RADIO_BORDER),
                ),
            ))
        })
        .with_icon(Icon::CircleDot),
        // Without feathers' generated knob, the track taking `SWITCH_BG_CHECKED`
        // is the only thing that shows the switch is on.
        WidgetDefinition::new(
            "ui.toggle",
            "Toggle Switch",
            "Controls",
            |world, context| {
                Ok(spawn_widget(
                    world,
                    context.parent,
                    (
                        Name::new("ToggleSwitch"),
                        Node {
                            width: px(40),
                            height: px(22),
                            border: UiRect::all(px(2)),
                            border_radius: BorderRadius::all(px(11)),
                            ..default()
                        },
                        Checkbox,
                        jackdaw_widgets_runtime::ToggleSwitch,
                        TabIndex(0),
                        FocusIndicator,
                        EntityCursor::System(SystemCursorIcon::Pointer),
                        ThemeBackgroundColor(feathers_tokens::SWITCH_BG),
                        ThemeBorderColor(feathers_tokens::SWITCH_BORDER),
                    ),
                ))
            },
        )
        .with_icon(Icon::ToggleLeft),
        // `SliderValue` and `SliderRange` are spawned explicitly rather than
        // left to required components, so the document states the value.
        WidgetDefinition::new("ui.slider", "Slider", "Controls", |world, context| {
            Ok(spawn_widget(
                world,
                context.parent,
                (
                    Name::new("Slider"),
                    Node {
                        width: px(180),
                        height: px(16),
                        border_radius: BorderRadius::all(px(8)),
                        ..default()
                    },
                    Slider::default(),
                    SliderValue(0.0),
                    SliderRange::new(0.0, 1.0),
                    TabIndex(0),
                    FocusIndicator,
                    ThemeBackgroundColor(feathers_tokens::SLIDER_BG),
                ),
            ))
        })
        .with_icon(Icon::SlidersHorizontal),
        WidgetDefinition::new(
            "ui.text_input",
            "Text Input",
            "Controls",
            |world, context| {
                Ok(spawn_widget(
                    world,
                    context.parent,
                    (
                        Name::new("TextInput"),
                        Node {
                            width: px(200),
                            min_height: px(28),
                            padding: UiRect::axes(px(8), px(4)),
                            border_radius: BorderRadius::all(tokens::CORNER_RADIUS),
                            ..default()
                        },
                        bevy::text::EditableText::default(),
                        // `EditableText` is not reflectable, so the document
                        // carries the text here instead.
                        jackdaw_widgets_runtime::TextValue::default(),
                        TextFont {
                            font_size: FontSize::Px(14.0),
                            ..default()
                        },
                        TabIndex(0),
                        FocusIndicator,
                        EntityCursor::System(SystemCursorIcon::Text),
                        ThemeTextColor(feathers_tokens::TEXT_INPUT_TEXT),
                        ThemeBackgroundColor(feathers_tokens::TEXT_INPUT_BG),
                    ),
                ))
            },
        )
        .with_icon(Icon::TextCursorInput),
        WidgetDefinition::new(
            "ui.scroll_area",
            "Scroll Area",
            "Controls",
            |world, context| {
                Ok(spawn_widget(
                    world,
                    context.parent,
                    (
                        Name::new("ScrollArea"),
                        Node {
                            width: px(220),
                            height: px(160),
                            flex_direction: FlexDirection::Column,
                            overflow: Overflow::scroll_y(),
                            row_gap: px(4),
                            ..default()
                        },
                        ScrollArea,
                        ThemeBackgroundColor(feathers_tokens::PANE_BODY_BG),
                    ),
                ))
            },
        )
        .with_icon(Icon::ScrollText),
        // The options are the whole widget: `jackdaw_widgets_runtime` rebuilds
        // the button, popup, and rows from them.
        WidgetDefinition::new("ui.dropdown", "Dropdown", "Controls", |world, context| {
            Ok(spawn_widget(
                world,
                context.parent,
                (
                    Name::new("Dropdown"),
                    Node {
                        min_width: px(140),
                        min_height: px(28),
                        align_items: AlignItems::Stretch,
                        justify_content: JustifyContent::Stretch,
                        ..default()
                    },
                    jackdaw_widgets_runtime::Dropdown {
                        options: vec!["One".to_string(), "Two".to_string(), "Three".to_string()],
                        selected: 0,
                    },
                    BackgroundColor(Color::NONE),
                ),
            ))
        })
        .with_icon(Icon::SquareChevronDown),
        // The rows are built from the options, so the document carries only the
        // choices and the selection.
        WidgetDefinition::new(
            "ui.radio_group",
            "Radio Group",
            "Controls",
            |world, context| {
                Ok(spawn_widget(
                    world,
                    context.parent,
                    (
                        Name::new("RadioGroup"),
                        Node {
                            min_width: px(140),
                            flex_direction: FlexDirection::Column,
                            row_gap: px(4),
                            ..default()
                        },
                        bevy::ui_widgets::RadioGroup,
                        jackdaw_widgets_runtime::RadioOptions {
                            options: vec![
                                "One".to_string(),
                                "Two".to_string(),
                                "Three".to_string(),
                            ],
                            selected: 0,
                        },
                        BackgroundColor(Color::NONE),
                    ),
                ))
            },
        )
        .with_icon(Icon::ListChecks),
        // The panes are authored children, in tab order; the strip above them
        // is built from the labels.
        WidgetDefinition::new("ui.tabs", "Tabs", "Layout", |world, context| {
            let tabs = spawn_widget(
                world,
                context.parent,
                (
                    Name::new("Tabs"),
                    Node {
                        min_width: px(200),
                        min_height: px(120),
                        flex_direction: FlexDirection::Column,
                        row_gap: px(6),
                        ..default()
                    },
                    jackdaw_widgets_runtime::TabStrip {
                        labels: vec!["First".to_string(), "Second".to_string()],
                        active: 0,
                    },
                    BackgroundColor(Color::NONE),
                ),
            );
            for name in ["FirstPane", "SecondPane"] {
                world.spawn((
                    Name::new(name),
                    Node {
                        flex_grow: 1.0,
                        flex_direction: FlexDirection::Column,
                        padding: UiRect::all(px(8)),
                        ..default()
                    },
                    ThemeBackgroundColor(feathers_tokens::PANE_BODY_BG),
                    ChildOf(tabs),
                ));
            }
            Ok(tabs)
        })
        .with_icon(Icon::PanelsTopLeft),
        WidgetDefinition::new(
            "ui.nine_patch",
            "Nine Patch",
            "Display",
            |world, context| {
                Ok(spawn_widget(
                    world,
                    context.parent,
                    (
                        Name::new("NinePatch"),
                        Node {
                            width: px(160),
                            height: px(96),
                            ..default()
                        },
                        ImageNode::default(),
                        jackdaw_widgets_runtime::NineSlice { border: 12.0 },
                    ),
                ))
            },
        )
        .with_icon(Icon::Grid2x2),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_api_internal::EntityIconRegistry;
    use jackdaw_api_internal::entity_icons::registered_icon;

    /// Seeding a registry with `WORLD_ENTITY_ICONS` (plus the camera-rig
    /// icon) must resolve against the real `TypePath` of each type. A typo in
    /// any path resolves to no registered type, the lookup returns None, and
    /// the matching assertion fails here.
    #[test]
    fn world_entity_icon_paths_match_real_type_paths() {
        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();
        {
            let registry = world.resource::<AppTypeRegistry>();
            let mut registry = registry.write();
            registry.register::<jackdaw_scene_types::Brush>();
            registry.register::<jackdaw_scene_types::Terrain>();
            registry.register::<crate::entity_ops::SceneFogVolume>();
            registry.register::<crate::entity_ops::SceneReflectionProbe>();
            registry.register::<crate::entity_ops::SceneAnimationPlayer>();
            registry.register::<crate::entity_ops::SceneAudioSource>();
            registry.register::<crate::reference_image::ReferenceImage>();
            #[cfg(feature = "camera_rig")]
            registry.register::<jackdaw_camera_rig::CameraRig>();
        }

        let mut icons = EntityIconRegistry::default();
        for (type_path, icon) in WORLD_ENTITY_ICONS {
            icons.register(*type_path, *icon);
        }
        #[cfg(feature = "camera_rig")]
        icons.register(CAMERA_RIG_ICON.0, CAMERA_RIG_ICON.1);
        world.insert_resource(icons);

        let cases: &[(Entity, Icon)] = &[
            (
                world.spawn(jackdaw_scene_types::Brush::default()).id(),
                Icon::Cuboid,
            ),
            (
                world.spawn(jackdaw_scene_types::Terrain::default()).id(),
                Icon::Mountain,
            ),
            (
                world.spawn(crate::entity_ops::SceneFogVolume).id(),
                Icon::CloudFog,
            ),
            (
                world.spawn(crate::entity_ops::SceneReflectionProbe).id(),
                Icon::Sparkles,
            ),
            (
                world.spawn(crate::entity_ops::SceneAnimationPlayer).id(),
                Icon::Play,
            ),
            (
                world.spawn(crate::entity_ops::SceneAudioSource).id(),
                Icon::Volume2,
            ),
            (
                world
                    .spawn(crate::reference_image::ReferenceImage::default())
                    .id(),
                Icon::PictureInPicture,
            ),
        ];
        for (entity, expected) in cases {
            assert_eq!(
                registered_icon(&world, *entity).map(Icon::unicode),
                Some(expected.unicode()),
            );
        }

        #[cfg(feature = "camera_rig")]
        {
            let rig = world.spawn(jackdaw_camera_rig::CameraRig::default()).id();
            assert_eq!(
                registered_icon(&world, rig).map(Icon::unicode),
                Some(Icon::Orbit.unicode()),
            );
        }
    }
}
