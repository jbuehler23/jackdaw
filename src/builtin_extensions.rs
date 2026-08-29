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
    ("jackdaw::entity_ops::SceneFogVolume", Icon::CloudFog),
    ("jackdaw::entity_ops::SceneReflectionProbe", Icon::Sparkles),
    ("jackdaw::entity_ops::SceneAnimationPlayer", Icon::Play),
    ("jackdaw::entity_ops::SceneAudioSource", Icon::Volume2),
    ("jackdaw::reference_image::ReferenceImage", Icon::Image),
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
        for (type_path, icon) in WORLD_ENTITY_ICONS {
            ctx.register_entity_icon(*type_path, *icon);
        }
        #[cfg(feature = "camera_rig")]
        ctx.register_entity_icon(CAMERA_RIG_ICON.0, CAMERA_RIG_ICON.1);

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
/// can coexist in the dock tree.
///
/// One panel shows either the 3D world or the 2D canvas, so the operators
/// addressing the canvas are registered here too rather than on a panel of
/// their own.
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

/// Animation timeline in the bottom dock.
#[derive(Default)]
pub struct TimelineExtension;

impl JackdawExtension for TimelineExtension {
    fn id(&self) -> String {
        "jackdaw.timeline".to_string()
    }

    fn label(&self) -> String {
        "Timeline".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_window(
            WindowDescriptor::new("jackdaw.timeline")
                .with_name("Timeline")
                .with_icon(Icon::Ruler.unicode())
                .with_default_area(DefaultArea::BottomDock)
                .with_priority(1)
                .with_build(|window| {
                    window.spawn(jackdaw_animation::timeline_panel());
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
/// # Bevy UI components rather than feathers controls
///
/// A UI widget is authored content: it is written into the scene document and
/// spawned again from that document on load. The `bevy_feathers` controls do
/// not survive that round trip. They are `SceneComponent`s, which only
/// `spawn_scene` can materialise; re-inserting one from a reflected patch logs
/// an error and leaves a bare marker. The editor's save policy also skips the
/// `bevy_feathers::` prefix as widget implementation detail. The definitions
/// below therefore assemble `bevy_ui` and `bevy_ui_widgets`.
///
/// # Theming
///
/// The feathers styling components are plain `Reflect` components:
/// `ThemeBackgroundColor`, `ThemeBorderColor`, `ThemeTextColor`,
/// `InheritableThemeTextColor`, `ButtonVariant`, `FocusIndicator`, and
/// `EntityCursor`. A definition spawns them directly, and
/// [`crate::scene_io`]'s always-save list carries them through a round trip, so
/// a UI widget names a design token rather than a colour and is repainted when
/// the theme changes.
///
/// The dynamic styling of the multi-entity controls does not carry over.
/// `update_button_styles` operates on one entity, so a button gets its full
/// hover, pressed, disabled, and focus treatment. A checkbox, radio, or toggle
/// switch drives its checked and hover states from generated children
/// (`CheckboxMark`, `RadioOutline`, `ToggleSwitchSlide`) whose marker types are
/// private to feathers, so those widgets get the theme-token surface
/// (background, border, focus ring, cursor) and nothing that responds to
/// `Checked`.
///
/// Value behaviour comes from [`crate::authored_widgets`], which attaches the
/// `bevy_ui_widgets` self-update observers globally.
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
    }
}

/// Spawn one authored widget under the resolved parent.
fn spawn_widget(world: &mut World, parent: Option<Entity>, bundle: impl Bundle) -> Entity {
    let mut entity = world.spawn(bundle);
    if let Some(parent) = parent {
        entity.insert(ChildOf(parent));
    }
    entity.id()
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
///
/// Each is a single entity, except the button, whose caption is a child because
/// `InheritableThemeTextColor` colours descendants and not the entity holding
/// it. No other definition spawns children.
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
        // `ThemeTextColor` rather than the inheritable variant: this entity
        // holds the text itself, and the inheritable one reaches descendants.
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
        // The one widget feathers styles entirely on one entity:
        // `update_button_styles` reads `ButtonVariant`, `Hovered`,
        // `ThemeBackgroundColor`, and `InheritableThemeTextColor` from the
        // button itself, so an authored button gets hover, pressed, disabled,
        // and focus treatment. The caption is a child because the inheritable
        // text colour propagates downward only.
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
        // without it.
        //
        // The tokens are the ones feathers puts on its checkbox outline child,
        // which this one-entity checkbox stands in for. Feathers switches them
        // from systems that query private marker types, so none of that reaches
        // a one-entity box; `jackdaw_widgets_runtime::authored_check_styles`
        // does the resting/checked half of the swap here instead, using the
        // same tokens. The hover and press treatments are still feathers-only:
        // they read picking state a document does not carry.
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
        // A radio spawns without `Checked` and outside a `RadioGroup`, since
        // which radios belong together is a structural authoring decision. Its
        // ring is the only part feathers themes on one entity; the filled mark
        // is a generated child. So the ring is the whole chosen treatment here,
        // taken from `RADIO_BORDER_CHECKED` once `Checked` lands.
        //
        // A radio on its own therefore never self-updates: `bevy_ui_widgets`
        // addresses a radio change to the group, so the observer in
        // `crate::authored_widgets` does nothing until a `RadioGroup` ancestor
        // exists, which is added through the inspector.
        WidgetDefinition::new("ui.radio", "Radio Button", "Controls", |world, context| {
            Ok(spawn_widget(
                world,
                context.parent,
                (
                    Name::new("Radio Button"),
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
        // The feathers toggle switch carries these two tokens on its root, so
        // the authored one matches it at rest; the sliding knob is the
        // generated child this lacks. With the knob missing, the track taking
        // `SWITCH_BG_CHECKED` is the only thing that says the switch is on --
        // which is what these tokens, and not the checkbox ones, get it.
        WidgetDefinition::new(
            "ui.toggle",
            "Toggle Switch",
            "Controls",
            |world, context| {
                Ok(spawn_widget(
                    world,
                    context.parent,
                    (
                        Name::new("Toggle Switch"),
                        Node {
                            width: px(40),
                            height: px(22),
                            border: UiRect::all(px(2)),
                            border_radius: BorderRadius::all(px(11)),
                            ..default()
                        },
                        Checkbox,
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
        // left to `Slider`'s required components, so the document states the
        // displayed value rather than inheriting an upstream default. The
        // track takes the theme token; the filled bar is a generated child
        // feathers styles privately.
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
                        Name::new("Text Input"),
                        Node {
                            width: px(200),
                            min_height: px(28),
                            padding: UiRect::axes(px(8), px(4)),
                            border_radius: BorderRadius::all(tokens::CORNER_RADIUS),
                            ..default()
                        },
                        bevy::text::EditableText::default(),
                        // `EditableText` is not reflectable, so the document
                        // carries the text here instead; the widget crate
                        // keeps the two in sync.
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
                        Name::new("Scroll Area"),
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
                Icon::Image,
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
