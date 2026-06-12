//! Built-in Jackdaw extensions. Each feature area of the editor owns
//! its dock windows through a `JackdawExtension`, so Jackdaw uses the
//! same API third-party authors do. Disable one in File > Extensions
//! to remove its windows from the layout.

use bevy::prelude::*;
use jackdaw_api::{
    DefaultArea, ExtensionPoint, HierarchyWindow, InspectorWindow,
    prelude::{ExtensionContext, ExtensionKind, JackdawExtension, WindowDescriptor},
};
use jackdaw_feathers::icons::Icon;

/// Reflect type paths of jackdaw's authorable world components paired with
/// their outliner icon, in priority order. Type paths use each type's
/// defining module (what `TypePath` reports), not a re-export path. Tested
/// against the real `TypePath` so a typo fails loudly.
pub(crate) const WORLD_ENTITY_ICONS: &[(&str, Icon)] = &[
    ("jackdaw_jsn::types::Brush", Icon::Cuboid),
    ("jackdaw_jsn::types::Terrain", Icon::Mountain),
    ("jackdaw_jsn::types::NavmeshRegion", Icon::Waypoints),
    ("jackdaw::entity_ops::SceneFogVolume", Icon::CloudFog),
    ("jackdaw::entity_ops::SceneReflectionProbe", Icon::Sparkles),
    ("jackdaw::entity_ops::SceneAnimationPlayer", Icon::Play),
    ("jackdaw::entity_ops::SceneAudioSource", Icon::Volume2),
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
                                font_size: 11.0,
                                ..default()
                            },
                            TextColor(Color::srgba(1.0, 1.0, 1.0, 0.3)),
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
    }
}

/// 3D viewport, registered as a regular dock panel so multiple
/// instances (quad-view, stacked viewports for animation work, etc.)
/// can coexist in the dock tree.
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
        ctx.register_window(
            WindowDescriptor::new("jackdaw.viewport")
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
                                font_size: 11.0,
                                ..default()
                            },
                            TextColor(Color::srgba(1.0, 1.0, 1.0, 0.3)),
                        )],
                    ));
                }),
        );
    }
}

/// Right-sidebar stack: Components, Materials, Resources, Systems.
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
            WindowDescriptor::new("jackdaw.inspector.materials")
                .with_name("Materials")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(1)
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
                .with_priority(2)
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
                                font_size: 11.0,
                                ..default()
                            },
                            TextColor(Color::srgba(1.0, 1.0, 1.0, 0.3)),
                        )],
                    ));
                }),
        );

        ctx.register_window(
            WindowDescriptor::new("jackdaw.inspector.systems")
                .with_name("Systems")
                .with_default_area(DefaultArea::RightSidebar)
                .with_priority(3)
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
                                font_size: 11.0,
                                ..default()
                            },
                            TextColor(Color::srgba(1.0, 1.0, 1.0, 0.3)),
                        )],
                    ));
                }),
        );
    }
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
            registry.register::<jackdaw_jsn::Brush>();
            registry.register::<jackdaw_jsn::Terrain>();
            registry.register::<jackdaw_jsn::NavmeshRegion>();
            registry.register::<crate::entity_ops::SceneFogVolume>();
            registry.register::<crate::entity_ops::SceneReflectionProbe>();
            registry.register::<crate::entity_ops::SceneAnimationPlayer>();
            registry.register::<crate::entity_ops::SceneAudioSource>();
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
                world.spawn(jackdaw_jsn::Brush::default()).id(),
                Icon::Cuboid,
            ),
            (
                world.spawn(jackdaw_jsn::Terrain::default()).id(),
                Icon::Mountain,
            ),
            (
                world.spawn(jackdaw_jsn::NavmeshRegion::default()).id(),
                Icon::Waypoints,
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
