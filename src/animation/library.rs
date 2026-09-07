//! Every animation clip the project's glTF files hold, indexed once per file.
//!
//! A clip used to arrive as a document entity spawned under the model that
//! carried it, which meant a scene saved a list of what its files happened to
//! contain and a game loading that scene read components it has no use for.
//! The same answer is editor state instead: the library is built by loading
//! each file once and asking it, and nothing about it reaches a document.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;

use bevy::asset::LoadState;
use bevy::gltf::Gltf;
use bevy::prelude::*;

/// Deepest directory tree the library walks, matching the asset listing the
/// remote serves.
const MAX_LIBRARY_DEPTH: usize = 12;

/// Frames a file may spend waiting for its clip assets before the library
/// indexes what it has and moves on, so one unreadable file cannot stall the
/// whole queue.
const CLIP_PATIENCE_FRAMES: u32 = 600;

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

/// Whether a path names a glTF file.
fn is_gltf(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".glb") || lower.ends_with(".gltf")
}

/// How far the library has got through the files it means to ask.
///
/// One file is asked at a time and its handle is parked here: dropping a
/// handle mid-load cancels it, and the restarted load republishes the asset,
/// which respawns every instance of the model in the open scene.
#[derive(Resource, Default)]
struct LibraryScan {
    /// Files found but not yet asked.
    queue: VecDeque<String>,
    /// Files already queued, so nothing is asked about twice.
    asked: HashSet<String>,
    /// The file being loaded, and how long it has been waited on.
    pending: Option<(String, Handle<Gltf>, u32)>,
    /// A handle for every file asked about, so a later frame never has to
    /// start the same load again.
    held: Vec<Handle<Gltf>>,
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
/// The walk over the assets directory loads every glTF it finds, so it runs
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
/// is of unknown size, and every answer costs a glTF load.
fn index_animation_library(
    project: Option<Res<crate::project::ProjectRoot>>,
    sources: Query<&jackdaw_scene_types::GltfSource>,
    sets: Query<&jackdaw_animation_runtime::AnimationSet>,
    asset_server: Res<AssetServer>,
    gltfs: Res<Assets<Gltf>>,
    clip_assets: Res<Assets<AnimationClip>>,
    demand: Res<LibraryDemand>,
    mut scan: ResMut<LibraryScan>,
    mut library: ResMut<AnimationLibrary>,
) {
    if let Some(project) = project.as_deref() {
        let assets_dir = project.assets_dir();
        if scan.walking.as_deref() != Some(assets_dir.as_path()) {
            scan.reseed(assets_dir);
            *library = AnimationLibrary::default();
        }
    }

    // What the open scene points at comes first: those files are the ones an
    // author is looking at, and they are loaded already.
    let wanted: Vec<String> = sources
        .iter()
        .map(|source| source.path.clone())
        .chain(sets.iter().flat_map(|set| set.sources.iter().cloned()))
        .collect();
    for path in wanted {
        scan.want(&path);
    }

    if demand.project_walk() {
        walk_one_directory(&mut scan);
    }
    ask_one_file(&mut scan, &asset_server, &gltfs, &clip_assets, &mut library);
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
        scan.want(&relative.to_string_lossy());
    }
}

/// Move the one file being asked about along, and start the next when it is
/// answered.
fn ask_one_file(
    scan: &mut LibraryScan,
    asset_server: &AssetServer,
    gltfs: &Assets<Gltf>,
    clip_assets: &Assets<AnimationClip>,
    library: &mut AnimationLibrary,
) {
    if scan.pending.is_none()
        && let Some(path) = scan.queue.pop_front()
    {
        let handle: Handle<Gltf> = asset_server.load(crate::entity_ops::to_asset_path(&path));
        scan.held.push(handle.clone());
        scan.pending = Some((path, handle, 0));
    }
    let Some((path, handle, waited)) = scan.pending.as_mut() else {
        return;
    };
    *waited += 1;
    let Some(gltf) = gltfs.get(&*handle) else {
        // A load the server has given up on is answered too, rather than
        // asking after a missing file forever.
        if matches!(
            asset_server.get_load_state(&*handle),
            Some(LoadState::Failed(_))
        ) {
            scan.pending = None;
        }
        return;
    };
    let patience_spent = *waited > CLIP_PATIENCE_FRAMES;
    let mut clips = Vec::with_capacity(gltf.named_animations.len());
    for (name, clip) in &gltf.named_animations {
        match clip_assets.get(clip) {
            Some(clip) => clips.push(LibraryClip {
                name: name.to_string(),
                duration_secs: clip.duration(),
                looped_hint: looped_hint(name),
            }),
            None if patience_spent => clips.push(LibraryClip {
                name: name.to_string(),
                duration_secs: 0.0,
                looped_hint: looped_hint(name),
            }),
            None => return,
        }
    }
    library.insert(path.clone(), clips);
    scan.pending = None;
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

    #[test]
    fn a_file_with_no_clips_is_not_listed() {
        let mut library = AnimationLibrary::default();
        library.insert("models/rock.glb".into(), Vec::new());
        assert!(library.is_empty());
    }
}
