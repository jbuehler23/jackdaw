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
//! The names are given on a copy of the document, in a frame's time. The text is
//! written from that copy and read back on a worker (`WriteJob`): text that does
//! not read never goes to a game, which loads the file and the user is told. The
//! game asks without blocking, so the answer can arrive frames later, and its place
//! in the instance's outbox is held from the moment of asking.
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
use jackdaw_pie_protocol::event::{from_bytes, to_bytes};
use jackdaw_pie_protocol::manifest::RunConfig;
use jackdaw_pie_protocol::{ControlEvent, SceneAnswer};

use crate::asset_files::asset_file_text;
use crate::pie::{InstanceKey, MAIN_SCENE};
use crate::play_overlay::{UnsavedPrefab, UnwrittenPrefab};
use crate::prefab::resolver_bsn::PREFAB_TYPE;
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

/// What `run` plays in place of the game's main scene, if anything: the scene a
/// project setting or the run configuration names in `JACKDAW_PIE_SCENE`, or else
/// the scene in front.
pub fn launch_scene(world: &World, run: &RunConfig) -> Option<LaunchScene> {
    let _span = info_span!("play.launch_scene").entered();
    let replaces =
        named_path(world, run, "JACKDAW_PIE_REPLACES").unwrap_or_else(|| MAIN_SCENE.to_string());
    let scene = match named_path(world, run, "JACKDAW_PIE_SCENE") {
        Some(named) if named == replaces => return None,
        Some(named) => PlayScene::Path(named),
        None => scene_in_front(world)?,
    };
    Some(LaunchScene { replaces, scene })
}

/// The asset path in the variable `key`, a project setting winning over the run
/// configuration as it does in the game's environment. Empty names nothing.
fn named_path(world: &World, run: &RunConfig, key: &str) -> Option<String> {
    world
        .get_resource::<crate::play_settings::PlaySettings>()
        .and_then(|settings| settings.env.get(key))
        .or_else(|| run.env.get(key))
        .filter(|path| !path.is_empty())
        .map(|path| path.replace('\\', "/"))
}

fn scene_in_front(world: &World) -> Option<PlayScene> {
    let scenes = world.get_resource::<Scenes>()?;
    let index = scenes.active;
    let tab = scenes.tabs.get(index)?;
    if tab.kind != TabKind::Scene || tab.is_refused() {
        return None;
    }
    match &tab.path {
        None if crate::scenes::operators::active_tab_modified(world) => {
            Some(PlayScene::Untitled(tab.display_name.clone()))
        }
        None => None,
        Some(_) => crate::pie::scene_asset_path_of_tab(world, index)
            .filter(|path| path != MAIN_SCENE)
            .map(PlayScene::Path),
    }
}

/// What the game spawns for a root asking for the asset path `requested`, for an
/// instance launched with `launch`. The document the path resolves to, if open in
/// a tab or an edited prefab in the cache, answers with its text; anything else
/// answers with the file. Waits for the text to be written and read back.
pub fn scene_answer(
    world: &mut World,
    launch: Option<&LaunchScene>,
    requested: &str,
) -> SceneAnswer {
    let answered = answer_with_assets(world, launch, requested);
    let prepared = prepare(world, None, answered, 0);
    let finished = write_now(world, None, prepared);
    decoded_answer(finished.answer, requested)
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
    let prepared = prepare_scene(world, key, launch, requested, 0);
    let finished = write_now(world, Some(key), prepared);
    ServedScene {
        files: finished.files,
        prefabs: finished.prefabs,
        answer: decoded_answer(finished.answer, requested),
    }
}

/// An answer written and finished on this thread, fallback included.
fn write_now(
    world: &mut World,
    key: Option<&InstanceKey>,
    mut prepared: PreparedScene,
) -> FinishedScene {
    loop {
        let written = prepared.job.write();
        match finish_scene(world, key, prepared.waiting, written, &mut []) {
            Finishing::Done(finished) => return finished,
            Finishing::Again(next) => prepared = next,
        }
    }
}

fn decoded_answer(encoded: Option<Vec<u8>>, requested: &str) -> SceneAnswer {
    match encoded.map(|bytes| from_bytes(&bytes)) {
        Some(Ok(ControlEvent::SceneDocument { answer, .. })) => answer,
        _ => SceneAnswer::UseFile {
            path: requested.to_string(),
            sidecars: None,
        },
    }
}

/// An answer on its way: what a worker writes, and what is done with the result.
pub(crate) struct PreparedScene {
    pub(crate) job: WriteJob,
    pub(crate) waiting: WaitingAnswer,
}

/// The first part of answering request `request` of the instance `key`: everything
/// that needs the world. The instance counts as holding every asset the text embeds
/// from here on, so an edit made before the answer goes out is sent behind it.
pub(crate) fn prepare_scene(
    world: &mut World,
    key: &InstanceKey,
    launch: Option<&LaunchScene>,
    requested: &str,
    request: u64,
) -> PreparedScene {
    let asked = AskedScene::resolved(world, launch, requested);
    world
        .get_resource_or_init::<PlayAssetNames>()
        .note_asked(key, asked);
    let answered = answer_with_assets(world, launch, requested);
    prepare(world, Some(key), answered, request)
}

fn prepare(
    world: &mut World,
    key: Option<&InstanceKey>,
    answered: Answered,
    request: u64,
) -> PreparedScene {
    let Answered {
        body,
        assets,
        own_names,
        prefabs,
        file,
        requested,
    } = answered;
    let mut files = None;
    let mut held = Vec::new();
    let mut before = HashMap::new();
    if let Some(key) = key {
        files = asset_files_missing_from(world, key, &assets.files);
        let mut names = world.get_resource_or_init::<PlayAssetNames>();
        for (name, definition) in &assets.embedded {
            let earlier = names.given_definition(key, name);
            before.entry(name.clone()).or_insert(earlier);
            names.note_given(key, name.clone(), *definition);
        }
        let reached = prefabs.iter().map(|prefab| prefab.path.clone()).collect();
        held = crate::play_overlay::unsaved_prefabs_beyond(world, key, &reached);
    }
    let prefab_names = prefabs
        .iter()
        .map(|prefab| (prefab.path.clone(), prefab.embedded.clone()))
        .collect();
    PreparedScene {
        job: WriteJob {
            request,
            body,
            prefabs,
            held,
        },
        waiting: WaitingAnswer {
            request,
            requested,
            file,
            files,
            before,
            own_names,
            prefab_names,
        },
    }
}

/// What an answer says, before its text exists.
enum AnswerBody {
    File(SceneAnswer),
    Document {
        ast: Box<SceneBsnAst>,
        dir: String,
        stem: String,
        sidecars: Option<String>,
    },
}

/// The part of an answer that does not need the world: writing each document as
/// text and reading the text back. Runs on a worker; frees the documents too.
pub(crate) struct WriteJob {
    request: u64,
    body: AnswerBody,
    prefabs: Vec<UnwrittenPrefab>,
    /// Unsaved prefabs a game holds that the scene no longer reaches, by path.
    held: Vec<(String, SceneBsnAst)>,
}

/// What a [`WriteJob`] made.
pub(crate) struct WrittenScene {
    /// The encoded [`ControlEvent::SceneDocument`], or why it cannot go.
    answer: Result<Vec<u8>, String>,
    prefabs: Vec<(String, Result<String, String>)>,
    /// Those of the held prefabs whose text does not read back.
    unreadable: Vec<String>,
}

impl WrittenScene {
    /// What stands for the result of a worker that panicked: nothing it was to
    /// write can go.
    pub(crate) fn lost(waiting: &WaitingAnswer) -> Self {
        let detail = || "the worker writing it panicked".to_string();
        Self {
            answer: Err(detail()),
            prefabs: waiting
                .prefab_names
                .iter()
                .map(|(path, _)| (path.clone(), Err(detail())))
                .collect(),
            unreadable: Vec::new(),
        }
    }
}

impl WriteJob {
    /// Whether there is nothing here worth a worker: no document to write.
    pub(crate) fn is_immediate(&self) -> bool {
        matches!(self.body, AnswerBody::File(_)) && self.prefabs.is_empty() && self.held.is_empty()
    }

    pub(crate) fn write(self) -> WrittenScene {
        let _span = info_span!("play.write_scene").entered();
        let request = self.request;
        let encode = |answer: SceneAnswer| {
            to_bytes(&ControlEvent::SceneDocument { request, answer })
                .map_err(|err| err.to_string())
        };
        let answer = match self.body {
            AnswerBody::File(answer) => encode(answer),
            AnswerBody::Document {
                ast,
                dir,
                stem,
                sidecars,
            } => text_read_back(&ast).and_then(|text| {
                encode(SceneAnswer::Document {
                    text,
                    dir,
                    stem,
                    sidecars,
                })
            }),
        };
        WrittenScene {
            answer,
            prefabs: self
                .prefabs
                .into_iter()
                .map(|prefab| {
                    let text =
                        text_read_back(&prefab.ast).map(|body| asset_file_text(PREFAB_TYPE, &body));
                    (prefab.path, text)
                })
                .collect(),
            unreadable: self
                .held
                .into_iter()
                .filter(|(_, ast)| text_read_back(ast).is_err())
                .map(|(path, _)| path)
                .collect(),
        }
    }
}

/// `ast`'s text, or why a reader would refuse it.
fn text_read_back(ast: &SceneBsnAst) -> Result<String, String> {
    let text = jackdaw_bsn::emit_scene(ast);
    jackdaw_bsn::parse_emitted(&text).map_err(|err| err.to_string())?;
    Ok(text)
}

/// What the last part of an answer needs from the first.
pub(crate) struct WaitingAnswer {
    request: u64,
    requested: String,
    /// The file the game loads if the document does not read back.
    file: Option<String>,
    files: Option<ControlEvent>,
    /// The `#Name` of each asset root the scene and its prefabs hold, with what the
    /// game held under it before this answer counted it as given.
    before: HashMap<String, Option<u64>>,
    /// Those the scene's own text holds.
    own_names: Vec<String>,
    /// Those each unsaved prefab's text holds, by path.
    prefab_names: Vec<(String, Vec<String>)>,
}

/// What is ready to send, in send order.
pub(crate) struct FinishedScene {
    pub(crate) files: Option<ControlEvent>,
    pub(crate) prefabs: Option<ControlEvent>,
    /// The encoded [`ControlEvent::SceneDocument`].
    pub(crate) answer: Option<Vec<u8>>,
}

/// Where an answer stands after [`finish_scene`].
pub(crate) enum Finishing {
    Done(FinishedScene),
    /// The document was refused, so this file answer takes its place.
    Again(PreparedScene),
}

/// What the status bar says when `path`'s unsaved edits cannot go to a game.
fn edits_not_sent(path: &str) -> String {
    format!("{path}: unsaved edits not sent, the game reads the saved file")
}

/// The last part of an answer, given what the worker wrote. Answers of one game
/// finish in the order they were asked for, so what is counted here is what the game
/// reads in that order; `later` holds that game's answers still to finish. A scene
/// whose text does not read back is refused: the game loads the file and the user
/// hears why. A prefab that does not read back leaves the game on that prefab's file.
pub(crate) fn finish_scene(
    world: &mut World,
    key: Option<&InstanceKey>,
    waiting: WaitingAnswer,
    written: WrittenScene,
    later: &mut [&mut WaitingAnswer],
) -> Finishing {
    let WaitingAnswer {
        request,
        requested,
        file,
        files,
        mut before,
        own_names,
        prefab_names,
    } = waiting;
    let WrittenScene {
        answer,
        prefabs,
        unreadable,
    } = written;
    let answer = match (answer, file) {
        (Ok(bytes), _) => Some(bytes),
        (Err(detail), Some(file)) => {
            warn!("PIE: the document for {requested} does not read back, using its file: {detail}");
            crate::status_bar::notify_error(world, edits_not_sent(&requested));
            if let Some(key) = key {
                let names: Vec<String> = before.keys().cloned().collect();
                give_back(world, key, &names, &mut before, later);
            }
            let fallback = Answered::use_file(world, file);
            let mut prepared = prepare(world, key, fallback, request);
            prepared.waiting.files = merged_files(files, prepared.waiting.files.take());
            return Finishing::Again(prepared);
        }
        (Err(detail), None) => {
            error!("PIE: could not answer for {requested}: {detail}");
            crate::status_bar::notify_error(world, edits_not_sent(&requested));
            to_bytes(&ControlEvent::SceneDocument {
                request,
                answer: SceneAnswer::UseFile {
                    path: requested.clone(),
                    sidecars: None,
                },
            })
            .ok()
        }
    };

    let mut unsaved = Vec::new();
    let mut refused = Vec::new();
    for (path, text) in prefabs {
        match text {
            Ok(text) => unsaved.push(UnsavedPrefab { path, text }),
            Err(detail) => {
                warn!("PIE: the unsaved prefab {path} does not read back: {detail}");
                crate::status_bar::notify_warn(world, edits_not_sent(&path));
                refused.push(path);
            }
        }
    }
    let Some(key) = key else {
        return Finishing::Done(FinishedScene {
            files,
            prefabs: None,
            answer,
        });
    };
    if !refused.is_empty() {
        let stays = |name: &String| {
            own_names.contains(name)
                || prefab_names
                    .iter()
                    .any(|(path, names)| !refused.contains(path) && names.contains(name))
        };
        let gone: Vec<String> = prefab_names
            .iter()
            .filter(|(path, _)| refused.contains(path))
            .flat_map(|(_, names)| names.iter().filter(|name| !stays(name)).cloned())
            .collect();
        give_back(world, key, &gone, &mut before, later);
    }
    let reached = prefab_names.into_iter().map(|(path, _)| path).collect();
    let saved = crate::play_overlay::saved_prefabs_beyond(world, key, &reached);
    let to_files = saved.into_iter().chain(unreadable).chain(refused).collect();
    let prefabs = crate::play_overlay::prefab_files_missing_from(world, key, unsaved, to_files);
    Finishing::Done(FinishedScene {
        files,
        prefabs,
        answer,
    })
}

/// `names` never reached the game with this answer. Each goes back to what the game
/// held before it, unless a later answer still on its way holds it: then that answer
/// goes back to it, if it never reaches the game either.
fn give_back(
    world: &mut World,
    key: &InstanceKey,
    names: &[String],
    before: &mut HashMap<String, Option<u64>>,
    later: &mut [&mut WaitingAnswer],
) {
    let mut held = world.get_resource_or_init::<PlayAssetNames>();
    for name in names {
        let Some(earlier) = before.remove(name) else {
            continue;
        };
        match later
            .iter_mut()
            .find(|answer| answer.before.contains_key(name))
        {
            Some(answer) => {
                answer.before.insert(name.clone(), earlier);
            }
            None => held.restore_given(key, name, earlier),
        }
    }
}

/// Two sets of asset files as one message.
fn merged_files(first: Option<ControlEvent>, second: Option<ControlEvent>) -> Option<ControlEvent> {
    match (first, second) {
        (
            Some(ControlEvent::AssetFiles { mut files }),
            Some(ControlEvent::AssetFiles { files: more }),
        ) => {
            files.extend(more);
            Some(ControlEvent::AssetFiles { files })
        }
        (first, second) => first.or(second),
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
    /// What `ast`, under session names, holds or names.
    pub(crate) fn of(world: &World, ast: &mut SceneBsnAst) -> Self {
        Self {
            embedded: embedded_assets(world, ast),
            files: asset_files_named(world, ast),
        }
    }
}

/// An answer and what goes with it.
struct Answered {
    body: AnswerBody,
    /// For a document, what its text and its prefabs' texts hold or name.
    assets: DocumentAssets,
    /// The `#Name` of each asset root its own text holds.
    own_names: Vec<String>,
    /// The unsaved prefabs the scene reaches.
    prefabs: Vec<UnwrittenPrefab>,
    /// The file to fall back on, for a document.
    file: Option<String>,
    requested: String,
}

impl Answered {
    /// The file answer for `path`, with what the file reaches beyond itself.
    fn use_file(world: &mut World, path: String) -> Self {
        let overlay = crate::play_overlay::overlay_of_file(world, &path);
        Self {
            requested: path.clone(),
            body: AnswerBody::File(SceneAnswer::UseFile {
                path,
                sidecars: overlay.sidecars,
            }),
            assets: overlay.assets,
            own_names: Vec::new(),
            prefabs: overlay.prefabs,
            file: None,
        }
    }
}

/// The answer and what goes with it.
fn answer_with_assets(
    world: &mut World,
    launch: Option<&LaunchScene>,
    requested: &str,
) -> Answered {
    let AskedScene {
        requested,
        target,
        tab,
        ..
    } = AskedScene::resolved(world, launch, requested);
    let tab = tab.and_then(|id| {
        world
            .get_resource::<Scenes>()?
            .tabs
            .iter()
            .position(|held| held.id == id)
    });
    let served = tab.and_then(|tab| tab_asset_path(world, tab)).map_or_else(
        || ServedAs::of_target(&target, &requested),
        |path| ServedAs::of(&path, None),
    );
    let file = match &target {
        PlayScene::Path(path) => path.clone(),
        PlayScene::Untitled(_) => requested.clone(),
    };

    let Some(assets) = crate::asset_index::assets_dir(world) else {
        return Answered::use_file(world, file);
    };
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
    warn_absolute(world, &document.absolute);
    let mut ast = with_session_names(world, document);
    let mut held = DocumentAssets::of(world, &mut ast);
    let own_names = held.embedded.iter().map(|(name, _)| name.clone()).collect();
    let overlay =
        crate::play_overlay::overlay_of_document(world, tab, &target, &ast, &parent, &served.dir);
    held.embedded.extend(overlay.assets.embedded);
    held.files.extend(overlay.assets.files);
    Answered {
        body: AnswerBody::Document {
            ast: Box::new(ast),
            dir: served.dir,
            stem: served.stem,
            sidecars: overlay.sidecars,
        },
        assets: held,
        own_names,
        prefabs: overlay.prefabs,
        file: Some(file),
        requested,
    }
}

/// `document` with every embedded asset under its session name. A root and every
/// reference to it are renamed together, including the `@Name` spelling a load also
/// answers to, whatever wrote them. A root whose asset the live stores hold is
/// written from the store, which an edit reads too.
pub(crate) fn with_session_names(world: &mut World, document: GameDocument) -> SceneBsnAst {
    let GameDocument {
        mut ast,
        live_assets,
        ..
    } = document;
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
            let live = live_assets
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
    ast.drop_unreachable();
    ast
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
    let (type_path, value) = jackdaw_bsn::asset_value_from_root(ast, root)?;
    let mut hasher = DefaultHasher::new();
    (type_path, jackdaw_bsn::emit_value_text(&value)).hash(&mut hasher);
    Some(hasher.finish())
}

/// A digest of the asset `id` as a document would embed it, which compares with
/// [`root_definition`] of a served document. `None` when the asset cannot be
/// embedded.
fn asset_definition(world: &World, id: UntypedAssetId, type_id: TypeId) -> Option<u64> {
    let asset = CatalogAssetRef {
        name: "Asset".to_string(),
        type_id,
        asset_id: id,
    };
    let mut ast = SceneBsnAst::default();
    if !jackdaw_bsn::append_assets_to_ast(&mut ast, world, &[asset]).is_empty() {
        return None;
    }
    root_definition(&ast, *ast.roots.first()?)
}

/// The folder and name a document goes by in the game.
struct ServedAs {
    dir: String,
    stem: String,
}

impl ServedAs {
    fn of_target(target: &PlayScene, requested: &str) -> Self {
        match target {
            PlayScene::Path(path) => Self::of(path, None),
            PlayScene::Untitled(name) => Self::of(requested, Some(name.clone())),
        }
    }

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

/// A scene a game asked for, whichever way it was answered.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct AskedScene {
    /// The asset path the game knows it by.
    pub(crate) requested: String,
    /// What that path stands for.
    pub(crate) target: PlayScene,
    /// The folder it counts as being in, under the asset folder.
    pub(crate) dir: String,
    /// The tab holding it when it was asked for. A save can give that tab another
    /// path and the game goes on playing it.
    tab: Option<u64>,
}

impl AskedScene {
    fn of(launch: Option<&LaunchScene>, requested: &str) -> Self {
        let requested = requested.replace('\\', "/");
        let target = match launch {
            Some(launch) if launch.replaces == requested => launch.scene.clone(),
            _ => PlayScene::Path(requested.clone()),
        };
        let dir = ServedAs::of_target(&target, &requested).dir;
        Self {
            requested,
            target,
            dir,
            tab: None,
        }
    }

    /// What `requested` stands for now. A tab holding the target decides, and failing
    /// that the tab a game asked for it from earlier, since a save can have given that
    /// tab another path or a first one. An unsaved tab saved before any game asked
    /// stays unknown, and the game loads the file.
    fn resolved(world: &World, launch: Option<&LaunchScene>, requested: &str) -> Self {
        let mut asked = Self::of(launch, requested);
        let Some(scenes) = world.get_resource::<Scenes>() else {
            return asked;
        };
        asked.tab = tab_of(world, &asked.target)
            .map(|tab| scenes.tabs[tab].id)
            .or_else(|| {
                let names = world.get_resource::<PlayAssetNames>()?;
                names
                    .asked
                    .values()
                    .flatten()
                    .filter(|earlier| {
                        earlier.requested == asked.requested && earlier.target == asked.target
                    })
                    .find_map(|earlier| earlier.tab)
                    .filter(|id| scenes.tabs.iter().any(|tab| tab.id == *id))
            });
        asked
    }
}

/// The path under the asset folder of the file tab `tab` holds, if it has one.
#[cfg(test)]
pub(crate) fn note_test_asked_scene(world: &mut World, key: &InstanceKey, requested: &str) {
    let asked = AskedScene::resolved(world, None, requested);
    world
        .resource_mut::<PlayAssetNames>()
        .note_asked(key, asked);
}

pub(crate) fn tab_asset_path(world: &World, tab: usize) -> Option<String> {
    let path = world.get_resource::<Scenes>()?.tabs[tab].path.clone()?;
    world
        .get_resource::<crate::project::ProjectRoot>()?
        .to_asset_path(path)
}

/// The scenes the game `key` asked for that the document in front is, each with the
/// folder the document counts as being in now. The tab it was asked from decides
/// while that tab is open, and otherwise the path or name asked for.
pub(crate) fn scenes_playing_active_tab(world: &World, key: &InstanceKey) -> Vec<AskedScene> {
    let (Some(names), Some(scenes)) = (
        world.get_resource::<PlayAssetNames>(),
        world.get_resource::<Scenes>(),
    ) else {
        return Vec::new();
    };
    let Some(active) = scenes.tabs.get(scenes.active) else {
        return Vec::new();
    };
    let held_dir = tab_asset_path(world, scenes.active).map(|path| ServedAs::of(&path, None).dir);
    names
        .asked
        .get(key)
        .into_iter()
        .flatten()
        .filter(|asked| {
            match asked
                .tab
                .filter(|id| scenes.tabs.iter().any(|tab| tab.id == *id))
            {
                Some(id) => id == active.id,
                None => tab_of(world, &asked.target) == Some(scenes.active),
            }
        })
        .map(|asked| AskedScene {
            dir: held_dir.clone().unwrap_or_else(|| asked.dir.clone()),
            ..asked.clone()
        })
        .collect()
}

/// New subtrees of the live document as a game takes them.
pub(crate) struct SubtreesForGame {
    /// The text of each subtree, by the node at its top.
    pub(crate) texts: HashMap<u64, String>,
    /// The `#Name`, definition and text of each asset root the texts name.
    pub(crate) assets: Vec<(String, u64, String)>,
    /// The asset files they name.
    pub(crate) files: Vec<AssetFile>,
    /// The unsaved prefabs they reach.
    pub(crate) prefabs: Vec<UnsavedPrefab>,
    /// The `#Name` and definition of each asset root those prefabs' texts hold.
    pub(crate) prefab_assets: Vec<(String, u64)>,
}

/// The subtrees of the live document at `spawns`, as `(node id, document node)`,
/// under session names and spelled as `asked`'s text spells them. A child in
/// `existing` is left out: it exists in the game and moves there on its own. One
/// authoring pass covers them all, so its cost follows the nodes spawned and not the
/// document. Unsaved sidecars of a new node arrive on a respawn. The texts are not
/// read back here: the game parses them anyway and reports one it cannot.
pub(crate) fn subtrees_for_game(
    world: &mut World,
    spawns: &[(u64, Entity)],
    existing: &bevy::platform::collections::HashSet<u64>,
    asked: &AskedScene,
) -> Option<SubtreesForGame> {
    let _span = info_span!("play.subtrees").entered();
    let assets = crate::asset_index::assets_dir(world)?;
    let tab = world.resource::<Scenes>().active;
    let parent = tab_folder(world, tab, &assets, &asked.dir);
    let registry = world.resource::<AppTypeRegistry>().clone();
    let mut part = SceneBsnAst::default();
    let copies: Vec<(u64, Entity)> = {
        let live = world.resource::<SceneBsnAst>();
        for root in jackdaw_bsn::asset_roots(live, &registry.read()) {
            jackdaw_bsn::clone_subtree_into(&mut part, live, root, None);
        }
        spawns
            .iter()
            .map(|&(id, node)| {
                (
                    id,
                    copy_new_nodes(&mut part, live, node, None, existing, false),
                )
            })
            .collect()
    };
    let mut document = crate::scene_io::save::part_of_live_document_for_game(world, part, &parent);
    warn_absolute(world, &document.absolute);
    keep_assets_named(&mut document.ast, &registry.read());
    let mut ast = with_session_names(world, document);
    let held = DocumentAssets::of(world, &mut ast);
    let overlay = crate::play_overlay::prefabs_of_document_part(
        world,
        tab,
        &asked.target,
        &ast,
        &parent,
        &asked.dir,
    );
    let texts = copies
        .into_iter()
        .map(|(id, copy)| (id, jackdaw_bsn::emit_entity(&ast, copy)))
        .collect();
    let roots = {
        let registry = registry.read();
        jackdaw_bsn::asset_roots(&ast, &registry)
    };
    let assets = held
        .embedded
        .into_iter()
        .filter_map(|(name, definition)| {
            let root = roots
                .iter()
                .copied()
                .find(|&root| ast.get_name(root) == Some(name.trim_start_matches('#')))?;
            let mut alone = SceneBsnAst::default();
            jackdaw_bsn::clone_subtree_into(&mut alone, &ast, root, None);
            Some((name, definition, jackdaw_bsn::emit_scene(&alone)))
        })
        .collect();
    let mut files = held.files;
    files.extend(overlay.assets.files);
    let mut prefabs = Vec::new();
    for prefab in overlay.prefabs {
        match text_read_back(&prefab.ast) {
            Ok(body) => prefabs.push(UnsavedPrefab {
                path: prefab.path,
                text: asset_file_text(PREFAB_TYPE, &body),
            }),
            Err(detail) => {
                warn!(
                    "PIE: the unsaved prefab {} does not read back: {detail}",
                    prefab.path
                );
                crate::status_bar::notify_warn(world, edits_not_sent(&prefab.path));
            }
        }
    }
    Some(SubtreesForGame {
        texts,
        assets,
        files,
        prefabs,
        prefab_assets: overlay.assets.embedded,
    })
}

/// Copy `node` into `part`, linked to its entity, with the children `existing` does
/// not hold. Everything below a prefab instance belongs to it (`whole`), whatever id
/// it repeats.
fn copy_new_nodes(
    part: &mut SceneBsnAst,
    live: &SceneBsnAst,
    node: Entity,
    parent: Option<Entity>,
    existing: &bevy::platform::collections::HashSet<u64>,
    whole: bool,
) -> Entity {
    let copy = part.create_entity_node(live.cloned_component_patches(node));
    match parent {
        Some(parent) => part.add_child_to_ast(parent, copy),
        None => part.add_to_roots(copy),
    }
    if let Some(entity) = live.ecs_for_ast(node) {
        part.link(entity, copy);
    }
    let whole = whole || crate::document_units::is_instance(live, node);
    for child in live.get_children_ast(node) {
        let exists = live
            .stable_id_of(child)
            .is_some_and(|id| existing.contains(&id));
        if whole || !exists {
            copy_new_nodes(part, live, child, Some(copy), existing, whole);
        }
    }
    copy
}

/// Drop the asset roots of `ast` that nothing in it names.
fn keep_assets_named(ast: &mut SceneBsnAst, registry: &bevy::reflect::TypeRegistry) {
    let mut named: HashSet<String> = HashSet::new();
    crate::scene_io::save::visit_asset_references(ast, registry, |reference| {
        if let Some(bare) = reference.strip_prefix(['#', '@']) {
            named.insert(bare.to_string());
        }
    });
    let unused: Vec<Entity> = jackdaw_bsn::asset_roots(ast, registry)
        .into_iter()
        .filter(|&root| ast.get_name(root).is_none_or(|name| !named.contains(name)))
        .collect();
    ast.roots.retain(|root| !unused.contains(root));
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

/// The document in tab `index`, authored for a game: the live one
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
        return Some(crate::scene_io::save::live_document_for_game(world, parent));
    }
    match &scenes.tabs[index].content {
        TabContent::Scene(Some(document)) => Some(crate::scene_io::save::snapshot_for_game(
            world, document, parent,
        )),
        TabContent::Prefab(path) => world
            .get_resource::<crate::prefab::PrefabAstCache>()?
            .get(path.as_path())
            .map(|document| crate::scene_io::save::snapshot_for_game(world, document, parent)),
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
    /// The scenes each game asked for, so structural changes go only to a game
    /// playing the document they were made in.
    asked: HashMap<InstanceKey, Vec<AskedScene>>,
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
    pub fn was_given(&self, key: &InstanceKey, name: &str) -> bool {
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

    /// The definition the game `key` holds under `name`.
    pub(crate) fn given_definition(&self, key: &InstanceKey, name: &str) -> Option<u64> {
        self.given.get(key)?.get(name).copied()
    }

    /// What the game `key` was to get under `name` never reached it, so it holds
    /// `earlier` there.
    pub(crate) fn restore_given(&mut self, key: &InstanceKey, name: &str, earlier: Option<u64>) {
        let Some(given) = self.given.get_mut(key) else {
            return;
        };
        match earlier {
            Some(definition) => {
                given.insert(name.to_string(), definition);
            }
            None => {
                given.remove(name);
            }
        }
    }

    fn note_asked(&mut self, key: &InstanceKey, asked: AskedScene) {
        let held = self.asked.entry(key.clone()).or_default();
        held.retain(|earlier| earlier.requested != asked.requested);
        held.push(asked);
    }

    /// Keep what games were given only for those `keep` accepts.
    pub(crate) fn retain_games(&mut self, keep: impl Fn(&InstanceKey) -> bool) {
        self.given.retain(|key, _| keep(key));
        self.given_prefabs.retain(|key, _| keep(key));
        self.asked.retain(|key, _| keep(key));
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
    use jackdaw_runtime::JackdawPlugin;
    use jackdaw_scene_types::SceneNodeId;

    use super::*;
    use crate::play_sync::PlayReferences;

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
            PlayReferences::with(app.world(), app.world().resource::<PlayAssetNames>())
                .reference(&late.untyped()),
            Some(name),
            "live sync sends the same name"
        );
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
            crate::pie::scene_asset_path_of_tab(
                app.world(),
                app.world().resource::<Scenes>().active
            )
            .as_deref(),
            Some("zone.bsn"),
            "and it plays as the scene in front"
        );
    }

    /// A key for tests that prepare answers by hand.
    fn game_key() -> InstanceKey {
        InstanceKey {
            config: "game".to_string(),
            instance: 1,
        }
    }

    fn unreadable_root(app: &mut App) -> Entity {
        let mut ast = app.world_mut().resource_mut::<SceneBsnAst>();
        let node = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Struct(
            jackdaw_bsn::BsnStructData {
                type_path: String::new(),
                fields: jackdaw_bsn::BsnStructFields(vec![jackdaw_bsn::BsnField {
                    name: "0".into(),
                    value: jackdaw_bsn::BsnValue::Int(1),
                }]),
            },
        )]);
        ast.add_to_roots(node);
        node
    }

    /// An earlier answer refused while a later one holds the same name leaves the
    /// name given, and if the later one is refused too the game holds nothing.
    #[test]
    fn a_refusal_leaves_a_name_a_later_answer_holds() {
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
        unreadable_root(&mut app);
        let key = game_key();
        let world = app.world_mut();
        let first = prepare_scene(world, &key, None, "zone.bsn", 1);
        let mut second = prepare_scene(world, &key, None, "zone.bsn", 2);
        let name = second
            .waiting
            .before
            .keys()
            .next()
            .expect("the answer holds a name")
            .clone();

        let written = first.job.write();
        let refused = finish_scene(
            world,
            Some(&key),
            first.waiting,
            written,
            &mut [&mut second.waiting],
        );
        assert!(matches!(refused, Finishing::Again(_)));
        assert!(
            world.resource::<PlayAssetNames>().was_given(&key, &name),
            "the later answer still holds it"
        );

        let written = second.job.write();
        finish_scene(world, Some(&key), second.waiting, written, &mut []);
        assert!(
            !world.resource::<PlayAssetNames>().was_given(&key, &name),
            "neither answer reached the game"
        );
    }

    /// `depth` nodes, each the only child of the one before, the last named `Leaf`.
    fn nested_nodes(depth: usize) -> String {
        let mut text = "#Leaf\nbevy_transform::components::transform::Transform\n".to_string();
        for _ in 1..depth {
            text = format!("#Node\nbevy_ecs::hierarchy::Children [\n{text}]\n");
        }
        text
    }

    /// A component holding lists nested `depth` deep.
    fn nested_values(depth: usize) -> String {
        format!(
            "#Node\ngame::Deep {{ value: {}{} }}\n",
            "[".repeat(depth),
            "]".repeat(depth)
        )
    }

    fn reads_back_at_the_depth_limit() {
        let text = nested_nodes(jackdaw_bsn::MAX_AST_DEPTH);
        let ast = jackdaw_bsn::parse_bsn_text(&text).expect("the nodes parse");
        let emitted = text_read_back(&ast).expect("the nodes read back");
        assert!(emitted.contains("#Leaf"), "every level was written");
        let text = nested_values(jackdaw_bsn::MAX_AST_DEPTH);
        let ast = jackdaw_bsn::parse_bsn_text(&text).expect("the values parse");
        text_read_back(&ast).expect("the values read back");
    }

    /// The deepest document the emitter writes, written and read back on a worker,
    /// whose stack is smaller than the main thread's.
    #[test]
    fn a_document_at_the_depth_limit_reads_back_on_a_worker() {
        let pool = bevy::tasks::AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        bevy::tasks::block_on(pool.spawn(async { reads_back_at_the_depth_limit() }));
    }

    /// And with half a worker's stack to spare.
    #[test]
    fn a_document_at_the_depth_limit_fits_half_a_worker_stack() {
        std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(reads_back_at_the_depth_limit)
            .expect("a thread")
            .join()
            .expect("it read back");
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
