use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{report_to_caller, warn_caller};
use jackdaw_tilemap::TilesetSetup;
use jackdaw_tileset::Tileset;

use crate::tileset_dock::{DockView, TilesetDockViews, dock_path, refill, refill_all};
use crate::tileset_edit::text;

/// The id of a mode of the Tileset window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ModeId(pub &'static str);

impl ModeId {
    /// Pick tiles and blocks of tiles.
    pub const TILES: ModeId = ModeId("tiles");
    /// Paint terrains on tile zones.
    pub const TERRAINS: ModeId = ModeId("terrains");
    /// Mark rectangles of tiles as objects.
    pub const OBJECTS: ModeId = ModeId("objects");
}

impl Default for ModeId {
    fn default() -> Self {
        ModeId::TILES
    }
}

/// What a mode's side panel is built from.
pub struct ModePanel<'a> {
    pub root: Entity,
    /// The upper column beside the sheet.
    pub list: Entity,
    /// The column under `list`.
    pub detail: Entity,
    pub tileset: &'a Tileset,
    pub setup: &'a TilesetSetup,
    pub view: &'a DockView,
}

/// What a mode's sheet overlay is drawn from: sheet `sheet`, drawn in the
/// node `frame`.
pub struct ModeSheet<'a> {
    pub root: Entity,
    pub frame: Entity,
    pub sheet: u32,
    pub tileset: &'a Tileset,
    pub setup: &'a TilesetSetup,
    pub view: &'a DockView,
}

/// A mode of the Tileset window, shown in its mode bar.
#[derive(Clone, Debug)]
pub struct TilesetMode {
    pub id: ModeId,
    pub label: &'static str,
    /// Place in the mode bar, lowest first.
    pub order: i32,
    /// Builds the side panel.
    pub panel: fn(&mut World, &ModePanel),
    /// Draws over each sheet, after its tiles.
    pub overlay: Option<fn(&mut World, &ModeSheet)>,
}

/// The registered modes of the Tileset window.
#[derive(Resource, Default, Clone, Debug)]
pub struct TilesetModes(Vec<TilesetMode>);

impl TilesetModes {
    /// Add `mode`, or replace the mode of the same id.
    pub fn register(&mut self, mode: TilesetMode) {
        self.0.retain(|m| m.id != mode.id);
        let at = self.0.partition_point(|m| m.order <= mode.order);
        self.0.insert(at, mode);
    }

    /// Remove the mode `id`.
    pub fn unregister(&mut self, id: ModeId) {
        self.0.retain(|m| m.id != id);
    }

    /// The modes in mode bar order.
    pub fn iter(&self) -> impl Iterator<Item = &TilesetMode> {
        self.0.iter()
    }

    pub fn get(&self, id: ModeId) -> Option<&TilesetMode> {
        self.0.iter().find(|m| m.id == id)
    }

    /// The mode named `name` by id or label, in any case.
    pub fn find(&self, name: &str) -> Option<&TilesetMode> {
        (self.0.iter())
            .find(|m| m.id.0.eq_ignore_ascii_case(name) || m.label.eq_ignore_ascii_case(name))
    }
}

/// Registers modes of the Tileset window from a plugin.
pub trait TilesetModeAppExt {
    fn register_tileset_mode(&mut self, mode: TilesetMode) -> &mut Self;
}

impl TilesetModeAppExt for App {
    fn register_tileset_mode(&mut self, mode: TilesetMode) -> &mut Self {
        (self.world_mut().get_resource_or_init::<TilesetModes>()).register(mode);
        self
    }
}

/// Registers modes of the Tileset window from an extension, for as long as
/// it is loaded.
pub trait TilesetModeExtensionExt {
    fn register_tileset_mode(&mut self, mode: TilesetMode) -> &mut Self;
}

impl TilesetModeExtensionExt for ExtensionContext<'_> {
    fn register_tileset_mode(&mut self, mode: TilesetMode) -> &mut Self {
        self.spawn(RegisteredTilesetMode(mode));
        self
    }
}

/// A mode an extension registered, owned by the extension's entity: the
/// mode goes when the extension unloads.
#[derive(Component, Clone, Debug)]
pub struct RegisteredTilesetMode(pub TilesetMode);

fn on_mode_added(
    added: On<Add<RegisteredTilesetMode>>,
    registered: Query<&RegisteredTilesetMode>,
    mut commands: Commands,
) -> Result {
    let mode = registered.get(added.entity)?.0.clone();
    commands.queue(move |world: &mut World| {
        world.get_resource_or_init::<TilesetModes>().register(mode);
        refill_all(world);
    });
    Ok(())
}

fn on_mode_removed(
    removed: On<Remove<RegisteredTilesetMode>>,
    registered: Query<&RegisteredTilesetMode>,
    mut commands: Commands,
) -> Result {
    let id = registered.get(removed.entity)?.0.id;
    commands.queue(move |world: &mut World| {
        world.get_resource_or_init::<TilesetModes>().unregister(id);
        refill_all(world);
    });
    Ok(())
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<TilesetModes>()
        .add_observer(on_mode_added)
        .add_observer(on_mode_removed)
        .register_tileset_mode(TilesetMode {
            id: ModeId::TILES,
            label: "Tiles",
            order: 0,
            panel: crate::tileset_dock::sheet_panel,
            overlay: None,
        })
        .register_tileset_mode(TilesetMode {
            id: ModeId::TERRAINS,
            label: "Terrains",
            order: 10,
            panel: crate::tileset_dock::sheet_panel,
            overlay: Some(crate::tileset_bits::terrain_overlay),
        })
        .register_tileset_mode(TilesetMode {
            id: ModeId::OBJECTS,
            label: "Objects",
            order: 100,
            panel: crate::tileset_dock::objects_panel,
            overlay: Some(crate::tileset_dock::object_outlines),
        });
}

/// Switch the mode of a Tileset tab.
#[operator(
    id = "tileset.mode",
    label = "Tileset Mode",
    description = "Switch a Tileset tab to one of its modes: tiles picks tiles, terrains \
                   paints terrains on tile zones with the brush terrain (right erases), \
                   objects marks rectangles of tiles as objects; extensions add more. With \
                   no mode it steps to the next.",
    allows_undo = false,
    is_available = a_tileset_is_shown,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the tab with the \
                   keyboard, then the open tileset."
        ),
        mode(String, doc = "Mode id or label; empty steps to the next."),
    )
)]
pub fn tileset_mode(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let (path, mode) = (text(&params, "path"), text(&params, "mode"));
    commands.queue(move |world: &mut World| match switch(world, &path, &mode) {
        Ok(message) => report_to_caller(world, message),
        Err(err) => warn_caller(world, format!("tileset.mode: {err}")),
    });
    OperatorResult::Finished
}

fn a_tileset_is_shown(views: Res<TilesetDockViews>) -> bool {
    !views.0.is_empty()
}

fn switch(world: &mut World, path: &str, name: &str) -> Result<String, String> {
    let path = dock_path(world, path)?;
    let modes = world
        .get_resource::<TilesetModes>()
        .cloned()
        .unwrap_or_default();
    let mut views = world.resource_mut::<TilesetDockViews>();
    let view = (views.0.get_mut(&path)).ok_or_else(|| format!("{path} has no Tileset tab"))?;
    let mode = if name.is_empty() {
        let at = modes.iter().position(|m| m.id == view.mode);
        let next = at.map_or(0, |i| (i + 1) % modes.0.len().max(1));
        modes.0.get(next)
    } else {
        modes.find(name)
    };
    let Some(mode) = mode else {
        let names: Vec<&str> = modes.iter().map(|m| m.id.0).collect();
        return Err(format!("mode is one of {}, not `{name}`", names.join(", ")));
    };
    view.set_mode(mode.id);
    let label = mode.label;
    refill(world, &path);
    Ok(format!("{label} mode"))
}
