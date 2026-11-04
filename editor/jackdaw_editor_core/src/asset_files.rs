//! What a file on disk is: a scene, a prefab, or an asset of some kind.
//!
//! An asset file carries a header naming the type it holds, written after the
//! version stamp. The header is a hint a file listing can read without parsing;
//! where the editor has something to open that type as, the document's own root
//! is the truth, so a header that disagrees with it is reported and ignored. A
//! header naming any other type stands for the file, since the parse could only
//! confirm a type nothing acts on. A file with no header at all is still known
//! by its root, which is how everything written before headers existed keeps
//! working.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bevy::prelude::*;
use jackdaw_api::prelude::AssetKinds;
use jackdaw_prefab::components::PREFAB_TYPE;

/// What a file holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetFileKind {
    Scene,
    Prefab,
    Asset { type_path: String },
}

impl AssetFileKind {
    /// The type an asset file holds, if it is one.
    pub fn type_path(&self) -> Option<&str> {
        match self {
            Self::Asset { type_path } => Some(type_path),
            _ => None,
        }
    }
}

/// The text an asset file is written as: the version stamp, the header naming
/// its type, and the document.
pub fn asset_file_text(type_path: &str, body: &str) -> String {
    crate::scene_io::stamp::with_stamp(&jackdaw_bsn::with_asset_header(type_path, body))
}

/// What the file at `path` holds, read from its first root and from its header
/// only where the document names no type of its own.
pub fn read_asset_kind(path: &Path, kinds: &AssetKinds) -> AssetFileKind {
    kind_of_type(read_file_type(path).as_deref(), kinds)
}

/// The type a file names as the one it holds: its first root, with a prefab
/// naming the marker its roots carry, and its header for a document that names
/// nothing. `None` when neither names a type.
pub fn read_file_type(path: &Path) -> Option<String> {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jsn"))
    {
        return jsn_type(path);
    }
    jackdaw_bsn::asset_file_type(path)
}

/// What a type path means to the editor: the marker a prefab carries, a kind
/// it knows, or nothing it can open.
pub(crate) fn kind_of_type(type_path: Option<&str>, kinds: &AssetKinds) -> AssetFileKind {
    let Some(type_path) = type_path else {
        return AssetFileKind::Scene;
    };
    if type_path == PREFAB_TYPE {
        return AssetFileKind::Prefab;
    }
    if kinds.by_type_path(type_path).is_some() {
        return AssetFileKind::Asset {
            type_path: type_path.to_string(),
        };
    }
    AssetFileKind::Scene
}

/// A legacy `.jsn` document is a prefab when its first scene entity carries
/// the `Prefab` component. Parsed as plain JSON.
fn jsn_type(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    value
        .get("scene")
        .and_then(|scene| scene.get(0))
        .and_then(|entity| entity.get("components"))
        .and_then(|components| components.get(PREFAB_TYPE))
        .map(|_| PREFAB_TYPE.to_string())
}

/// Whether a file is a prefab, which is the one question the browser's grid
/// asks often enough to memo.
pub fn is_prefab(path: &Path, kinds: &AssetKinds) -> bool {
    read_asset_kind(path, kinds) == AssetFileKind::Prefab
}

/// What one read of a file found: the type it names and the references its
/// values spell.
#[derive(Clone, Default)]
struct FileFacts {
    type_path: Option<String>,
    references: Vec<String>,
}

/// Whether a quoted value could name another file: a `@name`, or a path whose
/// last segment carries a plausible extension.
fn could_name_a_file(value: &str) -> bool {
    if value.is_empty() || value.contains(['{', '}', '\n']) {
        return false;
    }
    if value.starts_with('@') {
        return true;
    }
    Path::new(value)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.len() <= 8
                && extension.starts_with(|first: char| first.is_ascii_alphabetic())
                && extension.chars().all(|part| part.is_ascii_alphanumeric())
        })
}

/// The file a value names, without the label a reference into a model or a
/// scene carries: `models/town.glb#Scene0` names `models/town.glb`.
fn without_label(value: &str) -> &str {
    match value.starts_with('@') {
        true => value,
        false => value.split('#').next().unwrap_or(value),
    }
}

/// The references a document's values spell, in the order it spells them.
fn spelled_references(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '"' {
            continue;
        }
        let mut value = String::new();
        let mut escaped = false;
        for inside in characters.by_ref() {
            if escaped {
                value.push(inside);
                escaped = false;
                continue;
            }
            match inside {
                '\\' => escaped = true,
                '"' => break,
                _ => value.push(inside),
            }
        }
        let named = without_label(&value);
        if could_name_a_file(named) && !found.iter().any(|held| held == named) {
            found.push(named.to_string());
        }
    }
    found
}

/// What one read of a file finds, for the memo below.
///
/// `opened` names the types the editor has something to open a file as. A
/// header naming any other type is taken at its word, since parsing the
/// document would only confirm a type nothing acts on.
fn read_file_facts(path: &Path, opened: Option<&HashSet<String>>) -> FileFacts {
    if !jackdaw_bsn::is_document_path(path) {
        return FileFacts {
            type_path: read_file_type(path),
            references: Vec::new(),
        };
    }
    let Ok(text) = jackdaw_bsn::read_document_text(path) else {
        return FileFacts::default();
    };
    let header = jackdaw_bsn::read_asset_header(&text);
    let trusted = header.filter(|header| opened.is_some_and(|opened| !opened.contains(header)));
    FileFacts {
        type_path: trusted.or_else(|| jackdaw_bsn::asset_text_type(&text, path)),
        references: spelled_references(&text),
    }
}

/// Per-path memo of what each file says: the type it names and the references
/// it spells. Keyed by path and invalidated when the file's mtime changes or
/// the file goes. What that type means is resolved on every call, so a kind
/// registered later is seen without rereading anything, except when the set of
/// types the editor opens changes, which is what makes a trusted header worth
/// reading past.
#[derive(Resource, Default)]
pub struct AssetKindCache {
    entries: HashMap<PathBuf, (SystemTime, FileFacts)>,
    opened: Option<HashSet<String>>,
}

impl AssetKindCache {
    pub fn check(&mut self, path: &Path, kinds: &AssetKinds) -> AssetFileKind {
        kind_of_type(self.type_of(path).as_deref(), kinds)
    }

    /// Name the types the editor has something to open a file as, so a file
    /// holding anything else costs a read rather than a read and a parse.
    pub(crate) fn follow(&mut self, kinds: &AssetKinds) {
        let opened: HashSet<String> = kinds
            .iter()
            .map(|kind| kind.type_path.clone())
            .chain([PREFAB_TYPE.to_string()])
            .collect();
        if self.opened.as_ref() == Some(&opened) {
            return;
        }
        self.opened = Some(opened);
        self.entries.clear();
    }

    /// The type the file at `path` names, from the memo where the file has
    /// not changed since it was last read.
    pub(crate) fn type_of(&mut self, path: &Path) -> Option<String> {
        self.facts(path).and_then(|facts| facts.type_path)
    }

    /// The references the file at `path` spells, from the same memo.
    pub(crate) fn references_of(&mut self, path: &Path) -> Vec<String> {
        self.facts(path)
            .map(|facts| facts.references)
            .unwrap_or_default()
    }

    fn facts(&mut self, path: &Path) -> Option<FileFacts> {
        let Ok(mtime) = std::fs::metadata(path).and_then(|meta| meta.modified()) else {
            self.entries.remove(path);
            return None;
        };
        if let Some((cached_mtime, cached)) = self.entries.get(path)
            && *cached_mtime == mtime
        {
            return Some(cached.clone());
        }
        let facts = read_file_facts(path, self.opened.as_ref());
        self.entries
            .insert(path.to_path_buf(), (mtime, facts.clone()));
        Some(facts)
    }
}

pub use jackdaw_bsn::walk_document_files;

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_api::prelude::AssetKind;

    const ITEM_TYPE: &str = "my_game::content::ItemDef";

    fn kinds() -> AssetKinds {
        let mut kinds = AssetKinds::default();
        kinds.register(AssetKind::extension("item", "Item", ITEM_TYPE));
        kinds
    }

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, text).expect("the file is written");
        path
    }

    #[test]
    fn a_saved_asset_carries_its_type_in_its_header() {
        let text = asset_file_text(ITEM_TYPE, "#torch\nmy_game::content::ItemDef { }\n");
        assert!(text.starts_with("// jackdaw "), "got:\n{text}");
        assert_eq!(
            jackdaw_bsn::read_asset_header(&text).as_deref(),
            Some(ITEM_TYPE)
        );
        assert!(
            crate::scene_io::stamp::read_stamp(&text).is_some(),
            "the version stamp still leads the file"
        );
    }

    #[test]
    fn a_file_without_a_header_is_known_by_its_first_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(
            tmp.path(),
            "torch.bsn",
            "#torch\nmy_game::content::ItemDef { stack_size: 4 }\n",
        );
        assert_eq!(
            read_asset_kind(&path, &kinds()),
            AssetFileKind::Asset {
                type_path: ITEM_TYPE.to_string()
            }
        );
    }

    #[test]
    fn a_scene_is_never_taken_for_an_asset() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(
            tmp.path(),
            "level.bsn",
            "#Cube\njackdaw_scene_types::types::Brush { }\n",
        );
        assert_eq!(read_asset_kind(&path, &kinds()), AssetFileKind::Scene);
    }

    #[test]
    fn a_scene_whose_root_holds_an_asset_type_is_still_a_scene() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(
            tmp.path(),
            "level.bsn",
            "#Spawner\n\
             my_game::content::ItemDef { }\n\
             bevy_ecs::hierarchy::Children [\n\
             #Child\n\
             bevy_transform::components::transform::Transform\n\
             ]\n",
        );
        assert_eq!(
            read_asset_kind(&path, &kinds()),
            AssetFileKind::Scene,
            "a root with a hierarchy under it spawns entities, whatever it holds"
        );
    }

    #[test]
    fn a_header_naming_a_prefab_defers_to_the_root_that_holds_an_asset() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(
            tmp.path(),
            "confused.bsn",
            &asset_file_text(PREFAB_TYPE, "#torch\nmy_game::content::ItemDef { }\n"),
        );
        assert_eq!(
            read_asset_kind(&path, &kinds()),
            AssetFileKind::Asset {
                type_path: ITEM_TYPE.to_string()
            }
        );
    }

    #[test]
    fn a_prefab_marker_on_the_root_outranks_a_header_naming_an_asset() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(
            tmp.path(),
            "Cube1.bsn",
            &asset_file_text(
                ITEM_TYPE,
                "jackdaw::prefab::components::Prefab\n\
                 #Cube1\n\
                 bevy_camera::visibility::Visibility::Inherited\n",
            ),
        );
        assert_eq!(read_asset_kind(&path, &kinds()), AssetFileKind::Prefab);
    }

    #[test]
    fn a_header_that_disagrees_with_the_root_defers_to_the_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(
            tmp.path(),
            "confused.bsn",
            &asset_file_text(
                "my_game::content::OutfitDef",
                "#torch\nmy_game::content::ItemDef { }\n",
            ),
        );
        assert_eq!(
            read_asset_kind(&path, &kinds()),
            AssetFileKind::Asset {
                type_path: ITEM_TYPE.to_string()
            }
        );
    }

    #[test]
    fn a_comment_below_the_document_is_not_read_as_a_header() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(
            tmp.path(),
            "level.bsn",
            "#Cube\njackdaw_scene_types::types::Brush { }\n// jackdaw asset my_game::content::ItemDef\n",
        );
        assert_eq!(read_asset_kind(&path, &kinds()), AssetFileKind::Scene);
    }

    #[test]
    fn the_kind_cache_rereads_a_file_whose_mtime_changed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(
            tmp.path(),
            "torch.bsn",
            "#Cube\njackdaw_scene_types::types::Brush { }\n",
        );
        let kinds = kinds();
        let mut cache = AssetKindCache::default();
        assert_eq!(cache.check(&path, &kinds), AssetFileKind::Scene);

        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(
            &path,
            asset_file_text(ITEM_TYPE, "#torch\nmy_game::content::ItemDef { }\n"),
        )
        .expect("the file is rewritten");

        assert_eq!(
            cache.check(&path, &kinds),
            AssetFileKind::Asset {
                type_path: ITEM_TYPE.to_string()
            },
            "the memo follows the file's mtime"
        );
    }

    #[test]
    fn the_kind_cache_forgets_a_file_that_has_gone() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let kinds = kinds();
        let path = write(
            tmp.path(),
            "torch.bsn",
            &asset_file_text(ITEM_TYPE, "#torch\nmy_game::content::ItemDef { }\n"),
        );
        let mut cache = AssetKindCache::default();
        assert!(matches!(
            cache.check(&path, &kinds),
            AssetFileKind::Asset { .. }
        ));

        std::fs::remove_file(&path).expect("the file is removed");

        assert_eq!(
            cache.check(&path, &kinds),
            AssetFileKind::Scene,
            "a file that has gone says nothing about what it held"
        );
        assert!(
            cache.entries.is_empty(),
            "and the memo does not hold on to it"
        );
    }

    #[test]
    fn a_prefab_is_known_by_the_marker_on_one_of_its_roots() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(
            tmp.path(),
            "Cube1.bsn",
            "jackdaw::prefab::components::Prefab\n\
             jackdaw::prefab::components::PrefabEntityId(0)\n\
             #Cube1\n\
             bevy_camera::visibility::Visibility::Inherited\n",
        );
        assert!(is_prefab(&path, &kinds()));

        let plain = write(
            tmp.path(),
            "scene.bsn",
            "#Root\n\
             bevy_transform::components::transform::Transform\n",
        );
        assert!(
            !is_prefab(&plain, &kinds()),
            "a hand-authored scene is not a prefab"
        );

        let named = write(
            tmp.path(),
            "named.bsn",
            "#\"jackdaw::prefab::components::Prefab\"\n\
             bevy_camera::visibility::Visibility::Inherited\n",
        );
        assert!(
            !is_prefab(&named, &kinds()),
            "the marker as a name is not the marker as a component"
        );

        let broken = write(
            tmp.path(),
            "broken.bsn",
            "jackdaw::prefab::components::Prefab {{{{",
        );
        assert!(
            !is_prefab(&broken, &kinds()),
            "a parse failure is not a prefab"
        );
    }

    #[test]
    fn a_legacy_prefab_document_is_still_known_by_its_first_entity() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let prefab = write(
            tmp.path(),
            "p.jsn",
            r#"{
                "scene": [{
                    "components": {
                        "jackdaw::prefab::components::Prefab": null,
                        "bevy_ecs::name::Name": "p"
                    }
                }]
            }"#,
        );
        assert!(is_prefab(&prefab, &kinds()));

        let scene = write(
            tmp.path(),
            "s.jsn",
            r#"{ "scene": [{ "components": { "bevy_ecs::name::Name": "root" } }] }"#,
        );
        assert!(!is_prefab(&scene, &kinds()));

        let garbage = write(tmp.path(), "g.jsn", "not json at all");
        assert!(!is_prefab(&garbage, &kinds()));
    }

    #[test]
    fn a_header_naming_a_type_the_editor_does_not_open_is_taken_at_its_word() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(
            tmp.path(),
            "rat.bsn",
            &asset_file_text(
                "my_game::content::MobDef",
                "#rat\nmy_game::content::ItemDef { }\n",
            ),
        );
        let mut cache = AssetKindCache::default();
        cache.follow(&kinds());

        assert_eq!(
            cache.type_of(&path).as_deref(),
            Some("my_game::content::MobDef"),
            "the header stands for the file, so the document is never parsed"
        );
    }

    #[test]
    fn a_kind_registered_later_makes_a_trusted_header_worth_reading_past() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = write(
            tmp.path(),
            "rat.bsn",
            &asset_file_text(
                "my_game::content::MobDef",
                "#rat\nmy_game::content::ItemDef { }\n",
            ),
        );
        let mut cache = AssetKindCache::default();
        cache.follow(&kinds());
        assert_eq!(
            cache.type_of(&path).as_deref(),
            Some("my_game::content::MobDef")
        );

        let mut wider = kinds();
        wider.register(AssetKind::extension(
            "mob",
            "Mob",
            "my_game::content::MobDef",
        ));
        cache.follow(&wider);

        assert_eq!(
            cache.type_of(&path).as_deref(),
            Some(ITEM_TYPE),
            "the file is read again, and its root is the truth for a type the editor opens"
        );
    }

    #[test]
    fn a_walk_finds_documents_in_every_folder_but_the_hidden_ones() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("content/items")).expect("dirs");
        std::fs::create_dir_all(tmp.path().join(".jackdaw")).expect("dirs");
        write(&tmp.path().join("content/items"), "torch.bsn", "#torch\n");
        write(&tmp.path().join(".jackdaw"), "cache.bsn", "#hidden\n");
        write(tmp.path(), "sky.png", "not a document");

        let found = walk_document_files(tmp.path());
        assert_eq!(found.len(), 1, "got {found:?}");
        assert!(found[0].ends_with("content/items/torch.bsn"));
    }
}
