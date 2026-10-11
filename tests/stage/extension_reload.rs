//! Reloading an extension, as a rebuild does, keeps the panels it had open.

use crate::util;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::{
    DockLayoutSnapshot, ExtensionAppExt as _, disable_extension, enable_extension,
    load_static_extension,
};
use jackdaw_panels::{
    DockAreaStyle,
    tree::{DockLeaf, DockTree, NodeId},
};

const PANEL: &str = "reload_sample.panel";

#[derive(Default)]
struct PanelExtension;

impl JackdawExtension for PanelExtension {
    fn id(&self) -> String {
        "reload_sample".to_string()
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_window(
            WindowDescriptor::new(PANEL)
                .with_name("Sample Panel")
                .with_default_area(DefaultArea::BottomDock)
                .with_build(|window| {
                    window.spawn(Text::new("Sample panel"));
                }),
        );
    }
}

/// The windows of `leaf`'s tabs in order, and the one in front.
fn tabs(app: &App, leaf: NodeId) -> (Vec<String>, Option<String>) {
    let tree = app.world().resource::<DockTree>();
    let leaf = tree
        .get(leaf)
        .and_then(|node| node.as_leaf())
        .expect("leaf");
    let windows = leaf.tabs().map(|(window, _)| window.to_string()).collect();
    let front = leaf.active.and_then(|active| {
        leaf.tabs()
            .find_map(|(window, tab)| (tab == active).then(|| window.to_string()))
    });
    (windows, front)
}

#[test]
fn a_reloaded_extension_keeps_its_open_panel_where_it_was() {
    let mut app = util::editor_test_app();
    app.register_extension::<PanelExtension>();
    enable_extension(app.world_mut(), "reload_sample");
    app.update();
    let leaf = app.world_mut().resource_mut::<DockTree>().set_root_leaf(
        DockLeaf::new("bottom_dock", DockAreaStyle::default())
            .with_windows(vec![PANEL.to_string(), "jackdaw.build".to_string()]),
    );
    let panel_tab = {
        let tree = app.world().resource::<DockTree>();
        let leaf = tree
            .get(leaf)
            .and_then(|node| node.as_leaf())
            .expect("leaf");
        leaf.tabs()
            .find(|(window, _)| *window == PANEL)
            .map(|(_, tab)| tab)
    }
    .expect("the panel's tab");
    app.world_mut()
        .resource_mut::<DockTree>()
        .set_active(leaf, panel_tab);
    app.update();

    let layout = DockLayoutSnapshot::capture(app.world());
    disable_extension(app.world_mut(), "reload_sample");
    app.update();
    assert_eq!(
        tabs(&app, leaf).0,
        vec!["jackdaw.build".to_string()],
        "unloading closes the panel"
    );
    load_static_extension(app.world_mut(), Box::new(PanelExtension));
    layout.restore(app.world_mut());
    app.update();

    assert_eq!(
        tabs(&app, leaf),
        (
            vec![PANEL.to_string(), "jackdaw.build".to_string()],
            Some(PANEL.to_string())
        ),
        "the reloaded panel is back in its place and in front"
    );
}

#[test]
fn a_panel_the_reloaded_extension_no_longer_offers_stays_closed() {
    let mut app = util::editor_test_app();
    app.register_extension::<PanelExtension>();
    enable_extension(app.world_mut(), "reload_sample");
    app.update();
    let leaf = app.world_mut().resource_mut::<DockTree>().set_root_leaf(
        DockLeaf::new("bottom_dock", DockAreaStyle::default())
            .with_windows(vec![PANEL.to_string(), "jackdaw.build".to_string()]),
    );
    app.update();

    let layout = DockLayoutSnapshot::capture(app.world());
    disable_extension(app.world_mut(), "reload_sample");
    layout.restore(app.world_mut());
    app.update();

    assert_eq!(tabs(&app, leaf).0, vec!["jackdaw.build".to_string()]);
}
