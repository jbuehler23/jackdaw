//! Copy a picked glTF/glb into the project when it is not already under `assets/`.
//! glTF's bring along associated .bin and texture files (that's most of what this file is doing).

use std::path::{Component, Path, PathBuf};

use crate::model_thumbnail::is_model_path;

pub(crate) const PREVIEW_MODELS_DIR: &str = "jackdaw_preview_models";

#[derive(Debug)]
pub(crate) enum PreviewImportError {
    Io(std::io::Error),
    NotAModel,
    InvalidGltf(String),
    MissingSidecar(PathBuf),
    EscapingUri(String),
}

impl std::fmt::Display for PreviewImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::NotAModel => write!(f, "preview model must be a .glb or .gltf file"),
            Self::InvalidGltf(reason) => write!(f, "gltf could not be read: {reason}"),
            Self::MissingSidecar(path) => {
                write!(f, "gltf is missing {}", path.display())
            }
            Self::EscapingUri(uri) => {
                write!(f, "gltf uri leaves the model folder: {uri}")
            }
        }
    }
}

impl From<std::io::Error> for PreviewImportError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// Return an assets-relative glTF path for `picked`.
///
/// Files already under `assets_dir` keep their relative path. Anything else
/// is copied into `assets/jackdaw_preview_models/`. A `.gltf` brings along
/// every local buffer and image uri; missing sidecars are rejected.
pub(crate) fn import_preview_model(
    assets_dir: &Path,
    picked: &Path,
) -> Result<String, PreviewImportError> {
    if !is_model_path(picked) {
        return Err(PreviewImportError::NotAModel);
    }
    if let Some(relative) = relative_to_assets(assets_dir, picked) {
        return Ok(relative);
    }
    let dest_root = assets_dir.join(PREVIEW_MODELS_DIR);
    std::fs::create_dir_all(&dest_root)?;
    if is_gltf(picked) {
        copy_gltf_bundle(&dest_root, picked)
    } else {
        copy_glb(&dest_root, picked)
    }
    .and_then(|path| asset_relative(assets_dir, &path))
}

fn copy_glb(dest_root: &Path, source: &Path) -> Result<PathBuf, PreviewImportError> {
    let stem = file_stem(source);
    let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("glb");
    let dest = unique_child(dest_root, stem, Some(ext));
    std::fs::copy(source, &dest)?;
    Ok(dest)
}

fn copy_gltf_bundle(dest_root: &Path, source: &Path) -> Result<PathBuf, PreviewImportError> {
    let sidecars = gltf_local_uris(source)?;
    let Some(file_name) = source.file_name() else {
        return Err(PreviewImportError::NotAModel);
    };
    let dest_dir = unique_child(dest_root, file_stem(source), None);
    std::fs::create_dir_all(&dest_dir)?;
    let dest_gltf = dest_dir.join(file_name);
    let copy_result = (|| -> Result<PathBuf, PreviewImportError> {
        std::fs::copy(source, &dest_gltf)?;
        let Some(src_dir) = source.parent() else {
            return Err(PreviewImportError::NotAModel);
        };
        for rel in &sidecars {
            let from = src_dir.join(rel);
            let to = dest_dir.join(rel);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&from, &to)?;
        }
        Ok(dest_gltf.clone())
    })();
    if copy_result.is_err() {
        let _ = std::fs::remove_dir_all(&dest_dir);
    }
    copy_result
}

fn gltf_local_uris(gltf_path: &Path) -> Result<Vec<PathBuf>, PreviewImportError> {
    let text = std::fs::read_to_string(gltf_path)?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|err| PreviewImportError::InvalidGltf(err.to_string()))?;
    let Some(src_dir) = gltf_path.parent() else {
        return Err(PreviewImportError::NotAModel);
    };
    let mut files = Vec::new();
    for key in ["buffers", "images"] {
        let Some(items) = value.get(key).and_then(|v| v.as_array()) else {
            continue;
        };
        for item in items {
            let Some(uri) = item.get("uri").and_then(|v| v.as_str()) else {
                continue;
            };
            if is_embedded_or_remote(uri) {
                continue;
            }
            let rel = Path::new(uri);
            if !is_local_relative(rel) {
                return Err(PreviewImportError::EscapingUri(uri.to_string()));
            }
            let abs = src_dir.join(rel);
            if !abs.is_file() {
                return Err(PreviewImportError::MissingSidecar(rel.to_path_buf()));
            }
            files.push(rel.to_path_buf());
        }
    }
    Ok(files)
}

fn is_embedded_or_remote(uri: &str) -> bool {
    uri.starts_with("data:") || uri.starts_with("http:") || uri.starts_with("https:")
}

fn is_local_relative(path: &Path) -> bool {
    !path.is_absolute()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

fn is_gltf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("gltf"))
}

fn file_stem(path: &Path) -> &str {
    path.file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("model")
}

fn unique_child(parent: &Path, stem: &str, ext: Option<&str>) -> PathBuf {
    let name = |n: &str| match ext {
        Some(e) => format!("{n}.{e}"),
        None => n.to_string(),
    };
    let first = parent.join(name(stem));
    if !first.exists() {
        return first;
    }
    let mut n = 1;
    loop {
        let candidate = parent.join(name(&format!("{stem}-{n}")));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

fn relative_to_assets(assets_dir: &Path, path: &Path) -> Option<String> {
    let path = dunce::simplified(path);
    let assets = dunce::simplified(assets_dir);
    path.strip_prefix(assets).ok().and_then(|rel| {
        if rel.as_os_str().is_empty() {
            None
        } else {
            Some(rel.to_string_lossy().replace('\\', "/"))
        }
    })
}

fn asset_relative(assets_dir: &Path, path: &Path) -> Result<String, PreviewImportError> {
    relative_to_assets(assets_dir, path).ok_or_else(|| {
        PreviewImportError::Io(std::io::Error::other(
            "copied preview model is not under assets/",
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(path, contents).expect("write file");
    }

    fn sibling_gltf(dir: &Path, name: &str, bin: &str) -> PathBuf {
        let gltf = dir.join(format!("{name}.gltf"));
        write(
            &gltf,
            &format!(
                r#"{{"asset":{{"version":"2.0"}},"buffers":[{{"uri":"{bin}","byteLength":1}}]}}"#
            ),
        );
        write(&dir.join(bin), "x");
        gltf
    }

    #[test]
    fn in_assets_keeps_relative_path() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let model = assets.join("core").join("flag.glb");
        write(&model, "glb");
        let path = import_preview_model(&assets, &model).expect("import");
        assert_eq!(path, "core/flag.glb");
        assert!(!assets.join(PREVIEW_MODELS_DIR).exists());
    }

    #[test]
    fn outside_glb_copies_into_preview_folder() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        std::fs::create_dir_all(&assets).expect("assets");
        let source = tmp.path().join("Downloads").join("rifle.glb");
        write(&source, "glb");
        let path = import_preview_model(&assets, &source).expect("import");
        assert_eq!(path, "jackdaw_preview_models/rifle.glb");
        assert!(assets.join(PREVIEW_MODELS_DIR).join("rifle.glb").exists());
    }

    #[test]
    fn glb_name_collision_gets_a_suffix() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        let dest = assets.join(PREVIEW_MODELS_DIR);
        std::fs::create_dir_all(&dest).expect("dest");
        write(&dest.join("rifle.glb"), "old");
        let source = tmp.path().join("rifle.glb");
        write(&source, "new");
        let path = import_preview_model(&assets, &source).expect("import");
        assert_eq!(path, "jackdaw_preview_models/rifle-1.glb");
    }

    #[test]
    fn gltf_copies_bin_into_unique_folder() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        std::fs::create_dir_all(&assets).expect("assets");
        let src_dir = tmp.path().join("kit");
        let gltf = sibling_gltf(&src_dir, "monke", "monke.bin");
        let path = import_preview_model(&assets, &gltf).expect("import");
        assert_eq!(path, "jackdaw_preview_models/monke/monke.gltf");
        let dest = assets.join(PREVIEW_MODELS_DIR).join("monke");
        assert!(dest.join("monke.gltf").exists());
        assert!(dest.join("monke.bin").exists());
    }

    #[test]
    fn gltf_without_bin_is_rejected() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        std::fs::create_dir_all(&assets).expect("assets");
        let gltf = tmp.path().join("broken.gltf");
        write(
            &gltf,
            r#"{"asset":{"version":"2.0"},"buffers":[{"uri":"missing.bin","byteLength":1}]}"#,
        );
        let err = import_preview_model(&assets, &gltf).expect_err("missing bin");
        assert!(matches!(err, PreviewImportError::MissingSidecar(_)));
        assert!(!assets.join(PREVIEW_MODELS_DIR).join("broken").exists());
    }

    #[test]
    fn embedded_gltf_copies_without_a_bin() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        std::fs::create_dir_all(&assets).expect("assets");
        let gltf = tmp.path().join("embedded.gltf");
        write(
            &gltf,
            r#"{"asset":{"version":"2.0"},"buffers":[{"uri":"data:application/octet-stream;base64,AA==","byteLength":1}]}"#,
        );
        let path = import_preview_model(&assets, &gltf).expect("import");
        assert_eq!(path, "jackdaw_preview_models/embedded/embedded.gltf");
        assert!(
            assets
                .join(PREVIEW_MODELS_DIR)
                .join("embedded")
                .join("embedded.gltf")
                .exists()
        );
    }

    #[test]
    fn gltf_copies_nested_image_uris() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        std::fs::create_dir_all(&assets).expect("assets");
        let src_dir = tmp.path().join("kit");
        let gltf = src_dir.join("prop.gltf");
        write(
            &gltf,
            r#"{"asset":{"version":"2.0"},"buffers":[{"uri":"prop.bin","byteLength":1}],"images":[{"uri":"textures/albedo.png"}]}"#,
        );
        write(&src_dir.join("prop.bin"), "b");
        write(&src_dir.join("textures").join("albedo.png"), "png");
        let path = import_preview_model(&assets, &gltf).expect("import");
        assert_eq!(path, "jackdaw_preview_models/prop/prop.gltf");
        let dest = assets.join(PREVIEW_MODELS_DIR).join("prop");
        assert!(dest.join("prop.bin").exists());
        assert!(dest.join("textures").join("albedo.png").exists());
    }

    #[test]
    fn gltf_parent_dir_uri_is_rejected() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let assets = tmp.path().join("assets");
        std::fs::create_dir_all(&assets).expect("assets");
        let src_dir = tmp.path().join("kit").join("inner");
        write(&tmp.path().join("kit").join("secret.bin"), "no");
        let gltf = src_dir.join("evil.gltf");
        write(
            &gltf,
            r#"{"asset":{"version":"2.0"},"buffers":[{"uri":"../secret.bin","byteLength":1}]}"#,
        );
        let err = import_preview_model(&assets, &gltf).expect_err("escape");
        assert!(matches!(err, PreviewImportError::EscapingUri(_)));
    }
}
