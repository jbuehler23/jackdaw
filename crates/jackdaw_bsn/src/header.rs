//! What an asset `.bsn` file holds: the header line naming its reflect type,
//! the walk that finds those files, and a reader for a game that wants the
//! value rather than an asset.
//!
//! The header sits in the leading comment block, on the line after the
//! version stamp when one is present. It is a hint for anything reading the
//! file before it parses; the first document root's type is the truth.

use std::any::TypeId;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::ecs::entity::Entity;
use bevy::reflect::{FromReflect, GetTypeRegistration, TypePath, TypeRegistry};

use crate::catalog::asset_value_from_root;
use crate::{BsnLoadError, SceneBsnAst, bsn_value_to_reflect, parse_bsn_text};

/// The comment marker that introduces an asset file's type header.
pub const ASSET_HEADER: &str = "// jackdaw asset ";

/// The type path of the marker a prefab document's roots carry.
pub const PREFAB_TYPE: &str = "jackdaw::prefab::components::Prefab";

/// How deep under the assets directory a walk looks.
pub(crate) const MAX_ASSET_DEPTH: usize = 12;

/// Prepend the header naming the type an asset file holds.
pub fn with_asset_header(type_path: &str, body: &str) -> String {
    format!("{ASSET_HEADER}{type_path}\n{body}")
}

/// The type an asset file's header names, read from its leading comment lines.
pub fn read_asset_header(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(type_path) = line.strip_prefix(ASSET_HEADER) {
            let type_path = type_path.trim();
            return (!type_path.is_empty()).then(|| type_path.to_string());
        }
        if !line.starts_with("//") {
            return None;
        }
    }
    None
}

/// The bare name a path or a reference carries: the last segment's text
/// before its first dot, so `materials/grass.material.bsn` and `grass` are
/// both `grass`.
pub fn asset_stem(reference: &str) -> &str {
    let last = reference
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(reference)
        .trim_start_matches('.');
    last.split_once('.').map_or(last, |(stem, _)| stem)
}

/// [`asset_stem`] of the file a path names.
pub fn path_stem(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(String::new, |name| asset_stem(name).to_string())
}

/// What each bare name stands for, over the documents a walk saw.
///
/// A name one file carries stands for that file; a name two files carry stands
/// for neither, and the two are there to be named in what says so. The editor
/// and the runtime both count over every document their walk saw, so a name
/// one calls ambiguous is ambiguous in the other.
#[derive(Default, Debug, Clone)]
pub struct StemIndex(BTreeMap<String, Vec<PathBuf>>);

impl StemIndex {
    /// The stems of every path given.
    pub fn from_paths<I, P>(paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        let mut index = Self::default();
        for path in paths {
            index.insert(path);
        }
        index
    }

    pub fn insert(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        let stem = path_stem(&path);
        if stem.is_empty() {
            return;
        }
        let held = self.0.entry(stem).or_default();
        if !held.contains(&path) {
            held.push(path);
        }
    }

    /// Forget a file, for one that has left the walk.
    pub fn remove(&mut self, path: &Path) {
        let stem = path_stem(path);
        let Some(held) = self.0.get_mut(&stem) else {
            return;
        };
        held.retain(|held| held != path);
        if held.is_empty() {
            self.0.remove(&stem);
        }
    }

    /// The one file this name stands for, or `None` when no file or several do.
    pub fn unique(&self, stem: &str) -> Option<&Path> {
        match self.0.get(stem)?.as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }

    /// The first two files a name several carry stands for, to name in a report.
    pub fn shared(&self, stem: &str) -> Option<(&Path, &Path)> {
        match self.0.get(stem)?.as_slice() {
            [first, second, ..] => Some((first, second)),
            _ => None,
        }
    }

    /// Every file this name stands for.
    pub fn paths(&self, stem: &str) -> &[PathBuf] {
        self.0.get(stem).map_or(&[], Vec::as_slice)
    }
}

/// The type path a document root names, or `None` when the root names none.
pub fn root_type_path(ast: &SceneBsnAst, root: Entity) -> Option<String> {
    asset_value_from_root(ast, root).map(|(type_path, _)| type_path)
}

/// The type a document names as the one it holds: [`PREFAB_TYPE`] for a
/// document whose roots carry the prefab marker, else its first root's type.
/// `None` for a document that spawns a hierarchy rather than holding one value.
pub fn document_type_path(text: &str) -> Option<String> {
    let ast = parse_bsn_text(text).ok()?;
    if text.contains(PREFAB_TYPE)
        && ast
            .roots
            .iter()
            .any(|&root| ast.find_patch_by_type_path(root, PREFAB_TYPE).is_some())
    {
        return Some(PREFAB_TYPE.to_string());
    }
    let root = *ast.roots.first()?;
    if !ast.get_children_ast(root).is_empty() {
        return None;
    }
    root_type_path(&ast, root)
}

/// The type the document at `path` holds, taken from its first root and from
/// its header only where the document names no type of its own. Either form
/// of the document reads.
pub fn asset_file_type(path: &Path) -> Option<String> {
    if !crate::file::is_document_path(path) {
        return None;
    }
    let text = crate::file::read_document_text(path).ok()?;
    asset_text_type(&text, path)
}

/// [`asset_file_type`] over text already in hand, `path` naming it in the
/// warning a header that disagrees with the document earns.
pub fn asset_text_type(text: &str, path: &Path) -> Option<String> {
    let header = read_asset_header(text);
    let Some(found) = document_type_path(text) else {
        return header;
    };
    if let Some(header) = header.filter(|header| *header != found) {
        log::warn!(
            "{} says it holds a {header} but its first root is a {found}; going by the root",
            path.display()
        );
    }
    Some(found)
}

/// Every document under `dir`, as absolute paths, skipping hidden directories
/// and following no symlink out of the tree.
///
/// A document held in both forms is listed once, as its text file.
pub fn walk_document_files(dir: &Path) -> Vec<PathBuf> {
    let found = walk_files_with_extensions(dir, &["bsn", "bsb", "jsn"]);
    let text: std::collections::HashSet<PathBuf> = found
        .iter()
        .filter(|path| !crate::file::is_binary_path(path))
        .cloned()
        .collect();
    found
        .into_iter()
        .filter(|path| {
            !crate::file::is_binary_path(path) || !text.contains(&crate::file::text_twin(path))
        })
        .collect()
}

/// Every file under `dir` whose extension is one of `extensions`, as absolute
/// paths, skipping hidden directories and following no symlink out of the tree.
pub fn walk_files_with_extensions(dir: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_files(dir, extensions, 0, &mut found);
    found.sort();
    found
}

fn collect_files(dir: &Path, extensions: &[&str], depth: usize, found: &mut Vec<PathBuf>) {
    if depth > MAX_ASSET_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_files(&path, extensions, depth + 1, found);
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extensions
                    .iter()
                    .any(|wanted| extension.eq_ignore_ascii_case(wanted))
            })
        {
            found.push(path);
        }
    }
}

/// Every asset file under `assets_root`, as the path it sits at and the type
/// it holds. Scenes and prefabs are not asset files and are left out.
pub fn walk_asset_files(assets_root: &Path) -> impl Iterator<Item = (PathBuf, String)> {
    walk_document_files(assets_root)
        .into_iter()
        .filter_map(|path| {
            let type_path = asset_file_type(&path)?;
            (type_path != PREFAB_TYPE).then_some((path, type_path))
        })
}

/// Why an asset file did not read as the value it was asked for.
#[derive(Debug, thiserror::Error)]
pub enum AssetFileError {
    #[error("{1}")]
    Read(PathBuf, crate::file::DocumentError),
    #[error("failed to parse {}: {}", .0.display(), .1)]
    Parse(PathBuf, BsnLoadError),
    #[error("{} holds no value", .0.display())]
    Empty(PathBuf),
    #[error("{} holds a {found}, not a {expected}", .path.display())]
    WrongType {
        path: PathBuf,
        expected: String,
        found: String,
    },
    #[error("{} does not read as a {}", .0.display(), .1)]
    Unreadable(PathBuf, String),
}

/// Read the one value an asset file holds, for a game that keeps its
/// definitions as rows rather than in an `Assets<T>` store.
///
/// The header is checked when the file carries one, the document's first root
/// is the value, and fields naming other assets by path are left at their
/// defaults, since resolving those takes an asset server.
pub fn read_asset_file<T>(path: &Path) -> Result<T, AssetFileError>
where
    T: FromReflect + TypePath + GetTypeRegistration,
{
    let expected = T::type_path();
    let text = crate::file::read_document_text(path)
        .map_err(|err| AssetFileError::Read(path.to_path_buf(), err))?;
    if let Some(header) = read_asset_header(&text).filter(|header| header != expected) {
        return Err(AssetFileError::WrongType {
            path: path.to_path_buf(),
            expected: expected.to_string(),
            found: header,
        });
    }
    let ast =
        parse_bsn_text(&text).map_err(|err| AssetFileError::Parse(path.to_path_buf(), err))?;
    let root = *ast
        .roots
        .first()
        .ok_or_else(|| AssetFileError::Empty(path.to_path_buf()))?;
    let (found, value) = asset_value_from_root(&ast, root)
        .ok_or_else(|| AssetFileError::Empty(path.to_path_buf()))?;
    if found != expected {
        return Err(AssetFileError::WrongType {
            path: path.to_path_buf(),
            expected: expected.to_string(),
            found,
        });
    }
    let mut registry = TypeRegistry::default();
    registry.register::<T>();
    bsn_value_to_reflect(&value, TypeId::of::<T>(), &registry, None)
        .and_then(|reflected| T::from_reflect(&*reflected))
        .ok_or_else(|| AssetFileError::Unreadable(path.to_path_buf(), expected.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_bsn_text;

    const STAMP: &str = "// jackdaw 0.19.0 | bevy 0.19";

    #[test]
    fn an_asset_file_s_header_round_trips() {
        let text = with_asset_header("my_game::content::ItemDef", "my_game::content::ItemDef {}");
        assert_eq!(
            read_asset_header(&text).as_deref(),
            Some("my_game::content::ItemDef")
        );
    }

    #[test]
    fn a_header_below_the_version_stamp_is_found() {
        let text = format!(
            "{STAMP}\n{}",
            with_asset_header("my_game::content::ItemDef", "my_game::content::ItemDef {}")
        );
        assert_eq!(
            read_asset_header(&text).as_deref(),
            Some("my_game::content::ItemDef")
        );
    }

    #[test]
    fn a_file_without_a_header_names_no_type() {
        assert!(read_asset_header("my_game::content::ItemDef {}").is_none());
        assert!(read_asset_header(&format!("{STAMP}\nmy_game::content::ItemDef {{}}")).is_none());
    }

    #[test]
    fn a_comment_below_the_body_is_not_a_header() {
        let text =
            format!("my_game::content::ItemDef {{}}\n{ASSET_HEADER}my_game::content::Impostor\n");
        assert!(read_asset_header(&text).is_none());
    }

    #[test]
    fn a_name_is_the_last_segment_before_its_first_dot() {
        assert_eq!(asset_stem("materials/grass.material.bsn"), "grass");
        assert_eq!(asset_stem("grass"), "grass");
        assert_eq!(asset_stem("zones/hedgerow/grass.bsn"), "grass");
        assert_eq!(
            path_stem(Path::new("content/items/torch.item.bsn")),
            "torch"
        );
    }

    #[test]
    fn a_name_two_documents_carry_stands_for_neither_and_names_both() {
        let index = StemIndex::from_paths([
            Path::new("zones/grass.bsn").to_path_buf(),
            Path::new("materials/grass.material.bsn").to_path_buf(),
        ]);

        assert!(index.unique("grass").is_none());
        assert_eq!(
            index
                .shared("grass")
                .map(|(first, second)| (first.to_path_buf(), second.to_path_buf())),
            Some((
                PathBuf::from("zones/grass.bsn"),
                PathBuf::from("materials/grass.material.bsn")
            ))
        );
    }

    #[test]
    fn a_name_one_document_carries_stands_for_it() {
        let index = StemIndex::from_paths([PathBuf::from("materials/grass.material.bsn")]);

        assert_eq!(
            index.unique("grass"),
            Some(Path::new("materials/grass.material.bsn"))
        );
        assert!(index.unique("slate").is_none());
    }

    #[test]
    fn a_root_reports_the_type_it_names() {
        let ast = parse_bsn_text("#Sword\nmy_game::content::ItemDef { damage: 3.0 }")
            .expect("document parses");
        let root = *ast.roots.first().expect("one root");
        assert_eq!(
            root_type_path(&ast, root).as_deref(),
            Some("my_game::content::ItemDef")
        );
    }

    /// A game's own definition type, of the shape a game keeps as rows.
    #[derive(bevy::reflect::Reflect, Default, Debug, PartialEq)]
    #[type_path = "my_game::content"]
    struct ItemDef {
        damage: f32,
        name: String,
    }

    fn write(dir: &Path, relative: &str, text: &str) -> PathBuf {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent directory")).expect("dir");
        std::fs::write(&path, text).expect("the file is written");
        path
    }

    #[test]
    fn an_asset_file_reads_back_as_the_value_it_holds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(
            dir.path(),
            "content/items/torch.bsn",
            &with_asset_header(
                "my_game::content::ItemDef",
                "#torch\nmy_game::content::ItemDef { damage: 3.0, name: \"Torch\" }\n",
            ),
        );

        let item = read_asset_file::<ItemDef>(&path).expect("the file reads as an item");

        assert_eq!(
            item,
            ItemDef {
                damage: 3.0,
                name: "Torch".to_string(),
            }
        );
    }

    #[test]
    fn an_asset_file_whose_root_carries_no_name_still_reads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(
            dir.path(),
            "content/items/lamp.bsn",
            &with_asset_header(
                "my_game::content::ItemDef",
                "my_game::content::ItemDef { damage: 1.0, name: \"Lamp\" }\n",
            ),
        );

        let item = read_asset_file::<ItemDef>(&path).expect("the file reads as an item");

        assert_eq!(item.name, "Lamp");
    }

    #[test]
    fn a_file_holding_another_type_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(
            dir.path(),
            "content/mobs/rat.bsn",
            "#rat\nmy_game::content::MobDef { health: 12.0 }\n",
        );

        let refused = read_asset_file::<ItemDef>(&path);

        assert!(
            matches!(refused, Err(AssetFileError::WrongType { .. })),
            "got {refused:?}"
        );
    }

    #[test]
    fn a_walk_lists_asset_files_with_their_types_and_leaves_scenes_and_prefabs_out() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "content/items/torch.bsn",
            "#torch\nmy_game::content::ItemDef { damage: 3.0 }\n",
        );
        write(
            dir.path(),
            "zones/starter.bsn",
            "#Root\nbevy_transform::components::transform::Transform\nbevy_ecs::hierarchy::Children [\n    bevy_transform::components::transform::Transform\n]\n",
        );
        write(
            dir.path(),
            "prefabs/tree.bsn",
            "#tree\njackdaw::prefab::components::Prefab\nbevy_ecs::hierarchy::Children [\n    bevy_transform::components::transform::Transform\n]\n",
        );

        let found: Vec<(PathBuf, String)> = walk_asset_files(dir.path()).collect();

        assert_eq!(found.len(), 1, "got {found:?}");
        assert!(found[0].0.ends_with("content/items/torch.bsn"));
        assert_eq!(found[0].1, "my_game::content::ItemDef");
    }
}
