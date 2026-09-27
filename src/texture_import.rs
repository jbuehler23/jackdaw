//! Texture import settings: writing the Bevy `.meta` that has a texture
//! block compressed with mipmaps, and reloading what uses it.
//!
//! The meta names Bevy's own processor, so only an app built on
//! `jackdaw_runtime`, or one running Bevy's asset processor, loads a texture
//! that carries it; any other Bevy app refuses the file.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_runtime::texture_import::{read_texture_import_meta, texture_import_meta};

/// Where processed textures are kept for a project.
pub fn cache_dir(project_root: &Path) -> PathBuf {
    project_root.join(".jackdaw").join("imported")
}

/// The `.meta` file beside a texture.
pub fn meta_path(texture: &Path) -> PathBuf {
    let mut name = texture.as_os_str().to_owned();
    name.push(".meta");
    PathBuf::from(name)
}

/// Whether a texture has an import meta.
pub fn is_imported(texture: &Path) -> bool {
    std::fs::read(meta_path(texture))
        .ok()
        .is_some_and(|bytes| read_texture_import_meta(&bytes).is_some())
}

/// Whether a texture's file name says it holds colour, read as sRGB, rather
/// than data such as a normal map, roughness, occlusion or a packed mask.
pub fn is_colour(path: &Path) -> bool {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let parts: Vec<&str> = stem
        .rsplit(['_', '-', '.', ' '])
        .filter(|part| !is_resolution_tag(part))
        .take(2)
        .collect();
    let last = parts.first().copied().unwrap_or_default();
    let pair = parts
        .get(1)
        .map(|before| format!("{before}_{last}"))
        .unwrap_or_default();
    if let Some(role) =
        jackdaw_material::classify_tag(&pair).or_else(|| jackdaw_material::classify_tag(last))
    {
        return role.is_srgb();
    }
    const PACKED_MASK_TAGS: [&str; 7] = ["orm", "mask", "m", "mra", "arm", "rma", "h"];
    !PACKED_MASK_TAGS.contains(&last)
}

/// Whether a file name part is a resolution such as `1k` or `2048`.
fn is_resolution_tag(part: &str) -> bool {
    let digits = part.strip_suffix('k').unwrap_or(part);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// Write the import meta for one texture, or remove it, reporting whether the
/// file changed.
pub fn write_import_meta(texture: &Path, imported: bool) -> std::io::Result<bool> {
    let meta = meta_path(texture);
    if !imported {
        if !is_imported(texture) {
            return Ok(false);
        }
        std::fs::remove_file(meta)?;
        return Ok(true);
    }
    let bytes = texture_import_meta(is_colour(texture));
    if std::fs::read(&meta).is_ok_and(|held| held == bytes) {
        return Ok(false);
    }
    std::fs::write(meta, bytes)?;
    Ok(true)
}

/// Whether an import can process the file at `path`.
pub fn is_importable(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| IMPORTED_EXTENSIONS.contains(&extension.to_lowercase().as_str()))
}

/// The textures a path names: the file itself, or every texture under a
/// folder.
fn textures_under(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_path_buf()];
    }
    jackdaw_bsn::walk_files_with_extensions(path, &IMPORTED_EXTENSIONS)
}

/// The image files an import can process.
const IMPORTED_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "tga", "webp"];

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TextureImportOp>();
}

/// Have a texture, or every texture under a folder, block compressed with
/// mipmaps by Bevy's asset processor, or load it as the original file again.
///
/// The import is a Bevy `.meta` beside the texture. Only an app built on
/// `jackdaw_runtime`, or one running Bevy's asset processor, loads a texture
/// that has it. A texture a terrain stacks into its layers is left as it is,
/// because the layers are built from uncompressed texels.
#[operator(
    id = "texture.import",
    label = "Import Texture",
    description = "Block compress a texture, or every texture under a folder, with mipmaps. \
                   Only apps built on jackdaw_runtime, or running Bevy's asset processor, \
                   can load a texture imported this way.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Texture file or folder, as a path under the project's assets."
        ),
        compression(
            String,
            default = "vram_compressed",
            doc = "\"vram_compressed\", or \"none\" to load the original file again."
        ),
    )
)]
pub(crate) fn texture_import(
    params: In<OperatorParameters>,
    project: Option<Res<crate::project::ProjectRoot>>,
    asset_server: Res<AssetServer>,
    terrains: Option<Res<crate::terrain::splat::TerrainSplatMaterials>>,
    preview: Option<ResMut<crate::texture_files::AssetPreviewState>>,
) -> OperatorResult {
    let Some(project) = project else {
        return OperatorResult::Cancelled;
    };
    let assets = project.assets_dir();
    let Some(named) = params.as_str("path") else {
        warn!("texture.import: no path given");
        return OperatorResult::Cancelled;
    };
    let imported = match params.as_str("compression").unwrap_or("vram_compressed") {
        "vram_compressed" => true,
        "none" => false,
        other => {
            warn!("texture.import: unknown compression `{other}`");
            return OperatorResult::Cancelled;
        }
    };
    let target = assets.join(named);
    if !target.exists() {
        warn!("texture.import: nothing at {named}");
        return OperatorResult::Cancelled;
    }
    let stacked: Vec<PathBuf> = terrains
        .iter()
        .flat_map(|terrains| terrains.texture_paths())
        .map(|path| assets.join(path))
        .collect();
    for texture in textures_under(&target) {
        if imported && stacked.contains(&texture) {
            continue;
        }
        match write_import_meta(&texture, imported) {
            Ok(true) => {
                if let Ok(relative) = texture.strip_prefix(&assets) {
                    asset_server.reload(relative.to_path_buf());
                }
            }
            Ok(false) => {}
            Err(err) => warn!("texture.import: {}: {err}", texture.display()),
        }
    }
    if let Some(mut preview) = preview {
        preview.set_changed();
    }
    OperatorResult::Finished
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_name_suffix_says_whether_the_texture_is_colour() {
        for (file, colour) in [
            ("Cliff1_a.png", true),
            ("rock_albedo_2k.png", true),
            ("Cliff1_n.png", false),
            ("grass_normal_gl_1k.png", false),
            ("Cliff1_mask_orm.png", false),
            ("Grass01_m.png", false),
            ("ground_roughness_1k.png", false),
            ("moss_ao.png", false),
        ] {
            assert_eq!(is_colour(Path::new(file)), colour, "{file}");
        }
    }

    #[test]
    fn importing_writes_a_meta_that_reads_back_and_clearing_removes_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let texture = dir.path().join("Cliff1_n.png");
        std::fs::write(&texture, b"png").expect("write");

        assert!(write_import_meta(&texture, true).expect("write meta"));
        assert!(is_imported(&texture));
        let meta = std::fs::read(meta_path(&texture)).expect("meta");
        let loader = read_texture_import_meta(&meta).expect("an import meta");
        assert!(!loader.is_srgb, "a normal map imports as linear data");
        assert!(!write_import_meta(&texture, true).expect("rewrite"));

        assert!(write_import_meta(&texture, false).expect("clear"));
        assert!(!meta_path(&texture).exists());
    }
}
