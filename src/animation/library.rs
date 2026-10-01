//! Every animation clip the project's glTF files hold, indexed once per file.
//!
//! A clip used to arrive as a document entity spawned under the model that
//! carried it, which meant a scene saved a list of what its files happened to
//! contain and a game loading that scene read components it has no use for.
//! The same answer is editor state instead: the library is built by reading
//! each file's glTF JSON once, and nothing about it reaches a document.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::tasks::{IoTaskPool, Task, block_on, poll_once};
use path_slash::PathExt as _;

/// Deepest directory tree the library walks, matching the asset listing the
/// remote serves.
const MAX_LIBRARY_DEPTH: usize = 12;

/// The magic number a binary glTF file starts with, `glTF` read little-endian.
const GLB_MAGIC: u32 = 0x4654_6C67;
/// The type of the JSON chunk that follows a binary glTF header, `JSON`.
const GLB_JSON_CHUNK: u32 = 0x4E4F_534A;

/// One clip a glTF file holds.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryClip {
    /// The name the clip carries in its file.
    pub name: String,
    /// How long it runs.
    pub duration_secs: f32,
    /// Whether the name says it was exported to loop.
    pub looped_hint: bool,
}

/// One glTF file, and the clips it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryFile {
    /// Assets-relative path of the file.
    pub path: String,
    /// Its clips, in the order the file lists them.
    pub clips: Vec<LibraryClip>,
}

/// Every glTF clip the editor has found, by assets-relative file path.
///
/// Only files that hold at least one clip are listed. Ordered by path so the
/// panel draws the same list twice running.
#[derive(Resource, Default, Debug)]
pub struct AnimationLibrary {
    files: BTreeMap<String, LibraryFile>,
}

impl AnimationLibrary {
    /// Every indexed file, by path.
    pub fn files(&self) -> impl Iterator<Item = &LibraryFile> {
        self.files.values()
    }

    /// How many files hold clips.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether nothing has been indexed yet.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// One file's clips, when it holds any.
    pub fn file(&self, path: &str) -> Option<&LibraryFile> {
        self.files.get(path)
    }

    /// One named clip of one file.
    pub fn clip(&self, path: &str, clip: &str) -> Option<&LibraryClip> {
        self.file(path)?.clips.iter().find(|it| it.name == clip)
    }

    /// Record what a file holds. Empty clip lists are not kept: the library
    /// answers "which files have animation in them".
    fn insert(&mut self, path: String, clips: Vec<LibraryClip>) {
        if clips.is_empty() {
            self.files.remove(&path);
            return;
        }
        self.files.insert(path.clone(), LibraryFile { path, clips });
    }
}

/// Whether a clip's name says it was exported to loop.
pub(super) fn looped_hint(name: &str) -> bool {
    name.ends_with("_Loop") || name.ends_with("Loop")
}

/// The clips a glTF file lists, in the order it lists them.
///
/// Read from the file's JSON alone, so asking costs no mesh, image or clip
/// load. A clip runs until its latest keyframe, which the glTF spec has every
/// animation input accessor record as its `max`.
fn clips_in_gltf(bytes: &[u8]) -> Vec<LibraryClip> {
    let Some(document) = gltf_json(bytes) else {
        return Vec::new();
    };
    let accessors = document["accessors"].as_array();
    let input_end = |sampler: &serde_json::Value| {
        let accessor = accessors?.get(usize::try_from(sampler["input"].as_u64()?).ok()?)?;
        accessor["max"].as_array()?.first()?.as_f64()
    };
    document["animations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|animation| {
            let name = animation["name"].as_str()?;
            let duration = animation["samplers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(input_end)
                .fold(0.0_f64, f64::max);
            Some(LibraryClip {
                name: name.to_string(),
                duration_secs: duration as f32,
                looped_hint: looped_hint(name),
            })
        })
        .collect()
}

/// The JSON document of a `.gltf` file, or the JSON chunk of a `.glb` one.
fn gltf_json(bytes: &[u8]) -> Option<serde_json::Value> {
    let word = |at: usize| {
        let four = bytes.get(at..at + 4)?;
        Some(u32::from_le_bytes(four.try_into().ok()?))
    };
    if word(0) != Some(GLB_MAGIC) {
        return serde_json::from_slice(bytes).ok();
    }
    let length = usize::try_from(word(12)?).ok()?;
    if word(16)? != GLB_JSON_CHUNK {
        return None;
    }
    serde_json::from_slice(bytes.get(20..20 + length)?).ok()
}

/// Whether a path names a glTF file.
fn is_gltf(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".glb") || lower.ends_with(".gltf")
}

/// How far the library has got through the files it means to ask.
///
/// One file is read at a time, on the IO pool.
#[derive(Resource, Default)]
struct LibraryScan {
    /// Files found but not yet asked.
    queue: VecDeque<String>,
    /// Files already queued, so nothing is asked about twice.
    asked: HashSet<String>,
    /// The file being read, and the read that answers it.
    pending: Option<(String, Task<Vec<LibraryClip>>)>,
    /// Directories left to walk, with their depth below the assets root.
    dirs: VecDeque<(PathBuf, usize)>,
    /// The project the walk was seeded from.
    walking: Option<PathBuf>,
}

impl LibraryScan {
    fn want(&mut self, path: &str) {
        if !is_gltf(path) || self.asked.contains(path) {
            return;
        }
        self.asked.insert(path.to_string());
        self.queue.push_back(path.to_string());
    }

    /// Start over for another project.
    fn reseed(&mut self, assets_dir: PathBuf) {
        *self = Self {
            dirs: VecDeque::from([(assets_dir.clone(), 0)]),
            walking: Some(assets_dir),
            ..default()
        };
    }
}

/// Who is asking for the whole project's clips rather than the open scene's.
///
/// The walk over the assets directory reads every glTF it finds, so it runs
/// only while the Library tab is showing or a remote listing asked for clip
/// details; the open scene's own sources are indexed regardless.
#[derive(Resource, Default, Debug)]
pub struct LibraryDemand {
    /// The Library tab is on screen.
    pub panel: bool,
    /// A remote listing asked for clip details.
    pub requested: bool,
}

impl LibraryDemand {
    fn project_walk(&self) -> bool {
        self.panel || self.requested
    }
}

/// Ask one more file, and walk one more directory, per frame.
///
/// Spread over frames rather than done at once: a project's assets directory
/// is of unknown size, and every answer costs a file read. The open scene's
/// sources are read when they change, and all of them again after a reseed.
fn index_animation_library(
    project: Option<Res<crate::project::ProjectRoot>>,
    sources: Query<Ref<jackdaw_scene_types::GltfSource>>,
    sets: Query<Ref<jackdaw_animation_runtime::AnimationSet>>,
    demand: Res<LibraryDemand>,
    mut scan: ResMut<LibraryScan>,
    mut library: ResMut<AnimationLibrary>,
) {
    let mut reseeded = false;
    if let Some(project) = project.as_deref() {
        let assets_dir = project.assets_dir();
        if scan.walking.as_deref() != Some(assets_dir.as_path()) {
            scan.reseed(assets_dir);
            *library = AnimationLibrary::default();
            reseeded = true;
        }
    }

    // What the open scene points at comes first: those files are the ones an
    // author is looking at.
    for source in &sources {
        if reseeded || source.is_changed() {
            scan.want(&source.path);
        }
    }
    for set in &sets {
        if reseeded || set.is_changed() {
            for path in &set.sources {
                scan.want(path);
            }
        }
    }

    if demand.project_walk() {
        walk_one_directory(&mut scan);
    }
    ask_one_file(&mut scan, &mut library);
}

/// Read one directory and queue the glTF files in it.
fn walk_one_directory(scan: &mut LibraryScan) {
    let Some((dir, depth)) = scan.dirs.pop_front() else {
        return;
    };
    let Some(root) = scan.walking.clone() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        // A link back up the tree would recurse, and one pointing out of the
        // assets directory would index files the project does not ship.
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            if depth < MAX_LIBRARY_DEPTH {
                scan.dirs.push_back((path, depth + 1));
            }
            continue;
        }
        let Ok(relative) = path.strip_prefix(&root) else {
            continue;
        };
        scan.want(&relative.to_slash_lossy());
    }
}

/// Move the one file being asked about along, and start the next when it is
/// answered.
fn ask_one_file(scan: &mut LibraryScan, library: &mut AnimationLibrary) {
    if scan.pending.is_none()
        && let Some(path) = scan.queue.pop_front()
        && let Some(root) = scan.walking.clone()
    {
        let file = root.join(crate::entity_ops::to_asset_path(&path));
        let task = IoTaskPool::get().spawn(async move { read_clips(&file) });
        scan.pending = Some((path, task));
    }
    let Some((_, task)) = scan.pending.as_mut() else {
        return;
    };
    let Some(clips) = block_on(poll_once(task)) else {
        return;
    };
    if let Some((path, _)) = scan.pending.take() {
        library.insert(path, clips);
    }
}

/// The clips a file on disk lists; none when it cannot be read.
fn read_clips(file: &Path) -> Vec<LibraryClip> {
    std::fs::read(file)
        .map(|bytes| clips_in_gltf(&bytes))
        .unwrap_or_default()
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<LibraryDemand>();
    app.init_resource::<AnimationLibrary>()
        .init_resource::<LibraryScan>()
        .add_systems(
            Update,
            index_animation_library.run_if(in_state(crate::AppState::Editor)),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_ending_in_loop_is_hinted_as_looping() {
        assert!(looped_hint("Jog_Fwd_Loop"));
        assert!(looped_hint("IdleLoop"));
        assert!(!looped_hint("Punch_Jab"));
        assert!(!looped_hint("Looping_Start"));
    }

    const TWO_CLIPS: &str = r#"{
        "asset": {"version": "2.0"},
        "accessors": [
            {"count": 2, "type": "SCALAR", "componentType": 5126, "min": [0.0], "max": [1.5]},
            {"count": 3, "type": "SCALAR", "componentType": 5126, "min": [0.0], "max": [2.25]},
            {"count": 2, "type": "SCALAR", "componentType": 5126, "min": [0.0], "max": [0.5]}
        ],
        "animations": [
            {"name": "Walk_Loop", "channels": [], "samplers": [{"input": 0, "output": 2}, {"input": 1, "output": 2}]},
            {"channels": [], "samplers": [{"input": 2, "output": 2}]},
            {"name": "Punch", "channels": [], "samplers": [{"input": 2, "output": 0}]}
        ]
    }"#;

    fn glb(json: &str) -> Vec<u8> {
        let mut chunk = json.as_bytes().to_vec();
        while !chunk.len().is_multiple_of(4) {
            chunk.push(b' ');
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&GLB_MAGIC.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&((20 + chunk.len()) as u32).to_le_bytes());
        bytes.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&GLB_JSON_CHUNK.to_le_bytes());
        bytes.extend_from_slice(&chunk);
        bytes
    }

    #[test]
    fn a_gltf_lists_its_named_clips_in_order_with_their_last_keyframe() {
        let clips = clips_in_gltf(TWO_CLIPS.as_bytes());
        assert_eq!(
            clips,
            vec![
                LibraryClip {
                    name: "Walk_Loop".into(),
                    duration_secs: 2.25,
                    looped_hint: true,
                },
                LibraryClip {
                    name: "Punch".into(),
                    duration_secs: 0.5,
                    looped_hint: false,
                },
            ]
        );
    }

    #[test]
    fn a_binary_gltf_lists_the_clips_its_json_chunk_names() {
        assert_eq!(
            clips_in_gltf(&glb(TWO_CLIPS)),
            clips_in_gltf(TWO_CLIPS.as_bytes())
        );
    }

    #[test]
    fn a_file_that_is_not_gltf_lists_no_clips() {
        assert!(clips_in_gltf(b"not a model").is_empty());
    }

    #[test]
    fn a_file_with_no_clips_is_not_listed() {
        let mut library = AnimationLibrary::default();
        library.insert("models/rock.glb".into(), Vec::new());
        assert!(library.is_empty());
    }

    fn library_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<LibraryDemand>()
            .init_resource::<AnimationLibrary>()
            .init_resource::<LibraryScan>()
            .add_systems(Update, index_animation_library);
        app.update();
        app
    }

    fn asked(app: &App, path: &str) -> bool {
        app.world().resource::<LibraryScan>().asked.contains(path)
    }

    fn source(path: &str) -> jackdaw_scene_types::GltfSource {
        jackdaw_scene_types::GltfSource {
            path: path.into(),
            scene_index: 0,
        }
    }

    #[test]
    fn sources_placed_or_repointed_after_the_first_pass_are_asked_about() {
        let mut app = library_app();
        let model = app.world_mut().spawn(source("models/crate.glb")).id();
        app.update();
        assert!(asked(&app, "models/crate.glb"));

        app.world_mut()
            .get_mut::<jackdaw_scene_types::GltfSource>(model)
            .unwrap()
            .path = "models/barrel.glb".into();
        app.update();
        assert!(asked(&app, "models/barrel.glb"));

        app.world_mut()
            .spawn(jackdaw_animation_runtime::AnimationSet {
                sources: vec!["models/rig.glb".into()],
                ..default()
            });
        app.update();
        assert!(asked(&app, "models/rig.glb"));
    }

    #[test]
    fn another_project_asks_about_the_open_scenes_sources_again() {
        let mut app = library_app();
        app.world_mut().spawn(source("models/crate.glb"));
        app.update();
        assert!(asked(&app, "models/crate.glb"));

        for root in ["first", "second"] {
            app.world_mut()
                .insert_resource(crate::project::ProjectRoot {
                    root: std::path::PathBuf::from(root),
                    config: default(),
                });
            app.update();
            assert!(
                asked(&app, "models/crate.glb"),
                "the {root} project starts over and still asks"
            );
        }
    }
}
