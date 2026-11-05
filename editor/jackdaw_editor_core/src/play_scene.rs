//! What a running game spawns a scene from.
//!
//! A scene root in the game asks by asset path ([`StateEvent::RequestScene`]).
//! [`scene_answer`] replies with the document's current text, for a scene or
//! prefab open in a tab or a prefab with a cached edit, or tells the game to load
//! the file. Nothing is written: the text
//! holds the node ids a file saved before they existed lacks, and unsaved edits.
//!
//! The text is never written anywhere, so each asset it embeds goes by a name
//! that is unique for the Play session ([`PlayAssetNames`]). A scene the game
//! loads from a file cannot spell one, so it cannot take an edit's asset away.
//!
//! What the scene reaches beyond its text goes with the answer: see
//! [`crate::play_overlay`].
//!
//! [`StateEvent::RequestScene`]: jackdaw_pie_protocol::StateEvent::RequestScene

use std::any::TypeId;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use bevy::asset::UntypedAssetId;
use bevy::prelude::*;
use jackdaw_bsn::{CatalogAssetRef, SceneBsnAst};
use jackdaw_pie_protocol::manifest::RunConfig;
use jackdaw_pie_protocol::{ControlEvent, SceneAnswer};

use crate::pie::{InstanceKey, MAIN_SCENE, open_scene_asset_path};
use crate::scene_io::asset_fields::{AbsoluteAssetField, refusal};
use crate::scene_io::save::GameDocument;
use crate::scenes::{Scenes, TabContent, TabKind};

/// What an instance plays in place of the scene its game usually asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchScene {
    /// The asset path this stands in for.
    pub replaces: String,
    /// What the game gets when it asks for `replaces`.
    pub scene: PlayScene,
}

/// A scene recorded when Play was pressed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlayScene {
    /// The scene at this asset path.
    Path(String),
    /// The tab of this name, which has no file.
    Untitled(String),
}

/// The scene the scene in front makes `run` play, if any. A run configuration
/// or project setting that names a scene keeps it, so the scene in front does
/// not take its place.
pub fn launch_scene(world: &World, run: &RunConfig) -> Option<LaunchScene> {
    let names_scene = run.env.contains_key("JACKDAW_PIE_SCENE")
        || world
            .get_resource::<crate::play_settings::PlaySettings>()
            .is_some_and(|settings| settings.env.contains_key("JACKDAW_PIE_SCENE"));
    if names_scene {
        return None;
    }
    let scenes = world.get_resource::<Scenes>()?;
    let tab = scenes.tabs.get(scenes.active)?;
    if tab.kind != TabKind::Scene || tab.is_refused() {
        return None;
    }
    let scene = match &tab.path {
        None if crate::scenes::operators::active_tab_modified(world) => {
            PlayScene::Untitled(tab.display_name.clone())
        }
        None => return None,
        Some(_) => PlayScene::Path(open_scene_asset_path(world).filter(|s| s != MAIN_SCENE)?),
    };
    let replaces = run
        .env
        .get("JACKDAW_PIE_REPLACES")
        .cloned()
        .unwrap_or_else(|| MAIN_SCENE.to_string());
    Some(LaunchScene { replaces, scene })
}

/// What the game spawns for a root asking for the asset path `requested`, for an
/// instance launched with `launch`. The document the path resolves to, if open in
/// a tab or an edited prefab in the cache, answers with its text; anything else
/// answers with the file.
pub fn scene_answer(
    world: &mut World,
    launch: Option<&LaunchScene>,
    requested: &str,
) -> SceneAnswer {
    answer_with_assets(world, launch, requested).answer
}

/// What goes to a game asking for a scene, in send order.
pub struct ServedScene {
    /// The asset files the document names that the game may not hold as the
    /// editor does.
    pub files: Option<ControlEvent>,
    /// The unsaved prefabs the document reaches that the game does not hold as
    /// the editor does.
    pub prefabs: Option<ControlEvent>,
    pub answer: SceneAnswer,
}

/// [`scene_answer`] for the instance `key`, which counts as holding every asset
/// the text embeds from here on.
pub fn serve_scene(
    world: &mut World,
    key: &InstanceKey,
    launch: Option<&LaunchScene>,
    requested: &str,
) -> ServedScene {
    let Answered {
        answer,
        assets,
        prefabs,
    } = answer_with_assets(world, launch, requested);
    let files = asset_files_missing_from(world, key, &assets.files);
    let mut names = world.get_resource_or_init::<PlayAssetNames>();
    for (name, definition) in assets.embedded {
        names.note_given(key, name, definition);
    }
    let prefabs = crate::play_overlay::prefab_files_missing_from(world, key, prefabs);
    ServedScene {
        files,
        prefabs,
        answer,
    }
}

/// The definitions of those of `files` the game `key` does not hold, now counted
/// as given.
pub(crate) fn asset_files_missing_from(
    world: &mut World,
    key: &InstanceKey,
    files: &[AssetFile],
) -> Option<ControlEvent> {
    let mut sent = Vec::new();
    for file in files {
        let Some(definition) = asset_definition(world, file.id, file.type_id) else {
            continue;
        };
        let mut names = world.get_resource_or_init::<PlayAssetNames>();
        if names.has_given(key, &file.path, definition) {
            continue;
        }
        names.note_given(key, file.path.clone(), definition);
        let asset = CatalogAssetRef {
            name: "File".to_string(),
            type_id: file.type_id,
            asset_id: file.id,
        };
        sent.push((
            file.path.clone(),
            jackdaw_bsn::serialize_assets_to_bsn(world, &[asset]),
        ));
    }
    (!sent.is_empty()).then_some(ControlEvent::AssetFiles { files: sent })
}

/// An asset file a game is told about by path.
pub(crate) struct AssetFile {
    pub(crate) path: String,
    pub(crate) type_id: TypeId,
    pub(crate) id: UntypedAssetId,
}

/// What a document's text holds or names, for tracking what a game was given.
#[derive(Default)]
pub(crate) struct DocumentAssets {
    /// The `#Name` and definition of each asset root its text holds.
    pub(crate) embedded: Vec<(String, u64)>,
    /// The asset files it names.
    pub(crate) files: Vec<AssetFile>,
}

impl DocumentAssets {
    /// What `text`, parsed as `parsed`, holds or names.
    pub(crate) fn of(world: &World, parsed: &mut SceneBsnAst) -> Self {
        Self {
            embedded: embedded_assets(world, parsed),
            files: asset_files_named(world, parsed),
        }
    }
}

/// An answer and what goes with it.
struct Answered {
    answer: SceneAnswer,
    /// For a document, what its text and its prefabs' texts hold or name.
    assets: DocumentAssets,
    /// The unsaved prefabs the scene reaches.
    prefabs: Vec<crate::play_overlay::UnsavedPrefab>,
}

impl Answered {
    /// The file answer for `path`, with what the file reaches beyond itself.
    fn use_file(world: &mut World, path: String) -> Self {
        let overlay = crate::play_overlay::overlay_of_file(world, &path);
        Self {
            answer: SceneAnswer::UseFile {
                path,
                sidecars: overlay.sidecars,
            },
            assets: overlay.assets,
            prefabs: overlay.prefabs,
        }
    }
}

/// The answer and what goes with it.
fn answer_with_assets(
    world: &mut World,
    launch: Option<&LaunchScene>,
    requested: &str,
) -> Answered {
    let requested = requested.replace('\\', "/");
    let target = match launch {
        Some(launch) if launch.replaces == requested => launch.scene.clone(),
        _ => PlayScene::Path(requested.clone()),
    };
    let served = match &target {
        PlayScene::Path(path) => ServedAs::of(path, None),
        PlayScene::Untitled(name) => ServedAs::of(&requested, Some(name.clone())),
    };
    let file = match &target {
        PlayScene::Path(path) => path.clone(),
        PlayScene::Untitled(_) => requested.clone(),
    };

    let Some(assets) = crate::asset_index::assets_dir(world) else {
        return Answered::use_file(world, file);
    };
    let tab = tab_of(world, &target);
    if let (None, PlayScene::Untitled(name)) = (tab, &target) {
        let message =
            format!("{name} was saved or closed during the build, so the game loads {requested}");
        warn!("PIE: {message}");
        crate::status_bar::notify_warn(world, message);
    }
    let parent = tab.map_or_else(
        || assets.join(&served.dir),
        |tab| tab_folder(world, tab, &assets, &served.dir),
    );
    let document = match (tab, &target) {
        (Some(tab), _) => document_of_tab(world, tab, &parent),
        (None, PlayScene::Path(path)) => crate::play_overlay::unsaved_cached_prefab(world, path),
        (None, PlayScene::Untitled(_)) => None,
    };
    let Some(document) = document else {
        return Answered::use_file(world, file);
    };
    let session_text = with_session_names(world, &document);
    let (text, mut parsed) = match session_text {
        Ok(served) => served,
        Err(err) => {
            warn!("PIE: the document for {requested} does not parse back, using its file: {err}");
            return Answered::use_file(world, file);
        }
    };
    warn_absolute(world, &document.absolute);
    let mut held = DocumentAssets::of(world, &mut parsed);
    let overlay = crate::play_overlay::overlay_of_document(
        world,
        tab,
        &target,
        &parsed,
        &parent,
        &served.dir,
    );
    held.embedded.extend(overlay.assets.embedded);
    held.files.extend(overlay.assets.files);
    Answered {
        answer: SceneAnswer::Document {
            text,
            dir: served.dir,
            stem: served.stem,
            sidecars: overlay.sidecars,
        },
        assets: held,
        prefabs: overlay.prefabs,
    }
}

/// `document`'s text with every embedded asset under its session name, and the
/// text parsed back. A root and every reference to it are renamed together,
/// including the `@Name` spelling a load also answers to, whatever wrote them. A
/// root whose asset the live stores hold is written from the store, which an edit
/// reads too.
pub(crate) fn with_session_names(
    world: &mut World,
    document: &GameDocument,
) -> Result<(String, SceneBsnAst), jackdaw_bsn::UnparsableEmit> {
    let mut ast = jackdaw_bsn::parse_emitted(&document.text)?;
    world.init_resource::<PlayAssetNames>();
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let mut renames: HashMap<String, String> = HashMap::new();
    let mut from_store: Vec<CatalogAssetRef> = Vec::new();
    let mut named: HashSet<String> = HashSet::new();
    world.resource_scope(|world, mut names: Mut<PlayAssetNames>| {
        for root in jackdaw_bsn::asset_roots(&ast, &registry) {
            let Some(old) = ast.get_name(root).map(str::to_owned) else {
                continue;
            };
            let Some(type_id) =
                jackdaw_bsn::asset_value_from_root(&ast, root).and_then(|(type_path, _)| {
                    registry
                        .get_with_type_path(&type_path)
                        .map(bevy::reflect::TypeRegistration::type_id)
                })
            else {
                continue;
            };
            let live = document
                .live_assets
                .get(&old)
                .filter(|id| id.type_id() == type_id)
                .and_then(|&id| Some((id, names.resolve(world, id, type_id)?)));
            let new = match live {
                Some((asset_id, new)) => {
                    ast.roots.retain(|&held| held != root);
                    if named.insert(new.clone()) {
                        from_store.push(CatalogAssetRef {
                            name: new.trim_start_matches('#').to_string(),
                            type_id,
                            asset_id,
                        });
                    }
                    new
                }
                None => {
                    let Some(definition) = root_definition(&ast, root) else {
                        continue;
                    };
                    let new = names.of_definition(definition);
                    if named.insert(new.clone()) {
                        rename_root(&mut ast, root, new.trim_start_matches('#'));
                    } else {
                        ast.roots.retain(|&held| held != root);
                    }
                    new
                }
            };
            renames.insert(format!("@{old}"), new.clone());
            renames.insert(format!("#{old}"), new);
        }
    });
    crate::scene_io::save::visit_asset_references(&mut ast, &registry, |reference| {
        if let Some(new) = renames.get(reference.as_str()) {
            reference.clone_from(new);
        }
    });
    drop(registry);
    jackdaw_bsn::append_assets_to_ast(&mut ast, world, &from_store);
    let text = jackdaw_bsn::emit_scene(&ast);
    let parsed = jackdaw_bsn::parse_emitted(&text)?;
    Ok((text, parsed))
}

fn rename_root(ast: &mut SceneBsnAst, root: Entity, name: &str) {
    let name_patch = ast.get_patches(root).and_then(|patches| {
        patches
            .0
            .iter()
            .copied()
            .find(|&patch| matches!(ast.get_patch(patch), Some(jackdaw_bsn::BsnPatch::Name(_))))
    });
    if let Some(patch) = name_patch {
        ast.set_patch(patch, jackdaw_bsn::BsnPatch::Name(name.to_string()));
    }
}

/// The `#Name` and definition of each asset root in `ast`.
fn embedded_assets(world: &World, ast: &SceneBsnAst) -> Vec<(String, u64)> {
    let registry = world.resource::<AppTypeRegistry>().read();
    jackdaw_bsn::asset_roots(ast, &registry)
        .into_iter()
        .filter_map(|root| {
            let name = ast.get_name(root)?;
            Some((format!("#{name}"), root_definition(ast, root)?))
        })
        .collect()
}

/// The project's asset files `ast` names by path.
fn asset_files_named(world: &World, ast: &mut SceneBsnAst) -> Vec<AssetFile> {
    let Some(project) = world.get_resource::<jackdaw_bsn::BsnProjectAssets>() else {
        return Vec::new();
    };
    let registry = world.resource::<AppTypeRegistry>().read();
    let mut files: BTreeMap<String, AssetFile> = BTreeMap::new();
    crate::scene_io::save::visit_asset_references(ast, &registry, |reference| {
        if !jackdaw_bsn::is_document_path(Path::new(reference.as_str())) {
            return;
        }
        if let Some(handle) = project.0.get(reference.as_str()) {
            files.insert(
                reference.clone(),
                AssetFile {
                    path: reference.clone(),
                    type_id: handle.type_id(),
                    id: handle.id(),
                },
            );
        }
    });
    files.into_values().collect()
}

/// A digest of what an asset root defines, leaving out its name.
fn root_definition(ast: &SceneBsnAst, root: Entity) -> Option<u64> {
    let value = jackdaw_bsn::asset_value_from_root(ast, root)?;
    let mut hasher = DefaultHasher::new();
    format!("{value:?}").hash(&mut hasher);
    Some(hasher.finish())
}

/// A digest of the asset `id` as a document would embed it, read back from text
/// so it compares with [`root_definition`] of a served document. `None` when the
/// asset cannot be embedded.
fn asset_definition(world: &World, id: UntypedAssetId, type_id: TypeId) -> Option<u64> {
    let asset = CatalogAssetRef {
        name: "Asset".to_string(),
        type_id,
        asset_id: id,
    };
    let (text, skipped) = jackdaw_bsn::serialize_assets_to_bsn_reporting(world, &[asset]);
    if !skipped.is_empty() {
        return None;
    }
    let ast = jackdaw_bsn::parse_emitted(&text).ok()?;
    root_definition(&ast, *ast.roots.first()?)
}

/// The folder and name a document goes by in the game.
struct ServedAs {
    dir: String,
    stem: String,
}

impl ServedAs {
    fn of(path: &str, name: Option<String>) -> Self {
        let path = Path::new(path);
        let dir = path
            .parent()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = name.unwrap_or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        Self { dir, stem }
    }
}

/// The index of the tab holding `target`, when it holds a scene or prefab that
/// loaded.
fn tab_of(world: &World, target: &PlayScene) -> Option<usize> {
    let scenes = world.get_resource::<Scenes>()?;
    let project = world.get_resource::<crate::project::ProjectRoot>()?;
    scenes.tabs.iter().position(|tab| {
        !tab.is_refused()
            && match target {
                PlayScene::Untitled(name) => tab.path.is_none() && tab.display_name == *name,
                PlayScene::Path(path) => tab
                    .path
                    .as_deref()
                    .is_some_and(|held| project.to_asset_path(held).as_deref() == Some(path)),
            }
    })
}

/// Where the tab's document counts as being, on disk.
fn tab_folder(world: &World, tab: usize, assets: &Path, dir: &str) -> PathBuf {
    world.resource::<Scenes>().tabs[tab]
        .path
        .as_deref()
        .and_then(Path::parent)
        .map_or_else(|| assets.join(dir), Path::to_path_buf)
}

/// The text of the document in tab `index`, read from the document: the live one
/// for the tab in front, the snapshot for any other, or the cache entry for a
/// prefab behind.
pub(crate) fn document_of_tab(
    world: &mut World,
    index: usize,
    parent: &Path,
) -> Option<GameDocument> {
    let scenes = world.resource::<Scenes>();
    if scenes.active == index {
        if !world.contains_resource::<jackdaw_bsn::SceneBsnAst>() {
            return None;
        }
        return Some(crate::scene_io::save::emit_bsn_scene_for_game(
            world, parent,
        ));
    }
    match &scenes.tabs[index].content {
        TabContent::Scene(Some(document)) => Some(crate::scene_io::save::emit_snapshot_for_game(
            world, document, parent,
        )),
        TabContent::Prefab(path) => world
            .get_resource::<crate::prefab::PrefabAstCache>()?
            .get(path.as_path())
            .map(|document| crate::scene_io::save::emit_snapshot_for_game(world, document, parent)),
        TabContent::Scene(None) => None,
    }
}

pub(crate) fn warn_absolute(world: &mut World, absolute: &[AbsoluteAssetField]) {
    if absolute.is_empty() {
        return;
    }
    let message = format!("Playing anyway: {}", refusal(absolute));
    warn!("PIE: {message}");
    crate::status_bar::notify_warn(world, message);
}

/// The one `#Name` each runtime asset goes by for this Play session, for served
/// text and live sync alike, and what each running game has been given.
///
/// Every name is minted here and held until Play stops, whatever a save or a
/// re-emit numbers the asset. A reparse of the document gives an asset a new id,
/// so an id with no name takes the name of an asset with the same definition,
/// rather than sending the game a copy per reparse.
#[derive(Resource, Default)]
pub struct PlayAssetNames {
    names: HashMap<UntypedAssetId, String>,
    /// The definition each name stands for, by `#Name`.
    definitions: BTreeMap<String, u64>,
    next: usize,
    /// The definition each game last received under each `#Name`, or under the
    /// path of an asset file.
    given: HashMap<InstanceKey, HashMap<String, u64>>,
    /// The digest of the unsaved text each game last received for each prefab, by
    /// path under the asset folder.
    given_prefabs: HashMap<InstanceKey, HashMap<String, u64>>,
}

impl PlayAssetNames {
    /// The `#Name` of the asset `id`, or `None` when the asset cannot be embedded
    /// for a game.
    pub(crate) fn resolve(
        &mut self,
        world: &World,
        id: UntypedAssetId,
        type_id: TypeId,
    ) -> Option<String> {
        let definition = asset_definition(world, id, type_id)?;
        if let Some(name) = self.names.get(&id).cloned() {
            if self.definitions.get(&name) == Some(&definition)
                || !self.shared_in_document(world, id, &name)
            {
                self.definitions.insert(name.clone(), definition);
                return Some(name);
            }
            self.names.remove(&id);
        }
        let name = self.of_definition(definition);
        self.names.insert(id, name.clone());
        Some(name)
    }

    /// Whether an asset other than `id` that the live document uses goes by `name`.
    /// A reparse leaves ids no document uses behind, which something like an undo
    /// step may keep alive, so those go here.
    fn shared_in_document(&mut self, world: &World, id: UntypedAssetId, name: &str) -> bool {
        if self
            .names
            .iter()
            .all(|(held, other)| *held == id || other != name)
        {
            return false;
        }
        let used = crate::play_sync::assets_in_document(world);
        self.names
            .retain(|held, _| *held == id || used.contains(held));
        self.names
            .iter()
            .any(|(held, other)| *held != id && other == name)
    }

    /// The `#Name` a reference to a root of the live document goes by, as
    /// [`with_session_names`] spells it, and the asset behind it where the live
    /// stores hold it. `None` when `reference` names no such root.
    pub(crate) fn session_reference(
        &mut self,
        world: &World,
        registry: &bevy::reflect::TypeRegistry,
        reference: &str,
    ) -> Option<(String, Option<CatalogAssetRef>)> {
        let bare = reference.strip_prefix(['#', '@'])?;
        let ast = world.get_resource::<SceneBsnAst>()?;
        let root = jackdaw_bsn::asset_roots(ast, registry)
            .into_iter()
            .find(|&root| ast.get_name(root) == Some(bare))?;
        let (type_path, _) = jackdaw_bsn::asset_value_from_root(ast, root)?;
        let type_id = registry.get_with_type_path(&type_path)?.type_id();
        let live = world
            .get_resource::<jackdaw_bsn::BsnSceneAssets>()
            .and_then(|assets| assets.0.get(&format!("#{bare}")))
            .map(bevy::asset::UntypedHandle::id)
            .filter(|id| id.type_id() == type_id)
            .and_then(|asset_id| {
                let name = self.resolve(world, asset_id, type_id)?;
                let asset = CatalogAssetRef {
                    name: name.trim_start_matches('#').to_string(),
                    type_id,
                    asset_id,
                };
                Some((name, Some(asset)))
            });
        live.or_else(|| Some((self.of_definition(root_definition(ast, root)?), None)))
    }

    /// The `#Name` the asset `id` goes by, if it has one.
    pub(crate) fn name_of(&self, id: UntypedAssetId) -> Option<String> {
        self.names.get(&id).cloned()
    }

    /// The `#Name` standing for `definition`, minted if none does.
    pub(crate) fn of_definition(&mut self, definition: u64) -> String {
        let same = self
            .definitions
            .iter()
            .find(|(_, held)| **held == definition)
            .map(|(name, _)| name.clone());
        same.unwrap_or_else(|| {
            let name = format!("#Play{}", self.next);
            self.next += 1;
            self.definitions.insert(name.clone(), definition);
            name
        })
    }

    /// Each asset named so far with its `#Name`.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&UntypedAssetId, &String)> {
        self.names.iter()
    }

    /// The definition `#Name` stands for.
    pub(crate) fn definition(&self, name: &str) -> Option<u64> {
        self.definitions.get(name).copied()
    }

    /// How many assets have a name.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether no asset has a name.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Whether the game `key` holds `definition` under `name`.
    pub(crate) fn has_given(&self, key: &InstanceKey, name: &str, definition: u64) -> bool {
        self.given
            .get(key)
            .and_then(|given| given.get(name))
            .is_some_and(|held| *held == definition)
    }

    /// Whether the game `key` holds any definition under `name`.
    pub(crate) fn was_given(&self, key: &InstanceKey, name: &str) -> bool {
        self.given
            .get(key)
            .is_some_and(|given| given.contains_key(name))
    }

    /// The game `key` holds `definition` under `name` from here on. A name a
    /// document embeds with another definition replaces what the game held.
    pub(crate) fn note_given(&mut self, key: &InstanceKey, name: String, definition: u64) {
        self.given
            .entry(key.clone())
            .or_default()
            .insert(name, definition);
    }

    /// Keep what games were given only for those `keep` accepts.
    pub(crate) fn retain_games(&mut self, keep: impl Fn(&InstanceKey) -> bool) {
        self.given.retain(|key, _| keep(key));
        self.given_prefabs.retain(|key, _| keep(key));
    }

    /// Whether the game `key` holds the unsaved prefab at `path` as `digest`.
    pub(crate) fn has_given_prefab(&self, key: &InstanceKey, path: &str, digest: u64) -> bool {
        self.given_prefabs
            .get(key)
            .and_then(|given| given.get(path))
            .is_some_and(|held| *held == digest)
    }

    /// The game `key` holds the unsaved prefab at `path` as `digest` from here on.
    pub(crate) fn note_given_prefab(&mut self, key: &InstanceKey, path: String, digest: u64) {
        self.given_prefabs
            .entry(key.clone())
            .or_default()
            .insert(path, digest);
    }

    /// The game `key` reads the prefab at `path` from its file from here on.
    pub(crate) fn forget_given_prefab(&mut self, key: &InstanceKey, path: &str) {
        if let Some(given) = self.given_prefabs.get_mut(key) {
            given.remove(path);
        }
    }

    /// The paths of the unsaved prefabs the game `key` holds.
    pub(crate) fn given_prefabs(&self, key: &InstanceKey) -> Vec<String> {
        self.given_prefabs
            .get(key)
            .map(|given| given.keys().cloned().collect())
            .unwrap_or_default()
    }
}

pub(crate) fn forget_asset_names(mut names: ResMut<PlayAssetNames>) {
    *names = PlayAssetNames::default();
}

#[cfg(test)]
mod tests {
    use bevy::asset::{AssetPlugin, ReflectAsset, ReflectHandle};
    use jackdaw_bsn::{SceneBsnAst, component_to_bsn_patch};
    use jackdaw_pie_protocol::{RemoteEntity, StateEvent};
    use jackdaw_runtime::JackdawPlugin;
    use jackdaw_scene_types::SceneNodeId;

    use super::*;
    use crate::pie_projection::{PieProjection, project_event};
    use crate::play_sync::AssetReferences;

    const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
    const PAINTED: &str = "game::Painted";

    #[derive(Asset, Reflect, Default, Clone)]
    #[reflect(Asset, Default)]
    #[type_path = "game"]
    struct Swatch {
        tone: u32,
    }

    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    #[type_path = "game"]
    struct Painted {
        level: u32,
        swatch: Handle<Swatch>,
    }

    #[derive(Reflect, Default, Clone)]
    #[type_path = "game"]
    enum Slot {
        #[default]
        Empty,
        Held(Handle<Swatch>),
        Named {
            swatch: Handle<Swatch>,
        },
    }

    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    #[type_path = "game"]
    struct Slotted {
        level: u32,
        first: Slot,
        second: Slot,
    }

    #[derive(Component, Reflect, Default, Clone)]
    #[reflect(Component, Default)]
    #[type_path = "game"]
    enum Coat {
        #[default]
        Bare,
        Painted(Handle<Swatch>),
    }

    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    #[type_path = "game"]
    struct Palette {
        level: u32,
        by_name: bevy::platform::collections::HashMap<String, Handle<Swatch>>,
    }

    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    #[type_path = "game"]
    struct Bag {
        level: u32,
        set: bevy::platform::collections::HashSet<Handle<Swatch>>,
    }

    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    #[type_path = "game"]
    struct Tinted(Handle<Swatch>);

    fn register(app: &mut App) {
        app.init_asset::<Swatch>();
        app.register_asset_reflect::<Swatch>();
        app.register_type_data::<Handle<Swatch>, ReflectHandle>();
        app.register_type::<Painted>();
        app.register_type::<Slotted>();
        app.register_type::<Coat>();
        app.register_type::<Palette>();
        app.register_type::<Bag>();
        app.register_type::<Tinted>();
        app.register_type::<Transform>();
        app.register_type::<SceneNodeId>();
    }

    /// An editor world with an empty document as `zone.bsn`, in front.
    fn editor(dir: &Path) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(AssetPlugin::default());
        register(&mut app);
        app.insert_resource(crate::project::ProjectRoot::new(
            dir.to_path_buf(),
            crate::project::ProjectConfig::default(),
        ));
        let mut tab = crate::scenes::SceneTab::new_untitled(1);
        tab.path = Some(dir.join("assets/zone.bsn"));
        app.insert_resource(Scenes {
            tabs: vec![tab],
            active: 0,
        });
        app.init_resource::<PieProjection>();
        app.init_resource::<PlayAssetNames>();
        app.init_resource::<SceneBsnAst>();
        app
    }

    /// A root node with the given components, linked to a new editor entity.
    fn add_node(app: &mut App, id: u64, painted: Painted, level: u32) -> Entity {
        let world = app.world_mut();
        let entity = world
            .spawn((SceneNodeId(id), Transform::default(), painted))
            .id();
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let patches = vec![
            component_to_bsn_patch(&SceneNodeId(id), &registry),
            component_to_bsn_patch(&Transform::default(), &registry),
            component_to_bsn_patch(
                &Painted {
                    level,
                    swatch: Handle::default(),
                },
                &registry,
            ),
        ];
        let mut ast = world.resource_mut::<SceneBsnAst>();
        let node = ast.create_entity_node(patches);
        ast.add_to_roots(node);
        ast.link(entity, node);
        entity
    }

    fn swatch(app: &mut App, tone: u32) -> Handle<Swatch> {
        app.world_mut()
            .resource_mut::<Assets<Swatch>>()
            .add(Swatch { tone })
    }

    fn text_of(answer: SceneAnswer) -> String {
        match answer {
            SceneAnswer::Document { text, .. } => text,
            SceneAnswer::UseFile { path, .. } => {
                panic!("expected a document, got the file {path}")
            }
        }
    }

    fn answer(app: &mut App) -> String {
        text_of(scene_answer(app.world_mut(), None, "zone.bsn"))
    }

    /// A game on the runtime's own loader, spawning `text` as it would an answer.
    fn game_spawning(text: String) -> App {
        let mut game = App::new();
        game.add_plugins(MinimalPlugins);
        game.add_plugins(bevy::transform::TransformPlugin);
        game.add_plugins(AssetPlugin::default());
        game.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
        register(&mut game);
        game.add_plugins(JackdawPlugin);
        let scene = game
            .world_mut()
            .resource_mut::<Assets<jackdaw_runtime::JackdawScene>>()
            .add(jackdaw_runtime::JackdawScene::new(text, PathBuf::new()));
        game.world_mut()
            .spawn(jackdaw_runtime::JackdawSceneRoot(scene));
        game.update();
        game
    }

    fn game_painted(game: &mut App, id: u64) -> (u32, u32) {
        let mut query = game.world_mut().query::<(&SceneNodeId, &Painted)>();
        let swatch = query
            .iter(game.world())
            .find(|(node, _)| node.0 == id)
            .map(|(_, painted)| (painted.level, painted.swatch.clone()))
            .expect("the node spawned");
        let tone = game
            .world()
            .resource::<Assets<Swatch>>()
            .get(&swatch.1)
            .map_or(u32::MAX, |swatch| swatch.tone);
        (swatch.0, tone)
    }

    fn stream_spawn(id: u64, level: u32, x: f32) -> StateEvent {
        StateEvent::EntitySpawned {
            entity: RemoteEntity {
                entity: 1,
                components: [
                    (
                        TRANSFORM.to_string(),
                        serde_json::json!({
                            "translation": [x, 0.0, 0.0],
                            "rotation": [0.0, 0.0, 0.0, 1.0],
                            "scale": [1.0, 1.0, 1.0]
                        }),
                    ),
                    (
                        PAINTED.to_string(),
                        serde_json::json!({ "level": level, "swatch": null }),
                    ),
                ]
                .into_iter()
                .collect(),
                scene_node_id: Some(id),
            },
        }
    }

    #[test]
    fn projected_game_state_never_reaches_the_text() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut app = editor(dir.path());
        let color = swatch(&mut app, 4);
        let entity = add_node(
            &mut app,
            5,
            Painted {
                level: 1,
                swatch: color.clone(),
            },
            1,
        );
        let before = answer(&mut app);

        project_event(app.world_mut(), stream_spawn(5, 99, 77.0));
        let world = app.world();
        let projected = world.get::<Painted>(entity).expect("the component");
        assert_eq!(projected.level, 99, "the entity shows the game's value");
        assert_eq!(
            world
                .get::<Transform>(entity)
                .expect("a transform")
                .translation
                .x,
            77.0
        );
        assert_eq!(projected.swatch, color, "the stream carries no handle");

        let after = answer(&mut app);
        assert_eq!(after, before, "the document alone decides the text");
        assert!(!after.contains("99") && !after.contains("77"), "{after}");
        assert_eq!(game_painted(&mut game_spawning(after), 5), (1, 4));
    }

    /// A document edit made while the entity is projected is in the text, next
    /// to no game value.
    #[test]
    fn an_edit_of_a_projected_entity_is_read_from_the_document() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut app = editor(dir.path());
        let color = swatch(&mut app, 4);
        let entity = add_node(
            &mut app,
            5,
            Painted {
                level: 1,
                swatch: color,
            },
            1,
        );
        project_event(app.world_mut(), stream_spawn(5, 99, 0.0));
        let registry = app.world().resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        app.world_mut()
            .resource_scope(|_, mut ast: Mut<SceneBsnAst>| {
                let node = ast.ast_for(entity).expect("a node");
                let value = jackdaw_bsn::BsnValue::from_reflect(&12u32, &registry);
                jackdaw_bsn::set_bsn_field(&mut ast, node, PAINTED, "level", value, &registry);
            });
        drop(registry);
        assert_eq!(
            game_painted(&mut game_spawning(answer(&mut app)), 5),
            (12, 4)
        );
    }

    /// A tab switched away from while projected keeps the document's values.
    #[test]
    fn a_snapshot_taken_while_projected_holds_the_document() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut app = editor(dir.path());
        let color = swatch(&mut app, 4);
        add_node(
            &mut app,
            5,
            Painted {
                level: 1,
                swatch: color,
            },
            1,
        );
        project_event(app.world_mut(), stream_spawn(5, 99, 0.0));
        let text = crate::scene_io::emit_bsn_scene_with_inline_assets(app.world_mut(), dir.path());
        assert!(!text.contains("99"), "{text}");
    }

    /// Asset names hold across emits with an asset added ahead in the document,
    /// and live sync's references read the same ones.
    #[test]
    fn an_unsaved_asset_keeps_its_name_across_emits() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut app = editor(dir.path());
        let late = swatch(&mut app, 6);
        add_node(
            &mut app,
            8,
            Painted {
                level: 1,
                swatch: late.clone(),
            },
            1,
        );
        let first = answer(&mut app);
        let name = app
            .world()
            .resource::<PlayAssetNames>()
            .iter()
            .next()
            .map(|(_, name)| name.clone())
            .expect("the asset was named");
        assert!(first.contains(&name), "{first}");
        assert_eq!(game_painted(&mut game_spawning(first), 8), (1, 6));

        let early = swatch(&mut app, 3);
        let added = add_node(
            &mut app,
            9,
            Painted {
                level: 2,
                swatch: early,
            },
            2,
        );
        let world = app.world_mut();
        let mut ast = world.resource_mut::<SceneBsnAst>();
        let node = ast.ast_for(added).expect("a node");
        let moved: Vec<Entity> = std::iter::once(node)
            .chain(ast.roots.iter().copied().filter(|&root| root != node))
            .collect();
        ast.roots = moved;

        let second = answer(&mut app);
        assert!(
            second.contains(&name),
            "the name still means the asset:\n{second}"
        );
        let mut game = game_spawning(second);
        assert_eq!(game_painted(&mut game, 8), (1, 6));
        assert_eq!(game_painted(&mut game, 9), (2, 3));
        assert_eq!(
            AssetReferences::with(app.world(), app.world().resource::<PlayAssetNames>())
                .reference(&late.untyped()),
            Some(name),
            "live sync sends the same name"
        );
    }

    /// A node holding `live`, overlaid with game state, whose document holds
    /// `patch`.
    fn overlaid_node(app: &mut App, id: u64, live: impl Bundle, patch: jackdaw_bsn::BsnPatch) {
        let world = app.world_mut();
        let entity = world
            .spawn((SceneNodeId(id), Transform::default(), live))
            .id();
        let registry = world.resource::<AppTypeRegistry>().clone();
        let patches = vec![
            component_to_bsn_patch(&SceneNodeId(id), &registry.read()),
            component_to_bsn_patch(&Transform::default(), &registry.read()),
            patch,
        ];
        let mut ast = world.resource_mut::<SceneBsnAst>();
        let node = ast.create_entity_node(patches);
        ast.add_to_roots(node);
        ast.link(entity, node);
        world
            .resource_mut::<PieProjection>()
            .overlaid
            .insert(entity);
    }

    fn patch_of(app: &App, value: &dyn PartialReflect) -> jackdaw_bsn::BsnPatch {
        component_to_bsn_patch(value, &app.world().resource::<AppTypeRegistry>().read())
    }

    fn tone_of(game: &App, handle: &Handle<Swatch>) -> u32 {
        game.world()
            .resource::<Assets<Swatch>>()
            .get(handle)
            .map_or(u32::MAX, |swatch| swatch.tone)
    }

    /// A save from Live view writes the document's values and takes only the
    /// references from the entity: in an enum variant, a map value, a set, a tuple
    /// component and a bare-type tuple component.
    #[test]
    fn a_save_of_an_overlaid_entity_keeps_every_kind_of_nested_handle() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut app = editor(dir.path());
        let (one, two, three) = (
            swatch(&mut app, 1),
            swatch(&mut app, 2),
            swatch(&mut app, 3),
        );
        let (four, five, six) = (
            swatch(&mut app, 4),
            swatch(&mut app, 5),
            swatch(&mut app, 6),
        );

        let doc = patch_of(
            &app,
            &Slotted {
                level: 1,
                first: Slot::Held(one.clone()),
                second: Slot::Named {
                    swatch: two.clone(),
                },
            },
        );
        overlaid_node(
            &mut app,
            1,
            Slotted {
                level: 99,
                first: Slot::Held(one),
                second: Slot::Named { swatch: two },
            },
            doc,
        );
        let doc = patch_of(
            &app,
            &Palette {
                level: 1,
                by_name: [("rock".to_string(), three.clone())].into_iter().collect(),
            },
        );
        overlaid_node(
            &mut app,
            2,
            Palette {
                level: 99,
                by_name: [("rock".to_string(), three)].into_iter().collect(),
            },
            doc,
        );
        let doc = patch_of(&app, &Tinted(four.clone()));
        overlaid_node(&mut app, 3, Tinted(four), doc);
        overlaid_node(
            &mut app,
            4,
            Tinted(five),
            jackdaw_bsn::BsnPatch::Type("game::Tinted".to_string()),
        );
        let doc = patch_of(&app, &Coat::Painted(six.clone()));
        overlaid_node(&mut app, 5, Coat::Painted(six.clone()), doc);
        let doc = patch_of(
            &app,
            &Bag {
                level: 1,
                set: [six.clone()].into_iter().collect(),
            },
        );
        overlaid_node(
            &mut app,
            6,
            Bag {
                level: 99,
                set: [six].into_iter().collect(),
            },
            doc,
        );

        let text = crate::scene_io::save::emit_bsn_scene_for_file(app.world_mut(), dir.path())
            .expect("nothing is refused");
        assert!(!text.contains("99"), "the document's values:\n{text}");

        let mut game = game_spawning(text);
        let mut slotted = game.world_mut().query::<&Slotted>();
        let slotted = slotted.single(game.world()).expect("spawned");
        assert_eq!(slotted.level, 1);
        let Slot::Held(first) = &slotted.first else {
            panic!("the first variant");
        };
        assert_eq!(tone_of(&game, first), 1, "a tuple variant");
        let Slot::Named { swatch } = &slotted.second else {
            panic!("the second variant");
        };
        assert_eq!(tone_of(&game, swatch), 2, "a struct variant");

        let mut palette = game.world_mut().query::<&Palette>();
        let palette = palette.single(game.world()).expect("spawned");
        assert_eq!(palette.level, 1);
        assert_eq!(tone_of(&game, &palette.by_name["rock"]), 3, "a map value");

        let mut tones: Vec<u32> = game
            .world_mut()
            .query::<&Tinted>()
            .iter(game.world())
            .map(|tinted| tone_of(&game, &tinted.0))
            .collect();
        tones.sort_unstable();
        assert_eq!(tones, vec![4, 5], "a tuple component, written and bare");

        let mut coat = game.world_mut().query::<&Coat>();
        let Coat::Painted(painted) = coat.single(game.world()).expect("spawned") else {
            panic!("the variant");
        };
        assert_eq!(tone_of(&game, painted), 6, "an enum component");

        let mut bag = game.world_mut().query::<&Bag>();
        let bag = bag.single(game.world()).expect("spawned");
        assert_eq!(bag.level, 1);
        let item = bag.set.iter().next().expect("the item");
        assert_eq!(tone_of(&game, item), 6, "a set item");
    }

    /// `tab.path` is canonical and the project root is not: the tab still answers.
    #[cfg(unix)]
    #[test]
    fn a_tab_under_a_symlinked_root_answers_with_its_document() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let real = dunce::canonicalize(dir.path())
            .expect("canonical")
            .join("real");
        std::fs::create_dir_all(real.join("assets")).expect("assets");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("a link");

        let mut app = editor(&link);
        app.world_mut().resource_mut::<Scenes>().tabs[0].path = Some(real.join("assets/zone.bsn"));
        let answer = scene_answer(app.world_mut(), None, "zone.bsn");
        assert!(
            matches!(answer, SceneAnswer::Document { .. }),
            "got {answer:?}"
        );
        assert_eq!(
            open_scene_asset_path(app.world()).as_deref(),
            Some("zone.bsn"),
            "and it plays as the scene in front"
        );
    }

    #[test]
    fn forgetting_the_names_starts_over_at_stop() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut app = editor(dir.path());
        let color = swatch(&mut app, 4);
        add_node(
            &mut app,
            5,
            Painted {
                level: 1,
                swatch: color,
            },
            1,
        );
        answer(&mut app);
        assert_eq!(app.world().resource::<PlayAssetNames>().len(), 1);
        app.world_mut()
            .run_system_cached(forget_asset_names)
            .expect("it runs");
        assert!(app.world().resource::<PlayAssetNames>().is_empty());
    }
}
