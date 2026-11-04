//! Tilemap and tileset editing for the Jackdaw scene editor: tile edit mode
//! and its tools, the tile palette, the Tileset window, tileset import, and
//! the tilemap data kept beside each scene.
//!
//! Add [`TileEditorPlugin`] after the editor's own plugins.

use std::path::Path;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ExtensionAppExt as _;
use jackdaw_editor_core::inspector::ComponentCardRegistry;
use jackdaw_editor_core::project_window::FileOpeners;
use jackdaw_editor_core::toolbar_2d::Toolbar2dSlots;

pub mod tile_palette;
pub mod tile_preview;
pub mod tile_select;
pub mod tile_stamp;
pub mod tile_tools;
pub mod tilemap;
pub mod tilemap_card;
pub mod tilemap_ops;
pub mod tilemap_region_ops;
pub mod tilemap_render;
pub mod tilemap_resolve;
pub mod tilemap_shape_ops;
pub mod tilemap_stroke;
pub mod tileset_bits;
pub mod tileset_dock;
pub mod tileset_edit;
pub mod tileset_import;
pub mod tileset_layout;
pub mod tileset_layout_ops;
pub mod tileset_suggest;
pub mod toolbar;

pub use jackdaw_editor_core::tile_mode::TILE_EDITOR_EXTENSION_ID;

/// The tile editor's systems, its hooks into the editor, and
/// [`TileEditorExtension`].
pub struct TileEditorPlugin;

impl Plugin for TileEditorPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            tileset_import::plugin,
            tilemap::plugin,
            tile_tools::plugin,
            tile_palette::plugin,
            tile_preview::plugin,
            tile_stamp::plugin,
            tile_select::plugin,
            tilemap_render::plugin,
            tilemap_resolve::plugin,
        ))
        .add_plugins((
            tileset_dock::plugin,
            tileset_bits::plugin,
            tileset_layout::plugin,
            tileset_suggest::plugin,
            tilemap_card::plugin,
        ));
        let world = app.world_mut();
        world
            .get_resource_or_init::<ComponentCardRegistry>()
            .register::<jackdaw_scene_types::Tilemap>(tilemap_card::fill_tilemap_card_body);
        let mut openers = world.get_resource_or_init::<FileOpeners>();
        openers.register(open_tileset_file);
        openers.register(open_array_texture);
        world
            .get_resource_or_init::<Toolbar2dSlots>()
            .register(toolbar::SLOT);
        app.register_extension::<TileEditorExtension>();
    }
}

/// The tile editor's operators, the tile palette window and the Tileset
/// window.
#[derive(Default)]
pub struct TileEditorExtension;

impl JackdawExtension for TileEditorExtension {
    fn id(&self) -> String {
        TILE_EDITOR_EXTENSION_ID.to_string()
    }

    fn label(&self) -> String {
        "Tiles".to_string()
    }

    fn description(&self) -> String {
        "Tilemap painting, the tile palette and the Tileset window.".to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        tileset_dock::add_to_extension(ctx);
        tileset_bits::add_to_extension(ctx);
        tileset_edit::add_to_extension(ctx);
        tilemap_stroke::add_to_extension(ctx);
        tile_tools::add_to_extension(ctx);
        tilemap_ops::add_to_extension(ctx);
        tilemap_shape_ops::add_to_extension(ctx);
        tilemap_region_ops::add_to_extension(ctx);
        tileset_import::add_to_extension(ctx);
        tileset_layout_ops::add_to_extension(ctx);
    }
}

fn open_tileset_file(world: &mut World, path: &Path) -> bool {
    let is_tileset = {
        let kinds = world.resource::<AssetKinds>();
        jackdaw_editor_core::asset_files::read_asset_kind(path, kinds).type_path()
            == Some(<jackdaw_tileset::Tileset as bevy::reflect::TypePath>::type_path())
    };
    if is_tileset {
        tileset_dock::edit_tileset(world, &path.to_string_lossy());
    }
    is_tileset
}

/// An array texture opens the tileset made from it; one with no tileset is
/// left to the Project window.
fn open_array_texture(world: &mut World, path: &Path) -> bool {
    if jackdaw_editor_core::texture_files::array_layout_of(path).is_none() {
        return false;
    }
    let Some(tileset) = tileset_import::tileset_of_image(world, path) else {
        return false;
    };
    tileset_dock::edit_tileset(world, &tileset);
    true
}
