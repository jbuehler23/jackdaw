//! Wiring the shared splat material into the editor.
//!
//! A terrain draws with `jackdaw_terrain`'s splat material once it has at
//! least one material slot and everything those materials name has loaded.
//! Otherwise it draws with the plain editor material.
//!
//! The editor contributes the name store: it hands
//! [`resolve_with`] a closure that looks a slot's name up in
//! [`MaterialRegistry`]. Which material slot is albedo, that slot order is
//! the id space, and how the arrays stack all live in the shared crate, so
//! the editor and a built game turn the same names into the same pixels.

use std::sync::Arc;

use bevy::asset::{AssetEvent, LoadState};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use jackdaw_terrain::render::{
    SplatArrayHandles, SplatBuildError, TerrainRenderPlugin, TerrainSplatMaterial,
    TextureSetImages, control_image_from_bytes, resolve_with, slope_image, splat_images,
};
use jackdaw_terrain::sidecar::{AutoTerrainSettings, TerrainMaterialSlot};
use jackdaw_terrain::splat::ControlTexels;
use jackdaw_terrain::texture_set::TextureSet;
use jackdaw_terrain::{Control, GridRect};

use super::regions::{
    ControlMask, TerrainRegionView, mask_control_block, masked_control, region_side,
};
use super::{TerrainDataStore, TerrainDirtyChunks};
use crate::material_assets::MaterialRegistry;

pub(super) fn plugin(app: &mut App) {
    app.add_plugins(TerrainRenderPlugin)
        .init_resource::<TerrainSplatMaterials>()
        .add_systems(
            Update,
            (
                invalidate_on_asset_change,
                resolve_terrain_materials,
                build_ready_materials,
                refresh_autoterrain,
                refresh_control_maps,
                refresh_slope_maps,
            )
                .chain()
                .run_if(in_state(crate::AppState::Editor)),
        );
}

/// What the editor knows about one terrain's splat rendering.
#[derive(Default)]
struct SplatEntry {
    /// The material list this entry was resolved from. A different list
    /// means everything below is stale.
    slots: Vec<TerrainMaterialSlot>,
    /// The slots, resolved into the array builder's input.
    set: TextureSet,
    /// Handles for what the resolved materials name.
    images: TextureSetImages,
    /// Names of slots whose material has no file in this project, for the
    /// Textures tab to say so by name.
    missing: Vec<String>,
    /// `None` until every image has decoded.
    material: Option<Handle<TerrainSplatMaterial>>,
    /// The control map this material samples, kept so a repaint can
    /// re-upload it without rebuilding the material.
    control: Option<Handle<Image>>,
    /// The uploaded control texture's texels, as bytes, kept between frames
    /// so a paint stroke rewrites only the rows its brush touched.
    control_texels: Vec<u8>,
    /// The region view [`Self::control_texels`] was masked for. A change
    /// here re-uploads the whole map; nothing else does.
    control_mask: ControlMask,
    /// The slope map autoterrain reads, kept so sculpting can re-upload it
    /// without rebuilding the material.
    slope: Option<Handle<Image>>,
    /// The heights [`Self::slope`] was built from, compared by pointer to
    /// spot an edit. Holding this `Arc` is what makes that comparison
    /// sound: the store patches an edited map in place only when it holds
    /// the sole reference, so this second reference forces every edit to
    /// come back as a fresh allocation with a new pointer.
    ///
    /// The cost is one full heightmap per splat terrain beside the store's
    /// own copy (4 MB at a 1024 grid) and a fresh allocation on every
    /// sculpt commit.
    slope_heights: Option<Arc<jackdaw_terrain::Heightmap>>,
    /// Terrain geometry the material was built against. The control map's
    /// size and the shader's UV scaling both depend on these, so a change
    /// to either has to rebuild rather than re-upload.
    built_size: Vec2,
    built_resolution: u32,
    /// The autoterrain settings the uniform carries. A change here rebuilds
    /// nothing: `refresh_autoterrain` writes the new numbers into the bound
    /// material.
    built_autoterrain: AutoTerrainSettings,
    /// Whether the chunks are carrying a material this entry has dropped.
    /// Chunks pick their material up at mesh-build time, so dropping the
    /// handle here is not enough: a terrain whose last material was removed
    /// would keep drawing the arrays it was built with.
    needs_chunk_rebuild: bool,
    /// The last problem reported for this entry: an invalid set, a failed
    /// texture load, a size mismatch. Shown verbatim in the Textures tab.
    /// `Some` also means "already logged", so a broken set complains once
    /// rather than every frame.
    error: Option<String>,
}

/// Splat materials by sidecar path, one per terrain.
///
/// Per terrain rather than per chunk: the control map and the terrain's
/// size are what differ between terrains, and every chunk of one terrain
/// shares both.
#[derive(Resource, Default)]
pub(crate) struct TerrainSplatMaterials {
    entries: HashMap<String, SplatEntry>,
}

impl TerrainSplatMaterials {
    /// The material a terrain's chunks should carry, if it has one ready.
    pub(crate) fn material(&self, data_path: &str) -> Option<Handle<TerrainSplatMaterial>> {
        self.entries
            .get(data_path)
            .and_then(|entry| entry.material.clone())
    }

    /// The last error reported for a terrain's materials, a validation
    /// failure or a texture that could not load, for the Textures tab to
    /// show beside the list.
    pub(crate) fn error(&self, data_path: &str) -> Option<&str> {
        self.entries
            .get(data_path)
            .and_then(|entry| entry.error.as_deref())
    }

    /// Names of this terrain's slots whose material has no file behind it.
    /// Those slots keep their ids and draw the fallback layer.
    pub(crate) fn missing(&self, data_path: &str) -> &[String] {
        self.entries
            .get(data_path)
            .map(|entry| entry.missing.as_slice())
            .unwrap_or(&[])
    }

    /// Albedo image handles, index-aligned with the terrain's material
    /// list: texture id `i` is slot `i`'s thumbnail. `None` where the
    /// material has no base colour texture or is missing.
    pub(crate) fn albedo_thumbnails(&self, data_path: &str) -> &[Option<Handle<Image>>] {
        self.entries
            .get(data_path)
            .map(|entry| entry.images.albedo.as_slice())
            .unwrap_or(&[])
    }
}

#[cfg(test)]
impl TerrainSplatMaterials {
    /// Inject an entry directly rather than driving it through
    /// `resolve_terrain_materials` -> `build_ready_materials`, which needs
    /// an `AssetServer` and decoded images.
    pub(crate) fn insert_test_entry(
        &mut self,
        data_path: impl Into<String>,
        missing: Vec<String>,
        error: Option<String>,
    ) {
        self.entries.insert(
            data_path.into(),
            SplatEntry {
                missing,
                error,
                ..default()
            },
        );
    }
}

/// Resolve every terrain's material list into the array builder's input.
///
/// Runs every frame rather than on a change signal: resolution is a
/// registry lookup per slot and the result is compared before it is stored,
/// so an unchanged terrain writes nothing. A material that finished loading
/// late, or that was saved after the terrain referenced it, is picked up on
/// the next frame without invalidation bookkeeping.
fn resolve_terrain_materials(
    mut materials: ResMut<TerrainSplatMaterials>,
    mut paint_state: ResMut<super::TerrainPaintState>,
    store: Res<TerrainDataStore>,
    registry: Res<MaterialRegistry>,
    standard: Res<Assets<StandardMaterial>>,
    assets: Res<AssetServer>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
) {
    let live: Vec<String> = terrains
        .iter()
        .filter(|terrain| !terrain.data_path.is_empty())
        .map(|terrain| terrain.data_path.clone())
        .collect();
    // A despawned terrain keeps nothing, so the next terrain at that path
    // starts from a fresh resolve rather than another terrain's slots.
    materials
        .entries
        .retain(|path, _| live.iter().any(|live| live == path));

    for data_path in live {
        let slots = store.materials(&data_path).to_vec();
        let resolved = resolve_with(
            &slots,
            |name| {
                registry
                    .get_by_name(name)
                    .and_then(|entry| standard.get(&entry.handle))
            },
            &assets,
        );
        let entry = materials.entries.entry(data_path).or_default();
        if entry.slots == slots && entry.set == resolved.set && entry.missing == resolved.missing {
            continue;
        }
        // The id space moved under the brush, so a pick past the end of the
        // list is clamped back into range rather than reset, which would
        // throw away the pick on an unrelated slot edit.
        let ceiling = slots.len().saturating_sub(1) as u8;
        paint_state.active_texture_id = paint_state.active_texture_id.min(ceiling);

        entry.slots = slots;
        entry.set = resolved.set;
        entry.images = resolved.images;
        entry.missing = resolved.missing;
        entry.material = None;
        entry.control = None;
        entry.error = None;
        entry.needs_chunk_rebuild = true;
    }
}

/// Record a problem for the Textures tab and log it once, then stay quiet
/// until the entry is cleared.
fn report_once(entry: &mut SplatEntry, message: &str) {
    if entry.error.is_some() {
        return;
    }
    entry.error = Some(message.to_string());
    error!("terrain materials: {message}");
}

/// Build the material for every terrain whose textures have all decoded.
///
/// Runs every frame and does nothing once a material exists, so retrying
/// while images load costs a hash lookup.
fn build_ready_materials(
    mut materials: ResMut<TerrainSplatMaterials>,
    mut splat_materials: ResMut<Assets<TerrainSplatMaterial>>,
    mut images: ResMut<Assets<Image>>,
    assets: Res<AssetServer>,
    store: Res<TerrainDataStore>,
    view: Res<TerrainRegionView>,
    mut terrains: Query<(
        Entity,
        &jackdaw_scene_types::Terrain,
        &mut TerrainDirtyChunks,
    )>,
) {
    for (entity, terrain, mut dirty) in &mut terrains {
        // The control map is sized by the terrain's stored cells and the
        // shader scales UVs by how much ground they cover, so a stroke
        // that allocates a region changes both.
        let shape = store.grid_shape(terrain);
        let Some(entry) = materials.entries.get_mut(&terrain.data_path) else {
            continue;
        };
        if entry.material.is_some()
            && (entry.built_size != shape.size || entry.built_resolution != shape.resolution)
        {
            entry.material = None;
            entry.control = None;
            entry.slope = None;
        }
        if entry.set.is_empty() {
            // Nothing to draw with, so the chunks go back to the plain
            // editor material rather than keep the last arrays.
            if std::mem::take(&mut entry.needs_chunk_rebuild) {
                dirty.rebuild_all = true;
            }
            continue;
        }
        if entry.material.is_some() {
            continue;
        }

        let built = match splat_images(&entry.set, &entry.images, &images) {
            Ok(built) => built,
            Err(SplatBuildError::NotReady) => {
                // Still loading or permanently failed; only the asset
                // server can tell the two apart.
                if let Some(failed) = first_failed(&entry.images, &assets) {
                    report_once(
                        entry,
                        &format!("texture '{failed}' could not be loaded; check the material"),
                    );
                }
                continue;
            }
            Err(other) => {
                report_once(entry, &other.to_string());
                continue;
            }
        };

        let mask = view.control_mask(entity);
        let texels = ControlTexels::from_control(
            &masked_control(
                mask,
                region_side(&store, terrain).unwrap_or(1),
                shape.resolution,
                store.control(&terrain.data_path),
            ),
            shape.resolution,
        )
        .to_bytes();
        let control = images.add(control_image_from_bytes(&texels, shape.resolution));
        let heights = store.heightmap(terrain).map;
        let slope = images.add(slope_image(&heights));
        let arrays = SplatArrayHandles {
            albedo: images.add(built.albedo),
            normal: images.add(built.normal),
            height: images.add(built.height),
        };
        let autoterrain = store.autoterrain(&terrain.data_path);
        let material = splat_materials.add(TerrainSplatMaterial::new(
            &entry.set,
            arrays,
            control.clone(),
            slope.clone(),
            shape.size,
            shape.resolution,
            autoterrain,
        ));
        entry.slope = Some(slope);
        entry.slope_heights = Some(heights);
        entry.control = Some(control);
        entry.control_texels = texels;
        entry.control_mask = mask;
        entry.material = Some(material);
        entry.built_size = shape.size;
        entry.built_resolution = shape.resolution;
        entry.built_autoterrain = autoterrain;
        entry.error = None;
        entry.needs_chunk_rebuild = false;
        // The chunks are spawned with the fallback material; a rebuild is
        // what swaps them onto this one.
        dirty.rebuild_all = true;
    }
}

/// The path of the first texture the asset server has given up on, if any.
fn first_failed(handles: &TextureSetImages, assets: &AssetServer) -> Option<String> {
    handles.handles().find_map(|handle| {
        matches!(
            assets.get_load_state(handle.id()),
            Some(LoadState::Failed(_))
        )
        .then(|| {
            assets
                .get_path(handle.id())
                .map(|path| path.to_string())
                .unwrap_or_else(|| "<unknown>".to_string())
        })
    })
}

/// Follow a terrain's autoterrain settings into the material it draws with.
///
/// A uniform write only: the control map, the texture arrays and the meshes
/// all stand. An entry with no material yet is left alone, so the settings
/// reach it through `build_ready_materials`.
fn refresh_autoterrain(
    mut materials: ResMut<TerrainSplatMaterials>,
    mut splat_materials: ResMut<Assets<TerrainSplatMaterial>>,
    store: Res<TerrainDataStore>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
) {
    for terrain in &terrains {
        let Some(entry) = materials.entries.get_mut(&terrain.data_path) else {
            continue;
        };
        let wanted = store.autoterrain(&terrain.data_path);
        if entry.built_autoterrain == wanted {
            continue;
        }
        let Some(handle) = entry.material.clone() else {
            continue;
        };
        let Some(mut material) = splat_materials.get_mut(&handle) else {
            continue;
        };
        material.set_autoterrain(wanted);
        entry.built_autoterrain = wanted;
    }
}

/// Re-upload a terrain's control map after it has been painted, or after
/// the region view mode changed which regions show their paint.
///
/// Only the rectangle the paint stroke wrote is rebuilt, into the texel
/// buffer this entry keeps between frames; the rect saves the CPU-side
/// rebuild, not the upload. The control texture is `RENDER_WORLD`-only, so
/// the image is replaced rather than edited in place and the renderer sends
/// all of it either way. A mode change has no rect and rebuilds the whole
/// buffer.
///
/// The mask is applied on the way to the texture and nowhere else: the
/// document keeps every control word, so a save or an export writes the
/// same bytes under every mode.
fn refresh_control_maps(
    mut materials: ResMut<TerrainSplatMaterials>,
    store: Res<TerrainDataStore>,
    view: Res<TerrainRegionView>,
    mut images: ResMut<Assets<Image>>,
    terrains: Query<(Entity, &jackdaw_scene_types::Terrain)>,
) {
    for (entity, terrain) in &terrains {
        let shape = store.grid_shape(terrain);
        let Some(entry) = materials.entries.get_mut(&terrain.data_path) else {
            continue;
        };
        // Nowhere to upload to yet; the mark stays set so a write that
        // lands before the material exists is not lost.
        let Some(control) = entry.control.clone() else {
            continue;
        };
        let mask = view.control_mask(entity);
        let expected = (shape.resolution as usize) * (shape.resolution as usize) * 4;
        // The whole grid is rebuilt only when the buffer was built for
        // another resolution, the entry holds none, or the mask moved.
        let whole = entry.control_mask != mask || entry.control_texels.len() != expected;
        let dirty = store.take_control_dirty(&terrain.data_path);
        if !whole && dirty.is_none() {
            continue;
        }
        entry.control_mask = mask;
        let side = region_side(&store, terrain).unwrap_or(1);
        match dirty.filter(|_| !whole) {
            Some(rect) => {
                let mut block = store.control_rect(&terrain.data_path, shape.resolution, rect);
                mask_control_block(mask, side, rect, &mut block);
                write_control_block(&mut entry.control_texels, shape.resolution, rect, &block);
            }
            None => {
                let words = masked_control(
                    mask,
                    side,
                    shape.resolution,
                    store.control(&terrain.data_path),
                );
                entry.control_texels =
                    ControlTexels::from_control(&words, shape.resolution).to_bytes();
            }
        }
        // Fails only if the handle has been dropped, which cannot happen
        // while this entry still holds it.
        if let Err(err) = images.insert(
            control.id(),
            control_image_from_bytes(&entry.control_texels, shape.resolution),
        ) {
            warn!("terrain control map could not be re-uploaded: {err}");
        }
    }
}

/// Re-upload a terrain's slope map after its heights have moved.
///
/// Autoterrain reads the slope of the stored ground rather than of the
/// surface being drawn, so a sculpt stroke has to reach the texture before
/// the ground it raised can be textured as slope. An edited map comes back
/// from the store under a new pointer, because the entry's own held
/// reference blocks the store's patch-in-place path (see
/// [`SplatEntry::slope_heights`]), so pointer identity is the staleness
/// test.
///
/// The map is regathered whole: sculpt strokes are rarer than a paint
/// stroke's per-frame writes, and the mesher rebuilds surfaces from the
/// same map on the same frames.
fn refresh_slope_maps(
    mut materials: ResMut<TerrainSplatMaterials>,
    store: Res<TerrainDataStore>,
    mut images: ResMut<Assets<Image>>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
) {
    for terrain in &terrains {
        let Some(entry) = materials.entries.get_mut(&terrain.data_path) else {
            continue;
        };
        let Some(slope) = entry.slope.clone() else {
            continue;
        };
        let heights = store.heightmap(terrain).map;
        if entry
            .slope_heights
            .as_ref()
            .is_some_and(|built| Arc::ptr_eq(built, &heights))
        {
            continue;
        }
        // Fails only if the handle has been dropped, which cannot happen
        // while this entry still holds it.
        if let Err(err) = images.insert(slope.id(), slope_image(&heights)) {
            warn!("terrain slope map could not be re-uploaded: {err}");
            continue;
        }
        entry.slope_heights = Some(heights);
    }
}

/// Patch `rect` of an uploaded control texture from that rect's own words,
/// row-major over the rect.
///
/// `jackdaw_terrain::write_control_rect` does the same job from a
/// whole-grid slice, which a stroke on a multi-region terrain cannot afford
/// to gather. Both write the same little-endian raw word.
fn write_control_block(bytes: &mut [u8], resolution: u32, rect: GridRect, block: &[Control]) {
    let width = rect.width.max(1) as usize;
    for (line, row) in rect.rows(resolution).enumerate() {
        for (column, index) in row.enumerate() {
            let at = index * 4;
            if at + 4 > bytes.len() {
                break;
            }
            let word = block
                .get(line * width + column)
                .copied()
                .unwrap_or_default()
                .to_raw();
            bytes[at..at + 4].copy_from_slice(&word.to_le_bytes());
        }
    }
}

/// Drop what a terrain's materials produced when one of their textures
/// changes on disk.
///
/// The editor build runs the asset file watcher, so editing one of a
/// material's PNGs raises `Modified` here and the following frames rebuild
/// from the new bytes. A change to the material file itself needs nothing
/// here: `resolve_terrain_materials` re-reads the live `StandardMaterial`
/// every frame.
fn invalidate_on_asset_change(
    mut materials: ResMut<TerrainSplatMaterials>,
    mut image_events: MessageReader<AssetEvent<Image>>,
) {
    let changed: Vec<_> = image_events
        .read()
        .filter_map(|event| match event {
            AssetEvent::Modified { id } => Some(*id),
            _ => None,
        })
        .collect();
    if changed.is_empty() {
        return;
    }

    for entry in materials.entries.values_mut() {
        if entry.images.handles().any(|h| changed.contains(&h.id())) {
            entry.material = None;
            entry.control = None;
            entry.error = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;

    fn splat_app() -> App {
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_asset::<Image>();
        app.init_asset::<StandardMaterial>();
        app
    }

    /// Removing a terrain's last material reaches the chunks: they pick
    /// their material up at mesh-build time, so a dropped handle alone
    /// would leave them drawing arrays that are gone.
    #[test]
    fn losing_every_material_asks_the_chunks_to_rebuild_once() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = splat_app();
        app.init_asset::<TerrainSplatMaterial>();
        app.init_resource::<TerrainSplatMaterials>();
        app.init_resource::<TerrainDataStore>();
        app.init_resource::<TerrainRegionView>();
        let entity = app
            .world_mut()
            .spawn((
                jackdaw_scene_types::Terrain {
                    data_path: "a.jdterrain".to_string(),
                    ..default()
                },
                TerrainDirtyChunks::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<TerrainSplatMaterials>()
            .entries
            .insert(
                "a.jdterrain".to_string(),
                SplatEntry {
                    needs_chunk_rebuild: true,
                    ..default()
                },
            );

        let rebuild_all = |app: &App| {
            app.world()
                .get::<TerrainDirtyChunks>(entity)
                .unwrap()
                .rebuild_all
        };
        app.world_mut()
            .run_system_once(build_ready_materials)
            .expect("system runs");
        assert!(rebuild_all(&app), "the chunks must be asked to rebuild");

        app.world_mut()
            .get_mut::<TerrainDirtyChunks>(entity)
            .unwrap()
            .rebuild_all = false;
        app.world_mut()
            .run_system_once(build_ready_materials)
            .expect("system runs");
        assert!(
            !rebuild_all(&app),
            "the request is consumed, not re-raised every frame",
        );
    }

    /// A sculpt lands in the store and the next refresh pass re-uploads.
    /// The staleness test is pointer identity, which holds only because the
    /// entry's held map blocks the store's patch-in-place path.
    #[test]
    fn a_sculpt_reaches_the_slope_map_and_quiet_ground_does_not() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = splat_app();
        app.init_resource::<TerrainSplatMaterials>();
        app.init_resource::<TerrainDataStore>();
        let terrain = jackdaw_scene_types::Terrain {
            data_path: "a.jdterrain".to_string(),
            ..default()
        };
        let mut data = jackdaw_terrain::RegionTerrainData::default();
        data.regions
            .ensure_grid(65)
            .expect("a small grid fits the cap");
        app.world_mut()
            .resource_mut::<TerrainDataStore>()
            .insert("a.jdterrain", data);
        app.world_mut().spawn(terrain.clone());

        let heights = app
            .world()
            .resource::<TerrainDataStore>()
            .heightmap(&terrain)
            .map;
        let slope = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(slope_image(&heights));
        app.world_mut()
            .resource_mut::<TerrainSplatMaterials>()
            .entries
            .insert(
                "a.jdterrain".to_string(),
                SplatEntry {
                    slope: Some(slope.clone()),
                    slope_heights: Some(heights),
                    ..default()
                },
            );
        let texels = |app: &App| {
            app.world()
                .resource::<Assets<Image>>()
                .get(slope.id())
                .expect("the slope image lives")
                .data
                .clone()
        };
        let flat = texels(&app);

        app.world_mut()
            .run_system_once(refresh_slope_maps)
            .expect("system runs");
        assert_eq!(texels(&app), flat, "unsculpted ground re-uploads nothing");

        let resolution = app
            .world()
            .resource::<TerrainDataStore>()
            .grid_resolution("a.jdterrain");
        app.world_mut()
            .resource_mut::<TerrainDataStore>()
            .brush_heights(
                &terrain,
                jackdaw_terrain::GridRect::whole(resolution),
                |heights| heights[32 * resolution as usize + 32] += 8.0,
            );
        app.world_mut()
            .run_system_once(refresh_slope_maps)
            .expect("system runs");
        assert_ne!(
            texels(&app),
            flat,
            "the raised cell must reach the uploaded slopes"
        );
    }

    #[test]
    fn a_terrain_with_no_entry_has_no_material_to_render_with() {
        let materials = TerrainSplatMaterials::default();
        assert!(materials.material("a.jdterrain").is_none());
        assert!(materials.missing("a.jdterrain").is_empty());
        assert!(materials.albedo_thumbnails("a.jdterrain").is_empty());
    }
}
