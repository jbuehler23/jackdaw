//! Built-in Jackdaw extensions. Each feature area of the editor owns
//! its dock windows through a `JackdawExtension`, so Jackdaw uses the
//! same API third-party authors do. Disable one in File > Extensions
//! to remove its windows from the layout.

use std::sync::Arc;

use bevy::prelude::*;
use jackdaw_api::prelude::{ExtensionContext, ExtensionKind, JackdawExtension, WindowDescriptor};
use jackdaw_feathers::icons::Icon;

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
        ctx.register_window(WindowDescriptor {
            id: "jackdaw.hierarchy".into(),
            name: "Scene Tree".into(),
            icon: None,
            default_area: Some("left".into()),
            priority: Some(0),
            build: Arc::new(|world, parent| {
                let icon_font = world
                    .get_resource::<jackdaw_feathers::icons::IconFont>()
                    .map(|f| f.0.clone())
                    .unwrap_or_default();
                world.spawn((ChildOf(parent), crate::layout::hierarchy_content(icon_font)));
            }),
        });

        ctx.register_window(WindowDescriptor {
            id: "jackdaw.import".into(),
            name: "Import".into(),
            icon: None,
            default_area: Some("left".into()),
            priority: Some(1),
            build: Arc::new(|world, parent| {
                world.spawn((
                    ChildOf(parent),
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
        });

        ctx.register_window(WindowDescriptor {
            id: "jackdaw.project_files".into(),
            name: "Project Files".into(),
            icon: None,
            default_area: Some("left".into()),
            priority: Some(10),
            build: Arc::new(|world, parent| {
                world.spawn((
                    ChildOf(parent),
                    crate::layout::project_files_panel_content(),
                ));
                world
                    .resource_mut::<crate::project_files::ProjectFilesState>()
                    .needs_refresh = true;
            }),
        });
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
        ctx.register_window(WindowDescriptor {
            id: "jackdaw.assets".into(),
            name: "Assets".into(),
            icon: Some(String::from(Icon::FolderOpen.unicode())),
            default_area: Some("bottom_dock".into()),
            priority: Some(0),
            build: Arc::new(|world, parent| {
                let icon_font = world
                    .get_resource::<jackdaw_feathers::icons::IconFont>()
                    .map(|f| f.0.clone())
                    .unwrap_or_default();
                world.spawn((
                    ChildOf(parent),
                    crate::asset_browser::asset_browser_panel(icon_font),
                ));
                world
                    .resource_mut::<crate::asset_browser::AssetBrowserState>()
                    .needs_refresh = true;
            }),
        });
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
        ctx.register_window(WindowDescriptor {
            id: "jackdaw.timeline".into(),
            name: "Timeline".into(),
            icon: Some(String::from(Icon::Ruler.unicode())),
            default_area: Some("bottom_dock".into()),
            priority: Some(1),
            build: Arc::new(|world, parent| {
                world.spawn((ChildOf(parent), jackdaw_animation::timeline_panel()));
            }),
        });
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
        ctx.register_window(WindowDescriptor {
            id: "jackdaw.terminal".into(),
            name: "Terminal".into(),
            icon: Some(String::from(Icon::Terminal.unicode())),
            default_area: Some("bottom_dock".into()),
            priority: Some(2),
            build: Arc::new(|world, parent| {
                world.spawn((
                    ChildOf(parent),
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
        });
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
        ctx.register_window(WindowDescriptor {
            id: "jackdaw.inspector.components".into(),
            name: "Components".into(),
            icon: None,
            default_area: Some("right_sidebar".into()),
            priority: Some(0),
            build: Arc::new(|world, parent| {
                let icon_font = world
                    .get_resource::<jackdaw_feathers::icons::IconFont>()
                    .map(|f| f.0.clone())
                    .unwrap_or_default();
                world.spawn((
                    ChildOf(parent),
                    crate::layout::inspector_components_content(icon_font),
                ));
            }),
        });

        ctx.register_window(WindowDescriptor {
            id: "jackdaw.inspector.materials".into(),
            name: "Materials".into(),
            icon: None,
            default_area: Some("right_sidebar".into()),
            priority: Some(1),
            build: Arc::new(|world, parent| {
                let icon_font = world
                    .get_resource::<jackdaw_feathers::icons::IconFont>()
                    .map(|f| f.0.clone())
                    .unwrap_or_default();
                world.spawn((
                    ChildOf(parent),
                    crate::material_browser::material_browser_panel(icon_font),
                ));
                world
                    .resource_mut::<crate::material_browser::MaterialBrowserState>()
                    .needs_rescan = true;
            }),
        });

        ctx.register_window(WindowDescriptor {
            id: "jackdaw.inspector.resources".into(),
            name: "Resources".into(),
            icon: None,
            default_area: Some("right_sidebar".into()),
            priority: Some(2),
            build: Arc::new(|world, parent| {
                world.spawn((
                    ChildOf(parent),
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
        });

        ctx.register_window(WindowDescriptor {
            id: "jackdaw.inspector.systems".into(),
            name: "Systems".into(),
            icon: None,
            default_area: Some("right_sidebar".into()),
            priority: Some(3),
            build: Arc::new(|world, parent| {
                world.spawn((
                    ChildOf(parent),
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
        });
    }
}
