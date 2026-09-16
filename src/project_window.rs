//! The Project window: the project's asset files in two columns, a folder tree
//! beside the selected folder's contents.
//!
//! A single click selects a file and puts its card in the inspector; a double
//! click opens it, which means a tab for a scene or a prefab and the kind's own
//! action for anything else. The kind filter narrows the tiles to one sort of
//! file, which is what the catalog listing used to be a window for.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, mpsc};

use bevy::ecs::spawn::Spawn;
use bevy::feathers::controls::FeathersDisclosureToggle;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use bevy::ui::Checked;
use bevy::ui_widgets::ValueChange;
use jackdaw_api::prelude::*;
use jackdaw_feathers::button::{ButtonProps, ButtonVariant, button};
use jackdaw_feathers::combobox::{
    ComboBoxChangeEvent, ComboBoxSelectedIndex, combobox_with_selected,
};
use jackdaw_feathers::split_panel::{panel, panel_group, panel_handle};
use jackdaw_feathers::text_edit::{
    TextEditCommitEvent, TextEditProps, TextEditValue, TextEditWrapper, set_text_input_value,
    text_edit,
};
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_feathers::{file_browser, icons, icons::IconFont, tokens};
use jackdaw_widgets::file_browser::{FileBrowserItem, FileItemDoubleClicked};
use jackdaw_widgets::tree_view::{
    TreeChildrenPopulated, TreeNodeExpandToggle, TreeNodeExpanded, TreeRowChildren, TreeRowContent,
    TreeRowLabel,
};
use path_slash::PathExt as _;

use crate::EditorEntity;
use crate::asset_drag::ActiveAssetDrag;
use crate::asset_files::AssetFileKind;
use crate::texture_files::{TextureInfo, is_image_file_path};

/// The window the project's files are shown in.
pub const PROJECT_WINDOW_ID: &str = "jackdaw.project";

/// The window ids the two panels this one replaced were saved under, the one
/// that sat in the bottom dock first: a migrated layout puts the two columns
/// where there is room for them.
pub const RETIRED_WINDOW_IDS: [&str; 2] = ["jackdaw.assets", "jackdaw.project_files"];

/// How many tiles one folder puts up at once. A folder holding more than this
/// is read and counted in full, but only this many are drawn, so opening a
/// folder of thousands of files costs a screenful rather than all of them.
const MAX_TILES: usize = 500;

/// How many entries a narrowed view reads before it stops looking deeper. A
/// project whose assets run to millions of files still answers a search.
const MAX_WALKED_ENTRIES: usize = 20_000;

/// The extensions the editor counts as sound.
const AUDIO_EXTENSIONS: [&str; 4] = ["ogg", "wav", "mp3", "flac"];

/// Which sort of file the tiles are narrowed to.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum KindFilter {
    #[default]
    All,
    Scenes,
    Prefabs,
    Materials,
    Definitions,
    Images,
    Audio,
}

impl KindFilter {
    pub const ALL: [KindFilter; 7] = [
        KindFilter::All,
        KindFilter::Scenes,
        KindFilter::Prefabs,
        KindFilter::Materials,
        KindFilter::Definitions,
        KindFilter::Images,
        KindFilter::Audio,
    ];

    /// The word a caller names the filter by.
    pub fn id(self) -> &'static str {
        match self {
            KindFilter::All => "all",
            KindFilter::Scenes => "scenes",
            KindFilter::Prefabs => "prefabs",
            KindFilter::Materials => "materials",
            KindFilter::Definitions => "definitions",
            KindFilter::Images => "images",
            KindFilter::Audio => "audio",
        }
    }

    /// What the filter reads as in the menu.
    pub fn label(self) -> &'static str {
        match self {
            KindFilter::All => "All",
            KindFilter::Scenes => "Scenes",
            KindFilter::Prefabs => "Prefabs",
            KindFilter::Materials => "Materials",
            KindFilter::Definitions => "Definitions",
            KindFilter::Images => "Images",
            KindFilter::Audio => "Audio",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|filter| filter.id().eq_ignore_ascii_case(id))
    }

    /// Whether an entry survives this filter. A folder always does, so the
    /// tree and the tiles agree on where the user is.
    fn keeps(self, entry: &DirEntry, kinds: &AssetKinds) -> bool {
        if entry.is_directory || self == KindFilter::All {
            return true;
        }
        match self {
            KindFilter::All => true,
            KindFilter::Scenes => {
                entry.kind == AssetFileKind::Scene && jackdaw_bsn::is_document_path(&entry.path)
            }
            KindFilter::Prefabs => entry.kind == AssetFileKind::Prefab,
            KindFilter::Materials => entry_kind_id(entry, kinds)
                .is_some_and(|kind| kind == crate::definition_assets::MATERIAL_KIND),
            KindFilter::Definitions => entry_kind_id(entry, kinds)
                .is_some_and(|kind| kind != crate::definition_assets::MATERIAL_KIND),
            KindFilter::Images => is_image_file_path(&entry.path),
            KindFilter::Audio => has_extension(&entry.path, &AUDIO_EXTENSIONS),
        }
    }
}

/// The kind id a file's type belongs to, for the files the catalog claims.
fn entry_kind_id<'a>(entry: &DirEntry, kinds: &'a AssetKinds) -> Option<&'a str> {
    let type_path = entry.kind.type_path()?;
    kinds.by_type_path(type_path).map(|kind| kind.kind.as_str())
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|wanted| extension.eq_ignore_ascii_case(wanted))
        })
}

/// One row of the tile grid.
#[derive(Clone, Debug)]
pub struct DirEntry {
    pub path: PathBuf,
    pub file_name: String,
    pub is_directory: bool,
    pub texture_info: Option<TextureInfo>,
    /// What the file says it holds; `Scene` for everything that says nothing.
    pub kind: AssetFileKind,
    /// The folder the file sits in, relative to the one the tiles are
    /// showing, for a result the search or the filter found further down.
    pub folder: Option<String>,
}

impl DirEntry {
    pub fn is_prefab(&self) -> bool {
        self.kind == AssetFileKind::Prefab
    }
}

/// What the Project window is showing.
#[derive(Resource)]
pub struct ProjectWindowState {
    /// The folder the tree is rooted at: the project's assets.
    pub root_directory: PathBuf,
    /// The folder whose files fill the tiles.
    pub current_directory: PathBuf,
    /// The file the tiles have selected, as an absolute path.
    pub selected_file: Option<String>,
    /// What the search box holds.
    pub search: String,
    pub kind_filter: KindFilter,
    pub entries: Vec<DirEntry>,
    pub needs_refresh: bool,
    pub needs_tree_refresh: bool,
    /// Whether the tiles are behind the listing.
    needs_tiles: bool,
    /// The file whose tile is showing a rename field.
    pub renaming: Option<PathBuf>,
    /// Whether the last narrowed listing gave up before it reached the bottom
    /// of the tree.
    stopped_looking: bool,
    /// The folders whose rows are open, so a rebuild puts the tree back the
    /// way the user left it.
    expanded: HashSet<PathBuf>,
    last_click_time: f64,
    kind_cache: crate::asset_files::AssetKindCache,
}

impl Default for ProjectWindowState {
    fn default() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            root_directory: cwd.clone(),
            current_directory: cwd,
            selected_file: None,
            search: String::new(),
            kind_filter: KindFilter::default(),
            entries: Vec::new(),
            needs_refresh: true,
            needs_tree_refresh: true,
            needs_tiles: true,
            renaming: None,
            stopped_looking: false,
            expanded: HashSet::new(),
            last_click_time: 0.0,
            kind_cache: crate::asset_files::AssetKindCache::default(),
        }
    }
}

impl ProjectWindowState {
    /// A window rooted at `directory` and showing it.
    pub fn at(directory: impl Into<PathBuf>) -> Self {
        let directory = directory.into();
        Self {
            current_directory: directory.clone(),
            root_directory: directory,
            ..Self::default()
        }
    }

    /// Show `directory`, dropping a selection that is no longer on screen.
    pub fn show_folder(&mut self, directory: PathBuf) {
        self.expand_to(&directory);
        self.current_directory = directory;
        self.selected_file = None;
        self.renaming = None;
        self.needs_refresh = true;
    }

    /// Open every folder row on the way down to `directory`, so what the
    /// tiles are showing is reachable in the tree rather than hidden under a
    /// closed ancestor.
    pub fn expand_to(&mut self, directory: &Path) {
        let root = self.root_directory.clone();
        let folders: Vec<PathBuf> = directory
            .ancestors()
            .take_while(|path| path.starts_with(&root))
            .map(Path::to_path_buf)
            .collect();
        let opened = folders
            .into_iter()
            .filter(|folder| self.expanded.insert(folder.clone()))
            .count();
        if opened > 0 {
            self.needs_tree_refresh = true;
        }
    }

    /// Whether the tiles are narrowed to a search or to one kind of file, in
    /// which case they answer for the whole tree below the folder shown.
    pub fn is_narrowed(&self) -> bool {
        !self.search.is_empty() || self.kind_filter != KindFilter::All
    }

    /// Whether the tree row for `directory` is open.
    pub fn is_expanded(&self, directory: &Path) -> bool {
        self.expanded.contains(directory)
    }

    fn rebuild(&mut self) {
        self.needs_refresh = true;
        self.needs_tree_refresh = true;
    }
}

/// Marker on the Project window's root node.
#[derive(Component)]
pub struct ProjectWindowPanel;

/// Marker on the container the folder tree is built under.
#[derive(Component)]
pub struct ProjectFolderTree;

/// Marker on the container the file tiles are built under.
#[derive(Component)]
pub struct ProjectFileGrid;

/// Marker on the bar naming the folder the tiles are showing.
#[derive(Component)]
pub struct ProjectPathBar;

/// Marker on the search field.
#[derive(Component)]
pub struct ProjectSearchInput;

/// Marker on the kind filter menu.
#[derive(Component)]
struct ProjectKindFilterMenu;

/// A folder row in the tree, and the folder it stands for.
#[derive(Component)]
pub struct ProjectFolderNode(pub PathBuf);

/// Links a folder row's disclosure control to the row it opens.
#[derive(Component)]
struct FolderDisclosure(Entity);

/// Marks the row of the folder the tiles are showing.
#[derive(Component)]
pub struct ShownFolder;

/// Marks the field a rename is typed into.
#[derive(Component)]
struct RenameField(PathBuf);

/// Open or close one folder row. Both the row click and the disclosure control
/// raise this, so the two agree on what a toggle does.
#[derive(EntityEvent)]
struct ToggleFolder {
    entity: Entity,
}

/// Watches the project's assets for files appearing, going and being renamed.
#[derive(Resource)]
struct ProjectWatcher {
    _watcher: notify::RecommendedWatcher,
    receiver: Mutex<mpsc::Receiver<()>>,
}

#[derive(Resource)]
struct ProjectFolderTask(Task<Option<rfd::FileHandle>>);

/// Where the context menu was opened, so the action observer can resolve the
/// click against the row rather than the selection.
#[derive(Resource, Default)]
struct MenuTarget {
    path: PathBuf,
}

pub struct ProjectWindowPlugin;

impl Plugin for ProjectWindowPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ProjectWindowState>()
            .init_resource::<MenuTarget>()
            .add_systems(OnEnter(crate::AppState::Editor), setup_project_window)
            .add_systems(
                Update,
                (
                    check_project_watcher,
                    poll_project_folder_pick,
                    refresh_folder_tree,
                    mark_the_shown_folder.after(refresh_folder_tree),
                    read_the_folder,
                    rebuild_tiles,
                    read_search_field,
                    show_the_filter_and_the_search.after(read_search_field),
                )
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_observer(on_folder_toggled)
            .add_observer(on_folder_disclosure_change)
            .add_observer(on_file_double_click)
            .add_observer(on_kind_filter_change)
            .add_observer(on_rename_commit)
            .add_observer(on_context_action);
    }
}

// -- Setup and watching -----------------------------------------------------

fn setup_project_window(
    mut state: ResMut<ProjectWindowState>,
    mut commands: Commands,
    project_root: Option<Res<crate::project::ProjectRoot>>,
) {
    let root = match project_root {
        Some(project) => project.assets_dir(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    state.root_directory = root.clone();
    state.current_directory = root.clone();
    state.selected_file = None;
    state.expanded.clear();
    state.expanded.insert(root.clone());
    state.rebuild();
    watch_directory(&root, &mut commands);
}

fn watch_directory(root: &Path, commands: &mut Commands) {
    let (tx, rx) = mpsc::channel();
    let watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        if let Ok(event) = res {
            use notify::EventKind;
            if matches!(
                event.kind,
                EventKind::Create(_)
                    | EventKind::Remove(_)
                    | EventKind::Modify(notify::event::ModifyKind::Name(_))
            ) {
                let _ = tx.send(());
            }
        }
    });
    match watcher {
        Ok(mut watcher) => {
            use notify::Watcher;
            if watcher
                .watch(root, notify::RecursiveMode::Recursive)
                .is_ok()
            {
                commands.insert_resource(ProjectWatcher {
                    _watcher: watcher,
                    receiver: Mutex::new(rx),
                });
            } else {
                warn!(
                    "the project's files at {} cannot be watched",
                    root.display()
                );
            }
        }
        Err(err) => warn!("the project's files cannot be watched: {err}"),
    }
}

fn check_project_watcher(
    watcher: Option<Res<ProjectWatcher>>,
    mut state: ResMut<ProjectWindowState>,
    mut materials: ResMut<crate::material_browser::MaterialBrowserState>,
) {
    let Some(watcher) = watcher else { return };
    let Ok(rx) = watcher.receiver.lock() else {
        return;
    };
    let mut changed = false;
    while rx.try_recv().is_ok() {
        changed = true;
    }
    if changed {
        state.rebuild();
        materials.needs_rescan = true;
    }
}

fn read_search_field(
    mut state: ResMut<ProjectWindowState>,
    fields: Query<&TextEditValue, (With<ProjectSearchInput>, Changed<TextEditValue>)>,
) {
    for field in fields {
        if state.search != field.0 {
            state.search = field.0.clone();
            state.needs_refresh = true;
        }
    }
}

/// Put the kind filter and the search text on the controls, so a window an
/// operator drove reads the same as one a click drove.
fn show_the_filter_and_the_search(
    state: Res<ProjectWindowState>,
    mut menus: Query<&mut ComboBoxSelectedIndex, With<ProjectKindFilterMenu>>,
    fields: Query<(&TextEditValue, &Children), With<ProjectSearchInput>>,
    wrappers: Query<&TextEditWrapper>,
    mut editables: Query<&mut bevy::text::EditableText>,
) {
    if !state.is_changed() {
        return;
    }
    let index = KindFilter::ALL
        .iter()
        .position(|filter| *filter == state.kind_filter)
        .unwrap_or(0);
    for mut selected in &mut menus {
        if selected.0 != index {
            selected.0 = index;
        }
    }
    for (value, children) in &fields {
        if value.0 == state.search {
            continue;
        }
        for child in children.iter() {
            let Ok(wrapper) = wrappers.get(child) else {
                continue;
            };
            let Ok(mut editable) = editables.get_mut(wrapper.0) else {
                continue;
            };
            set_text_input_value(&mut editable, state.search.clone());
            break;
        }
    }
}

// -- The folder tree --------------------------------------------------------

/// The folders directly under `dir`, sorted by name.
fn scan_folders(dir: &Path) -> Vec<PathBuf> {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut folders: Vec<PathBuf> = read_dir
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if !path.is_dir() {
                return None;
            }
            let name = path.file_name()?.to_string_lossy().to_string();
            (!name.starts_with('.') && name != "target").then_some(path)
        })
        .collect();
    folders.sort_by_key(|path| {
        path.file_name()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .into_string()
            .unwrap_or_default()
    });
    folders
}

/// Rebuild the tree from the root whenever the filesystem moved under it.
fn refresh_folder_tree(
    mut state: ResMut<ProjectWindowState>,
    trees: Query<(Entity, Option<&Children>), With<ProjectFolderTree>>,
    mut commands: Commands,
) {
    if !state.needs_tree_refresh {
        return;
    }
    let Ok((tree, existing)) = trees.single() else {
        return;
    };
    state.needs_tree_refresh = false;

    if let Some(children) = existing {
        for child in children.iter() {
            commands.entity(child).despawn();
        }
    }
    if !state.root_directory.is_dir() {
        return;
    }
    spawn_folder_row(
        &mut commands,
        tree,
        &state.root_directory,
        true,
        &state.expanded,
    );
}

/// Spawn one folder row. `is_root` names the row after the project's assets
/// rather than after the folder on disk, which may be called anything.
fn spawn_folder_row(
    commands: &mut Commands,
    parent: Entity,
    path: &Path,
    is_root: bool,
    expanded: &HashSet<PathBuf>,
) {
    let is_open = expanded.contains(path);
    let label = if is_root {
        "Assets".to_string()
    } else {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    };

    let row = commands
        .spawn((
            ProjectFolderNode(path.to_path_buf()),
            TreeNodeExpanded(is_open),
            TreeChildrenPopulated(is_open),
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                ..default()
            },
            ChildOf(parent),
        ))
        .id();

    let content = commands
        .spawn((
            TreeRowContent,
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(tokens::SPACING_XS)),
                column_gap: Val::Px(tokens::SPACING_SM),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                width: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(Color::NONE),
            ChildOf(row),
        ))
        .id();

    commands.entity(content).observe(highlight_on_hover);
    commands.entity(content).observe(unhighlight_on_out);

    let disclosure = commands
        .spawn_scene(bsn! { @FeathersDisclosureToggle })
        .insert((
            TreeNodeExpandToggle,
            FolderDisclosure(row),
            ChildOf(content),
        ))
        .id();
    if is_open {
        commands.entity(disclosure).insert(Checked);
    }

    commands.spawn((
        TreeRowLabel,
        Text::new(label),
        TextFont {
            font_size: tokens::TEXT_SIZE,
            ..default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(content),
    ));

    let children = commands
        .spawn((
            TreeRowChildren,
            Node {
                flex_direction: FlexDirection::Column,
                padding: UiRect::left(Val::Px(16.0)),
                margin: UiRect::left(Val::Px(tokens::SPACING_SM)),
                border: UiRect::left(Val::Px(1.0)),
                width: Val::Percent(100.0),
                display: if is_open {
                    Display::Flex
                } else {
                    Display::None
                },
                ..default()
            },
            BorderColor::all(tokens::CONNECTION_LINE),
            ChildOf(row),
        ))
        .id();
    if is_open {
        for nested in scan_folders(path) {
            spawn_folder_row(commands, children, &nested, false, expanded);
        }
    }

    let folder = path.to_path_buf();
    commands.entity(content).observe(
        move |click: On<Pointer<Click>>,
              mut commands: Commands,
              windows: Query<&Window>,
              mut menu: ResMut<jackdaw_widgets::context_menu::ContextMenuState>| {
            if click.event().button == PointerButton::Secondary {
                open_context_menu(&mut commands, &windows, &mut menu, &folder, true);
                return;
            }
            let path = folder.clone();
            commands.queue(move |world: &mut World| {
                select_path(world, &path);
            });
            commands.trigger(ToggleFolder { entity: row });
        },
    );
}

fn on_folder_disclosure_change(
    change: On<ValueChange<bool>>,
    disclosures: Query<&FolderDisclosure>,
    mut commands: Commands,
) {
    let Ok(row) = disclosures.get(change.source) else {
        return;
    };
    commands.trigger(ToggleFolder { entity: row.0 });
}

/// Flip the row's expanded flag, show or hide its children, populate them the
/// first time it opens, and point the disclosure the way the row now stands.
fn on_folder_toggled(
    event: On<ToggleFolder>,
    mut state: ResMut<ProjectWindowState>,
    mut rows: Query<(
        &mut TreeNodeExpanded,
        &mut TreeChildrenPopulated,
        &ProjectFolderNode,
    )>,
    children_query: Query<&Children>,
    containers: Query<(), With<TreeRowChildren>>,
    contents: Query<(), With<TreeRowContent>>,
    toggles: Query<(), With<TreeNodeExpandToggle>>,
    mut nodes: Query<&mut Node>,
    mut commands: Commands,
) {
    let row = event.entity;
    let Ok((mut expanded, mut populated, folder)) = rows.get_mut(row) else {
        return;
    };
    expanded.0 = !expanded.0;
    let is_expanded = expanded.0;
    let folder = folder.0.clone();
    if is_expanded {
        state.expanded.insert(folder.clone());
    } else {
        state.expanded.remove(&folder);
    }

    let Ok(children) = children_query.get(row) else {
        return;
    };
    for child in children.iter() {
        if containers.contains(child) {
            if let Ok(mut node) = nodes.get_mut(child) {
                node.display = if is_expanded {
                    Display::Flex
                } else {
                    Display::None
                };
            }
            if is_expanded && !populated.0 {
                populated.0 = true;
                for nested in scan_folders(&folder) {
                    spawn_folder_row(&mut commands, child, &nested, false, &state.expanded);
                }
            }
        }
        if !contents.contains(child) {
            continue;
        }
        let Ok(content_children) = children_query.get(child) else {
            continue;
        };
        for toggle in content_children.iter() {
            if toggles.contains(toggle) {
                jackdaw_feathers::utils::set_marker_if_alive::<Checked>(
                    &mut commands,
                    toggle,
                    is_expanded,
                );
            }
        }
    }
}

// -- The tile grid ----------------------------------------------------------

/// One listing row for `path`, or `None` for a file the tiles never show.
///
/// `current` is the folder the tiles are showing, which a result found
/// further down is captioned with its distance from.
fn dir_entry(path: PathBuf, current: &Path, selected: Option<&str>) -> Option<DirEntry> {
    let file_name = path.file_name()?.to_string_lossy().into_owned();
    if file_name.starts_with('.') {
        return None;
    }
    let is_selected = selected == Some(path.to_string_lossy().as_ref());
    if !is_selected && jackdaw_bsn::is_binary_path(&path) && path.with_extension("bsn").is_file() {
        return None;
    }
    let folder = path
        .parent()
        .filter(|parent| *parent != current)
        .and_then(|parent| parent.strip_prefix(current).ok())
        .map(|relative| relative.to_slash_lossy().into_owned());
    // `DirEntry::file_type` reports the link itself, which takes a
    // symlinked directory for a file. Ask the path, which follows it.
    let is_directory = path.is_dir();
    Some(DirEntry {
        path,
        file_name,
        is_directory,
        texture_info: None,
        kind: AssetFileKind::Scene,
        folder,
    })
}

/// Every file under `root` other than its own direct children, which the
/// listing already holds, and whether `limit` stopped the walk short of the
/// bottom of the tree.
fn walk_below(root: &Path, limit: usize) -> (Vec<PathBuf>, bool) {
    let mut found = Vec::new();
    let mut pending: VecDeque<PathBuf> = scan_folders(root).into_iter().collect();
    let mut read = 0usize;
    while let Some(folder) = pending.pop_front() {
        let Ok(read_dir) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in read_dir.flatten() {
            read += 1;
            if read > limit {
                return (found, true);
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "target" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                pending.push_back(path);
            } else {
                found.push(path);
            }
        }
    }
    (found, false)
}

/// Read the folder the window is showing, applying the search and the kind
/// filter, and memoing what each document holds by its modification time.
///
/// A search or a kind filter looks through the whole tree below the folder,
/// not only the folder itself, so a name the user half-remembers is found
/// from the root; each result carries the folder it was found in.
///
/// A document held in both forms is one file to the user, so only the text one
/// is listed, and the selected file is listed whatever the search and the
/// filter say, so narrowing the view never takes the inspector's subject off
/// the screen.
fn scan_current_directory(state: &mut ProjectWindowState, kinds: &AssetKinds) -> Vec<DirEntry> {
    let current = state.current_directory.clone();
    let selected = state.selected_file.clone();
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(&current) {
        Ok(read_dir) => read_dir.flatten().map(|entry| entry.path()).collect(),
        Err(_) => Vec::new(),
    };
    state.stopped_looking = false;
    if state.is_narrowed() {
        let (deeper, stopped) = walk_below(&current, MAX_WALKED_ENTRIES);
        paths.extend(deeper);
        state.stopped_looking = stopped;
    }

    let mut entries: Vec<DirEntry> = paths
        .into_iter()
        .filter_map(|path| dir_entry(path, &current, selected.as_deref()))
        .collect();

    for entry in entries.iter_mut() {
        let reads_a_type =
            entry
                .path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("jsn")
                        || jackdaw_bsn::is_document_extension(extension)
                });
        if !entry.is_directory && reads_a_type {
            entry.kind = state.kind_cache.check(&entry.path, kinds);
        }
    }

    let filter = state.kind_filter;
    let search = state.search.to_lowercase();
    entries.retain(|entry| {
        if selected.as_deref() == Some(entry.path.to_string_lossy().as_ref()) {
            return true;
        }
        let matches_search = search.is_empty() || entry.file_name.to_lowercase().contains(&search);
        matches_search && filter.keeps(entry, kinds)
    });
    entries.sort_by(|a, b| {
        b.is_directory
            .cmp(&a.is_directory)
            .then_with(|| a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()))
    });
    entries
}

/// Read the folder the window is showing into the listing. The listing is
/// state, not a view, so it keeps up whether or not the window is open.
fn read_the_folder(mut state: ResMut<ProjectWindowState>, kinds: Res<AssetKinds>) {
    if !state.needs_refresh {
        return;
    }
    state.needs_refresh = false;

    // A folder the user was in can go while they are in it; fall back to the
    // root rather than showing an empty grid with a path that no longer is.
    if !state.current_directory.is_dir() {
        let root = state.root_directory.clone();
        state.show_folder(root);
        state.needs_refresh = false;
    }
    if let Some(selected) = state.selected_file.clone()
        && !Path::new(&selected).exists()
    {
        state.selected_file = None;
        state.renaming = None;
    }

    let scanned = scan_current_directory(&mut state, &kinds);
    state.entries = scanned;
    state.needs_tiles = true;
}

/// Put the listing on screen as tiles, once there is a window to put it in.
fn rebuild_tiles(
    mut state: ResMut<ProjectWindowState>,
    mut commands: Commands,
    kinds: Res<AssetKinds>,
    icon_font: Res<IconFont>,
    asset_server: Res<AssetServer>,
    grids: Query<(Entity, Option<&Children>), With<ProjectFileGrid>>,
    path_bars: Query<(Entity, Option<&Children>), With<ProjectPathBar>>,
) {
    if !state.needs_tiles {
        return;
    }
    let Ok((grid, grid_children)) = grids.single() else {
        return;
    };
    state.needs_tiles = false;

    if let Some(children) = grid_children {
        for child in children.iter() {
            commands.entity(child).despawn();
        }
    }

    let mut entries = state.entries.clone();
    let selected = state.selected_file.clone();
    let renaming = state.renaming.clone();
    let drawn = entries.len().min(MAX_TILES);
    for entry in entries[..drawn].iter_mut() {
        if !entry.is_directory && is_image_file_path(&entry.path) {
            entry.texture_info = Some(TextureInfo::read(&entry.path, &asset_server));
        }
    }
    for entry in &entries[..drawn] {
        let tile = spawn_tile(
            &mut commands,
            grid,
            entry,
            &kinds,
            &icon_font,
            selected.as_deref() == Some(entry.path.to_string_lossy().as_ref()),
            renaming.as_deref() == Some(entry.path.as_path()),
        );
        attach_tile_behaviour(&mut commands, tile, entry);
    }
    if entries.len() > drawn {
        let rest = entries.len() - drawn;
        let note = commands
            .spawn((
                Text::new(format!(
                    "{rest} more files here; search or filter to reach them"
                )),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                Node {
                    width: percent(100),
                    margin: UiRect::all(px(tokens::SPACING_SM)),
                    ..default()
                },
            ))
            .id();
        jackdaw_feathers::utils::attach_or_despawn(&mut commands, grid, note);
    }
    if state.stopped_looking {
        let note = commands
            .spawn((
                Text::new(format!(
                    "stopped looking after {MAX_WALKED_ENTRIES} files; open a folder to look inside it"
                )),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                Node {
                    width: percent(100),
                    margin: UiRect::all(px(tokens::SPACING_SM)),
                    ..default()
                },
            ))
            .id();
        jackdaw_feathers::utils::attach_or_despawn(&mut commands, grid, note);
    }

    rebuild_path_bar(&mut commands, &path_bars, &state);
}

/// The tile one entry gets: a thumbnail for an image, a rendered model for a
/// `.glb`, and the file browser's own row for everything else.
fn spawn_tile(
    commands: &mut Commands,
    grid: Entity,
    entry: &DirEntry,
    kinds: &AssetKinds,
    icon_font: &IconFont,
    is_selected: bool,
    is_renaming: bool,
) -> Entity {
    let tile = match &entry.texture_info {
        Some(info) => spawn_image_tile(commands, entry, info, is_renaming),
        None => {
            let item = FileBrowserItem {
                path: entry.path.to_string_lossy().to_string(),
                is_directory: entry.is_directory,
                file_name: entry.file_name.clone(),
            };
            let definition = entry
                .kind
                .type_path()
                .and_then(|type_path| kinds.by_type_path(type_path));
            let icon = definition
                .map(|kind| kind.icon)
                .or_else(|| entry.is_prefab().then_some(icons::Icon::Package));
            let tile = match tile_subject(entry, kinds) {
                Some(subject) => commands
                    .spawn(thumbnail_tile(
                        &item,
                        icon_font,
                        entry.path.clone(),
                        subject,
                        icon,
                    ))
                    .id(),
                None => commands
                    .spawn(file_browser::file_browser_item_with_icon(
                        &item, icon_font, icon,
                    ))
                    .id(),
            };
            if let Some(definition) = definition {
                commands
                    .entity(tile)
                    .insert((Hovered::default(), Tooltip::title(definition.label.clone())));
            }
            if is_renaming {
                commands.spawn((rename_field(&entry.path), ChildOf(tile)));
            }
            tile
        }
    };

    if let Some(folder) = &entry.folder {
        attach_folder_caption(commands, tile, &entry.path, folder);
    }
    if is_selected {
        commands
            .entity(tile)
            .insert(BackgroundColor(tokens::ELEVATED_BG));
    }
    // The rebuild queues one spawn per entry against the grid it saw this
    // frame; a panel rebuild can despawn that grid before these flush, which
    // would orphan every tile under a dead parent.
    jackdaw_feathers::utils::attach_or_despawn(commands, grid, tile);
    tile
}

fn spawn_image_tile(
    commands: &mut Commands,
    entry: &DirEntry,
    info: &TextureInfo,
    is_renaming: bool,
) -> Entity {
    let tile = commands
        .spawn((
            Node {
                width: Val::Px(tokens::THUMB_CELL_WIDTH),
                height: Val::Px(tokens::THUMB_CELL_HEIGHT),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                padding: UiRect::all(Val::Px(2.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(4.0)),
                ..default()
            },
            BorderColor::all(Color::NONE),
            BackgroundColor(Color::NONE),
        ))
        .id();

    match &info.image_handle {
        Some(image) => {
            commands.spawn((
                ImageNode::new(image.clone()),
                Node {
                    width: Val::Px(tokens::THUMB_IMAGE_SIZE),
                    height: Val::Px(tokens::THUMB_IMAGE_SIZE),
                    ..default()
                },
                ChildOf(tile),
            ));
        }
        None => {
            let placeholder = commands
                .spawn((
                    Node {
                        width: Val::Px(tokens::THUMB_IMAGE_SIZE),
                        height: Val::Px(tokens::THUMB_IMAGE_SIZE),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.25, 0.25, 0.25)),
                    ChildOf(tile),
                ))
                .id();
            commands.spawn((
                Text::new(info.description()),
                TextFont {
                    font_size: tokens::TEXT_SIZE_XS,
                    ..default()
                },
                TextColor(Color::srgb(0.8, 0.8, 0.8)),
                ChildOf(placeholder),
            ));
        }
    }

    if is_renaming {
        commands.spawn((rename_field(&entry.path), ChildOf(tile)));
        return tile;
    }

    let is_truncated = entry.file_name.chars().count() > 10;
    let display_name = if is_truncated {
        let head: String = entry.file_name.chars().take(8).collect();
        format!("{head}...")
    } else {
        entry.file_name.clone()
    };
    let mut label = commands.spawn((
        Text::new(display_name),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            max_width: px(tokens::THUMB_NAME_MAX_WIDTH),
            overflow: Overflow::clip(),
            ..default()
        },
        ChildOf(tile),
    ));
    if is_truncated {
        label.insert((Hovered::default(), Tooltip::title(entry.file_name.clone())));
    }
    tile
}

/// What a file's tile is pictured with, or `None` for one that keeps its
/// icon.
fn tile_subject(entry: &DirEntry, kinds: &AssetKinds) -> Option<crate::thumbnail::Subject> {
    use crate::thumbnail::Subject;
    if entry.is_directory {
        return None;
    }
    if crate::thumbnail::is_model_path(&entry.path) {
        return Some(Subject::Model);
    }
    if entry_kind_id(entry, kinds)
        .is_some_and(|kind| kind == crate::definition_assets::MATERIAL_KIND)
    {
        return Some(Subject::Material);
    }
    if entry.is_prefab() {
        return Some(Subject::Prefab);
    }
    (entry.kind == AssetFileKind::Scene && jackdaw_bsn::is_document_path(&entry.path))
        .then_some(Subject::Scene)
}

/// The tile a pictured file gets: the same shape as the file browser's own
/// row, with the icon inside a square [`crate::thumbnail::ThumbnailSlot`]
/// that the rendered picture replaces once there is one. The glyph is the
/// fallback, so a pending or failed thumbnail looks like any other file.
fn thumbnail_tile(
    item: &FileBrowserItem,
    icon_font: &IconFont,
    path: PathBuf,
    subject: crate::thumbnail::Subject,
    icon: Option<icons::Icon>,
) -> impl Bundle {
    let slot_size = crate::thumbnail::THUMBNAIL_DISPLAY_SIZE;
    let glyph = icon.unwrap_or_else(|| file_browser::file_icon(&item.file_name));
    (
        item.clone(),
        Node {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            padding: UiRect::all(Val::Px(6.0)),
            width: Val::Px(80.0),
            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
            ..default()
        },
        BackgroundColor(Color::NONE),
        children![
            (
                crate::thumbnail::ThumbnailSlot::new(path, subject),
                Node {
                    width: Val::Px(slot_size),
                    height: Val::Px(slot_size),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                children![(
                    Text::new(String::from(glyph.unicode())),
                    TextFont {
                        font: icon_font.0.clone().into(),
                        font_size: tokens::ICON_LG,
                        ..default()
                    },
                    TextColor(tokens::FILE_ICON_COLOR),
                )],
            ),
            (
                Text::new(truncate_tile_name(&item.file_name, 12)),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                bevy::feathers::theme::ThemedText,
            ),
        ],
    )
}

/// Put the folder a result was found in under its tile, as a control that
/// takes the window there.
fn attach_folder_caption(commands: &mut Commands, tile: Entity, path: &Path, folder: &str) {
    let caption = commands
        .spawn((
            Text::new(truncate_tile_name(folder, 14)),
            TextFont {
                font_size: tokens::TEXT_SIZE_XS,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            Hovered::default(),
            Tooltip::title(folder.to_string()),
            ChildOf(tile),
        ))
        .id();
    let folder = path.parent().map(Path::to_path_buf);
    commands.entity(caption).observe(
        move |mut click: On<Pointer<Click>>, mut commands: Commands| {
            if click.event().button != PointerButton::Primary {
                return;
            }
            click.propagate(false);
            let Some(folder) = folder.clone() else {
                return;
            };
            commands.queue(move |world: &mut World| {
                select_path(world, &folder);
            });
        },
    );
}

/// Shorten a name to fit a tile, cutting on a character boundary so a
/// non-ASCII name cannot panic the slice.
fn truncate_tile_name(name: &str, max_len: usize) -> String {
    if name.chars().count() <= max_len {
        return name.to_string();
    }
    let head: String = name.chars().take(max_len.saturating_sub(3)).collect();
    format!("{head}...")
}

/// The field a rename is typed into, over the tile's own label.
fn rename_field(path: &Path) -> impl Bundle {
    let current = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    (
        RenameField(path.to_path_buf()),
        Node {
            width: Val::Px(tokens::THUMB_NAME_MAX_WIDTH),
            ..default()
        },
        children![text_edit(
            TextEditProps::default()
                .with_default_value(current)
                .auto_focus()
                .select_all_on_open()
        )],
    )
}

/// Click, double click, right click and drag, for one tile.
fn attach_tile_behaviour(commands: &mut Commands, tile: Entity, entry: &DirEntry) {
    commands
        .entity(tile)
        .observe(highlight_on_hover)
        .observe(unhighlight_on_out);

    let path = entry.path.clone();
    let is_directory = entry.is_directory;
    commands.entity(tile).observe(
        move |click: On<Pointer<Click>>,
              mut state: ResMut<ProjectWindowState>,
              mut commands: Commands,
              time: Res<Time>| {
            if click.event().button != PointerButton::Primary {
                return;
            }
            let now = time.elapsed_secs_f64();
            let as_string = path.to_string_lossy().into_owned();
            let is_double = state.selected_file.as_deref() == Some(as_string.as_str())
                && (now - state.last_click_time) < 0.4;
            state.last_click_time = now;
            if is_double {
                commands.trigger(FileItemDoubleClicked {
                    path: as_string,
                    is_directory,
                });
                return;
            }
            let path = path.clone();
            commands.queue(move |world: &mut World| {
                select_path(world, &path);
            });
        },
    );

    let path = entry.path.clone();
    let is_directory = entry.is_directory;
    commands.entity(tile).observe(
        move |click: On<Pointer<Click>>,
              mut commands: Commands,
              windows: Query<&Window>,
              mut menu: ResMut<jackdaw_widgets::context_menu::ContextMenuState>| {
            if click.event().button != PointerButton::Secondary {
                return;
            }
            open_context_menu(&mut commands, &windows, &mut menu, &path, is_directory);
            let path = path.clone();
            commands.queue(move |world: &mut World| {
                select_path(world, &path);
            });
        },
    );

    if entry.is_directory {
        return;
    }
    if entry
        .texture_info
        .as_ref()
        .is_some_and(TextureInfo::is_plain_2d)
    {
        let path = entry.path.clone();
        commands.entity(tile).observe(
            move |_: On<Pointer<DragStart>>, mut drag: ResMut<ActiveAssetDrag>| {
                drag.image = Some(path.clone());
            },
        );
        commands.entity(tile).observe(
            |_: On<Pointer<DragEnd>>, mut drag: ResMut<ActiveAssetDrag>| {
                drag.image = None;
            },
        );
        return;
    }
    if entry.is_prefab() || jackdaw_bsn::is_document_path(&entry.path) {
        let path = entry.path.clone();
        commands.entity(tile).observe(
            move |_: On<Pointer<DragStart>>, mut drag: ResMut<ActiveAssetDrag>| {
                drag.path = Some(path.clone());
            },
        );
        commands.entity(tile).observe(
            |_: On<Pointer<DragEnd>>, mut drag: ResMut<ActiveAssetDrag>| {
                drag.path = None;
            },
        );
    }
}

fn highlight_on_hover(hover: On<Pointer<Over>>, mut backgrounds: Query<&mut BackgroundColor>) {
    if let Ok(mut background) = backgrounds.get_mut(hover.event_target()) {
        background.0 = tokens::HOVER_BG;
    }
}

fn unhighlight_on_out(
    out: On<Pointer<Out>>,
    shown: Query<(), With<ShownFolder>>,
    mut backgrounds: Query<&mut BackgroundColor>,
) {
    let target = out.event_target();
    if let Ok(mut background) = backgrounds.get_mut(target) {
        background.0 = if shown.contains(target) {
            tokens::ELEVATED_BG
        } else {
            Color::NONE
        };
    }
}

/// Mark the tree row whose folder the tiles are showing, so the two columns
/// say the same thing about where the user is.
fn mark_the_shown_folder(
    state: Res<ProjectWindowState>,
    rows: Query<(&ProjectFolderNode, &Children)>,
    row_contents: Query<(), With<TreeRowContent>>,
    shown: Query<(), With<ShownFolder>>,
    mut backgrounds: Query<&mut BackgroundColor>,
    mut commands: Commands,
) {
    for (folder, children) in &rows {
        let Some(content) = children.iter().find(|child| row_contents.contains(*child)) else {
            continue;
        };
        let is_shown = folder.0 == state.current_directory;
        if is_shown == shown.contains(content) {
            continue;
        }
        if is_shown {
            commands.entity(content).insert(ShownFolder);
        } else {
            commands.entity(content).remove::<ShownFolder>();
        }
        if let Ok(mut background) = backgrounds.get_mut(content) {
            background.0 = if is_shown {
                tokens::ELEVATED_BG
            } else {
                Color::NONE
            };
        }
    }
}

/// Rebuild the bar naming where the tiles are: one button per folder on the
/// way down from the root, then the selected file.
fn rebuild_path_bar(
    commands: &mut Commands,
    path_bars: &Query<(Entity, Option<&Children>), With<ProjectPathBar>>,
    state: &ProjectWindowState,
) {
    let Ok((bar, existing)) = path_bars.single() else {
        return;
    };
    if let Some(children) = existing {
        for child in children.iter() {
            commands.entity(child).despawn();
        }
    }

    let mut folders: Vec<PathBuf> = state
        .current_directory
        .ancestors()
        .take_while(|path| path.starts_with(&state.root_directory))
        .map(Path::to_path_buf)
        .collect();
    folders.reverse();
    let selected_name = state.selected_file.as_ref().and_then(|selected| {
        Path::new(selected)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    });
    let root = state.root_directory.clone();

    let row = commands
        .spawn(Node {
            width: percent(100),
            align_items: AlignItems::Center,
            column_gap: Val::Px(2.0),
            ..default()
        })
        .with_children(|parent| {
            for (index, folder) in folders.iter().enumerate() {
                if index > 0 {
                    parent.spawn(separator_text());
                }
                let name = if *folder == root {
                    "Assets".to_string()
                } else {
                    folder
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                };
                let target = folder.clone();
                parent
                    .spawn(button(
                        ButtonProps::new(name).with_variant(ButtonVariant::Ghost),
                    ))
                    .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
                        let target = target.clone();
                        commands.queue(move |world: &mut World| {
                            select_path(world, &target);
                        });
                    });
            }
            if let Some(name) = selected_name {
                parent.spawn(separator_text());
                parent.spawn((
                    Text::new(name),
                    TextFont {
                        font_size: tokens::TEXT_SIZE,
                        ..default()
                    },
                    TextColor(tokens::TEXT_PRIMARY),
                ));
            }
        })
        .id();
    jackdaw_feathers::utils::attach_or_despawn(commands, bar, row);
}

fn separator_text() -> impl Bundle {
    (
        Text::new(" / "),
        TextFont {
            font_size: tokens::TEXT_SIZE,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
    )
}

// -- Selecting and opening --------------------------------------------------

/// Select a path: a folder takes the tiles into it, and a file takes the
/// inspector to that file's card without opening a tab.
pub fn select_path(world: &mut World, path: &Path) {
    let path = crate::definition_assets::resolve_project_path(world, path);
    if path.is_dir() {
        if let Some(mut state) = world.get_resource_mut::<ProjectWindowState>() {
            state.show_folder(path.clone());
        }
        crate::inspector::file_card::clear_selected_file(world);
        return;
    }
    let folder = path.parent().map(Path::to_path_buf);
    if let Some(mut state) = world.get_resource_mut::<ProjectWindowState>() {
        if let Some(folder) = folder {
            state.expand_to(&folder);
            if folder != state.current_directory {
                state.current_directory = folder;
            }
        }
        state.selected_file = Some(path.to_string_lossy().into_owned());
        state.renaming = None;
        state.needs_refresh = true;
    }
    crate::inspector::file_card::show_file(world, &path);
}

/// Open a path: a folder takes the tiles into it, a scene or prefab opens in a
/// tab, an asset file opens its card, and an image is applied to the selection.
fn open_path(world: &mut World, path: &Path, is_directory: bool) {
    let path = crate::definition_assets::resolve_project_path(world, path);
    if is_directory || path.is_dir() {
        select_path(world, &path);
        return;
    }

    let holds_an_asset = {
        let kinds = world.resource::<AssetKinds>();
        crate::asset_files::read_asset_kind(&path, kinds)
            .type_path()
            .is_some_and(|type_path| kinds.by_type_path(type_path).is_some())
    };
    if holds_an_asset {
        crate::definition_assets::open_definition_file(world, &path);
        return;
    }

    let is_document = jackdaw_bsn::is_document_path(&path)
        || path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("jsn"));
    if is_document {
        crate::scenes::operators::scene_open_system(world, &path);
        return;
    }

    if crate::texture_files::is_image_file_path(&path) {
        let plain_2d = !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("ktx2"))
            || !crate::texture_files::is_ktx2_non_2d(&path);
        if plain_2d {
            let call = world
                .operator("material.apply_texture")
                .param("path", path.to_string_lossy().into_owned())
                .settings(CallOperatorSettings {
                    creates_history_entry: true,
                    ..default()
                })
                .call();
            if let Err(err) = call {
                warn!("project.open: the texture could not be applied: {err}");
            }
        }
    }
}

fn on_file_double_click(event: On<FileItemDoubleClicked>, mut commands: Commands) {
    let path = PathBuf::from(&event.path);
    let is_directory = event.is_directory;
    commands.queue(move |world: &mut World| {
        open_path(world, &path, is_directory);
    });
}

// -- The context menu -------------------------------------------------------

const NEW_FOLDER_ACTION: &str = "project.new_folder";
const NEW_SCENE_ACTION: &str = "project.new_scene";
const NEW_ASSET_ACTION: &str = "project.new_asset";
const DUPLICATE_ACTION: &str = "project.duplicate";
const RENAME_ACTION: &str = "project.rename";
const REVEAL_ACTION: &str = "project.reveal";
const DELETE_ACTION: &str = "project.delete";
const CONVERT_TO_BINARY_ACTION: &str = "project.convert_to_binary";
const CONVERT_TO_TEXT_ACTION: &str = "project.convert_to_text";

fn open_context_menu(
    commands: &mut Commands,
    windows: &Query<&Window>,
    menu: &mut jackdaw_widgets::context_menu::ContextMenuState,
    path: &Path,
    is_directory: bool,
) {
    let cursor = windows
        .single()
        .ok()
        .and_then(bevy::prelude::Window::cursor_position)
        .unwrap_or_default();
    if let Some(open) = menu.menu_entity.take()
        && let Ok(mut entity) = commands.get_entity(open)
    {
        entity.despawn();
    }

    let mut items: Vec<(&str, &str)> = Vec::new();
    if is_directory {
        items.push((NEW_FOLDER_ACTION, "New Folder"));
        items.push((NEW_SCENE_ACTION, "New Scene"));
        items.push((NEW_ASSET_ACTION, crate::new_asset::NEW_ASSET_LABEL));
    }
    if jackdaw_bsn::is_document_path(path) {
        items.push(match jackdaw_bsn::is_binary_path(path) {
            true => (CONVERT_TO_TEXT_ACTION, "Convert to Text"),
            false => (CONVERT_TO_BINARY_ACTION, "Convert to Binary"),
        });
    }
    if !is_directory {
        items.push((DUPLICATE_ACTION, "Duplicate"));
    }
    items.push((RENAME_ACTION, "Rename"));
    items.push((REVEAL_ACTION, "Reveal in File Manager"));
    items.push((DELETE_ACTION, "Delete"));

    let entity = jackdaw_feathers::context_menu::spawn_context_menu(commands, cursor, None, &items);
    menu.menu_entity = Some(entity);
    commands.insert_resource(MenuTarget {
        path: path.to_path_buf(),
    });
}

fn on_context_action(
    event: On<jackdaw_widgets::context_menu::ContextMenuAction>,
    mut commands: Commands,
    target: Res<MenuTarget>,
    mut state: ResMut<ProjectWindowState>,
    mut menu: ResMut<jackdaw_widgets::context_menu::ContextMenuState>,
) {
    let action = event.action.as_str();
    if action == RENAME_ACTION {
        state.renaming = Some(target.path.clone());
        state.needs_refresh = true;
    } else {
        let Some(operator) = operator_for_action(action) else {
            return;
        };
        // A new scene is named after a file that does not exist yet; every
        // other action is named after the file or folder it was raised on.
        let path = match operator {
            "scene.new" => unused_file(&target.path, "scene", "bsn"),
            _ => target.path.clone(),
        };
        commands
            .operator(operator)
            .param("path", path.to_string_lossy().into_owned())
            .call();
    }
    if let Some(open) = menu.menu_entity.take()
        && let Ok(mut entity) = commands.get_entity(open)
    {
        entity.despawn();
    }
}

/// The operator a context-menu action runs. The ids the two retired panels
/// used are answered here too, so a menu or keymap that still names them works.
fn operator_for_action(action: &str) -> Option<&'static str> {
    let tail = action
        .strip_prefix("project.")
        .or_else(|| action.strip_prefix("asset_browser."))
        .or_else(|| action.strip_prefix("project_files."))?;
    match tail {
        "new_folder" => Some(ProjectNewFolderOp::ID),
        "new_scene" => Some("scene.new"),
        "new_asset" => Some(crate::new_asset::AssetNewPickerOp::ID),
        "duplicate" => Some("asset.duplicate"),
        "reveal" => Some(ProjectRevealOp::ID),
        "delete" => Some("file.delete"),
        "convert_to_binary" => Some("file.convert_to_binary"),
        "convert_to_text" => Some("file.convert_to_text"),
        _ => None,
    }
}

fn on_rename_commit(
    event: On<TextEditCommitEvent>,
    fields: Query<&RenameField>,
    child_of: Query<&ChildOf>,
    mut commands: Commands,
) {
    let Some(field) = child_of
        .iter_ancestors(event.entity)
        .find_map(|ancestor| fields.get(ancestor).ok())
    else {
        return;
    };
    let path = field.0.to_string_lossy().into_owned();
    commands
        .operator(FileRenameOp::ID)
        .param("path", path)
        .param("name", event.text.clone())
        .call();
}

// -- The panel --------------------------------------------------------------

/// The Project window's contents: the folder tree, the tiles, and the bar over
/// them holding the path, the search and the kind filter.
pub fn project_panel_content(icon_font: Handle<Font>) -> impl Bundle {
    (
        ProjectWindowPanel,
        EditorEntity,
        Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_LG)),
            overflow: Overflow::clip(),
            ..default()
        },
        BackgroundColor(tokens::PANEL_BG),
        children![project_toolbar(icon_font), project_columns()],
    )
}

fn project_toolbar(icon_font: Handle<Font>) -> impl Bundle {
    (
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            width: percent(100),
            height: px(34.0),
            padding: UiRect::axes(px(tokens::SPACING_MD), px(tokens::SPACING_SM)),
            flex_shrink: 0.0,
            ..default()
        },
        children![
            (
                ProjectPathBar,
                EditorEntity,
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    overflow: Overflow::clip(),
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    ..default()
                },
            ),
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(tokens::SPACING_SM),
                    flex_shrink: 0.0,
                    ..default()
                },
                children![
                    (
                        Node {
                            width: px(140.0),
                            ..default()
                        },
                        children![(
                            ProjectKindFilterMenu,
                            ComboBoxSelectedIndex(0),
                            combobox_with_selected(
                                KindFilter::ALL
                                    .iter()
                                    .map(|filter| filter.label().to_string())
                                    .collect::<Vec<String>>(),
                                0,
                            ),
                        )],
                    ),
                    (
                        Node {
                            width: px(200.0),
                            ..default()
                        },
                        children![(
                            ProjectSearchInput,
                            text_edit(
                                TextEditProps::default()
                                    .with_placeholder("Search...")
                                    .allow_empty()
                            ),
                        )],
                    ),
                    assets_folder_button(icon_font),
                ],
            ),
        ],
    )
}

/// The control that points the window at another folder as the project's
/// assets.
fn assets_folder_button(icon_font: Handle<Font>) -> impl Bundle {
    (
        jackdaw_feathers::button::icon_button(
            jackdaw_feathers::button::IconButtonProps::new(icons::Icon::FolderOpen)
                .variant(ButtonVariant::Ghost),
            &icon_font,
        ),
        jackdaw_feathers::button::ButtonOperatorCall::new(AssetSelectFolderOp::ID),
    )
}

/// The two columns, with a handle between them the user can drag.
fn project_columns() -> impl Bundle {
    (
        EditorEntity,
        Node {
            flex_direction: FlexDirection::Row,
            width: percent(100),
            flex_grow: 1.0,
            min_height: px(0.0),
            border: UiRect::top(px(1.0)),
            ..default()
        },
        BorderColor::all(tokens::BORDER_SUBTLE),
        panel_group(
            0.12,
            (
                Spawn((
                    panel(0.28),
                    ProjectFolderTree,
                    EditorEntity,
                    Node {
                        flex_direction: FlexDirection::Column,
                        min_width: px(0.0),
                        min_height: px(0.0),
                        overflow: Overflow::scroll_y(),
                        padding: UiRect::all(px(tokens::SPACING_SM)),
                        ..default()
                    },
                )),
                Spawn(panel_handle()),
                Spawn((
                    panel(0.72),
                    ProjectFileGrid,
                    EditorEntity,
                    Node {
                        flex_direction: FlexDirection::Row,
                        flex_wrap: FlexWrap::Wrap,
                        align_content: AlignContent::FlexStart,
                        min_width: px(0.0),
                        min_height: px(0.0),
                        overflow: Overflow::scroll_y(),
                        padding: UiRect::all(px(tokens::SPACING_SM)),
                        row_gap: px(tokens::SPACING_XS),
                        column_gap: px(tokens::SPACING_XS),
                        ..default()
                    },
                    BorderColor::all(tokens::BORDER_SUBTLE),
                )),
            ),
        ),
    )
}

fn on_kind_filter_change(
    event: On<ComboBoxChangeEvent>,
    menus: Query<(), With<ProjectKindFilterMenu>>,
    mut state: ResMut<ProjectWindowState>,
) {
    if !menus.contains(event.entity) {
        return;
    }
    let Some(filter) = KindFilter::ALL.get(event.selected).copied() else {
        return;
    };
    if state.kind_filter != filter {
        state.kind_filter = filter;
        state.needs_refresh = true;
    }
}

// -- Operators --------------------------------------------------------------

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ProjectSelectOp>()
        .register_operator::<ProjectOpenOp>()
        .register_operator::<ProjectFilterOp>()
        .register_operator::<ProjectSearchOp>()
        .register_operator::<ProjectRevealOp>()
        .register_operator::<ProjectNewFolderOp>()
        .register_operator::<FileRenameOp>()
        .register_operator::<AssetSelectFolderOp>();
}

/// Select a file or folder in the Project window.
#[operator(
    id = "project.select",
    label = "Select in Project",
    description = "Select a file, putting its card in the inspector, or show a folder.",
    allows_undo = false,
    params(path(String, doc = "File or folder, as a path under the project."))
)]
pub fn project_select(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let Some(path) = params.as_str("path").map(PathBuf::from) else {
        warn!("project.select: no path given");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        select_path(world, &path);
    });
    OperatorResult::Finished
}

/// Open a file the way a double click does.
#[operator(
    id = "project.open",
    label = "Open in Project",
    description = "Open a file: a tab for a scene or prefab, and the kind's own action otherwise.",
    allows_undo = false,
    params(path(String, doc = "File or folder, as a path under the project."))
)]
pub fn project_open(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let Some(path) = params.as_str("path").map(PathBuf::from) else {
        warn!("project.open: no path given");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        open_path(world, &path, false);
    });
    OperatorResult::Finished
}

/// Narrow the tiles to one sort of file.
#[operator(
    id = "project.filter",
    label = "Filter Project Files",
    description = "Show only files of one kind: all, scenes, prefabs, materials, definitions, \
                   images or audio.",
    allows_undo = false,
    params(kind(String, default = "all", doc = "The kind to show, or all."))
)]
pub fn project_filter(
    params: In<OperatorParameters>,
    mut state: ResMut<ProjectWindowState>,
) -> OperatorResult {
    let asked = params.as_str("kind").unwrap_or("all");
    let Some(filter) = KindFilter::from_id(asked) else {
        warn!("project.filter: no kind called '{asked}'");
        return OperatorResult::Cancelled;
    };
    state.kind_filter = filter;
    state.needs_refresh = true;
    OperatorResult::Finished
}

/// Narrow the tiles to the files whose name holds some text.
#[operator(
    id = "project.search",
    label = "Search Project Files",
    description = "Show only the files whose name holds this text.",
    allows_undo = false,
    params(text(
        String,
        default = "",
        doc = "The text to look for. Empty shows everything."
    ))
)]
pub fn project_search(
    params: In<OperatorParameters>,
    mut state: ResMut<ProjectWindowState>,
) -> OperatorResult {
    state.search = params.as_str("text").unwrap_or_default().to_string();
    state.needs_refresh = true;
    OperatorResult::Finished
}

/// Show a file where it sits, in the desktop's own file manager.
#[operator(
    id = "project.reveal",
    label = "Reveal in File Manager",
    description = "Open the desktop's file manager on the folder holding a file.",
    allows_undo = false,
    params(path(String, doc = "File or folder to reveal."))
)]
pub fn project_reveal(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let Some(path) = params.as_str("path").map(PathBuf::from) else {
        warn!("project.reveal: no path given");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let path = crate::definition_assets::resolve_project_path(world, &path);
        let folder = if path.is_dir() {
            path.clone()
        } else {
            path.parent().map(Path::to_path_buf).unwrap_or(path)
        };
        if let Err(err) = show_in_file_manager(&folder) {
            warn!(
                "project.reveal: {} cannot be opened: {err}",
                folder.display()
            );
        }
    });
    OperatorResult::Finished
}

/// Hand a folder to whatever the desktop opens folders with.
fn show_in_file_manager(folder: &Path) -> std::io::Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program).arg(folder).spawn()?;
    Ok(())
}

/// Make a folder under another one.
#[operator(
    id = "project.new_folder",
    label = "New Folder",
    description = "Make a folder under the one named, under a name nothing else holds.",
    allows_undo = false,
    params(
        path(String, doc = "The folder to make one in."),
        name(String, default = "New Folder", doc = "What to call it.")
    )
)]
pub fn project_new_folder(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(path) = params.as_str("path").map(PathBuf::from) else {
        warn!("project.new_folder: no path given");
        return OperatorResult::Cancelled;
    };
    let name = params.as_str("name").unwrap_or("New Folder").to_string();
    commands.queue(move |world: &mut World| {
        let parent = crate::definition_assets::resolve_project_path(world, &path);
        let parent = if parent.is_dir() {
            parent
        } else {
            match parent.parent() {
                Some(parent) => parent.to_path_buf(),
                None => return,
            }
        };
        let folder = unused_name(&parent, &name);
        if let Err(err) = std::fs::create_dir_all(&folder) {
            warn!(
                "project.new_folder: {} cannot be made: {err}",
                folder.display()
            );
            return;
        }
        if let Some(mut state) = world.get_resource_mut::<ProjectWindowState>() {
            state.rebuild();
        }
    });
    OperatorResult::Finished
}

/// The first of `name_1.ext`, `name_2.ext` that nothing in `parent` holds.
fn unused_file(parent: &Path, name: &str, extension: &str) -> PathBuf {
    for counter in 1..1000 {
        let candidate = parent.join(format!("{name}_{counter}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{name}.{extension}"))
}

/// The first of `name`, `name 2`, `name 3` that nothing in `parent` holds.
fn unused_name(parent: &Path, name: &str) -> PathBuf {
    let first = parent.join(name);
    if !first.exists() {
        return first;
    }
    for counter in 2..1000 {
        let candidate = parent.join(format!("{name} {counter}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

/// Rename a file or folder where it sits.
#[operator(
    id = "file.rename",
    label = "Rename",
    description = "Give a file or folder another name, in the folder it already sits in.",
    allows_undo = false,
    params(
        path(String, doc = "The file or folder to rename."),
        name(String, doc = "The name to give it, without a folder.")
    )
)]
pub fn file_rename(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let Some(path) = params.as_str("path").map(PathBuf::from) else {
        warn!("file.rename: no path given");
        return OperatorResult::Cancelled;
    };
    let Some(name) = params.as_str("name").map(str::to_string) else {
        warn!("file.rename: no name given");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        rename_file(world, &path, &name);
    });
    OperatorResult::Finished
}

fn rename_file(world: &mut World, path: &Path, name: &str) {
    let path = crate::definition_assets::resolve_project_path(world, path);
    let name = name.trim();
    if name.is_empty() || name.contains(std::path::is_separator) {
        crate::status_bar::notify_error(world, format!("'{name}' cannot name a file"));
        return;
    }
    let Some(parent) = path.parent() else {
        return;
    };
    let target = parent.join(name);
    if target == path {
        clear_rename(world);
        return;
    }
    if target.exists() {
        crate::status_bar::notify_error(world, format!("'{name}' is already taken"));
        return;
    }
    if let Err(err) = std::fs::rename(&path, &target) {
        crate::status_bar::notify_error(world, format!("'{name}' could not be written: {err}"));
        return;
    }
    if let Some(mut state) = world.get_resource_mut::<ProjectWindowState>() {
        if state.current_directory.starts_with(&path) {
            state.current_directory = target.clone();
        }
        state.expanded.remove(&path);
        state.renaming = None;
        state.rebuild();
    }
    // The card was standing for the old path, and a definition card holds that
    // path to save through, so both are put back on the file under its new name.
    select_path(world, &target);
}

fn clear_rename(world: &mut World) {
    if let Some(mut state) = world.get_resource_mut::<ProjectWindowState>() {
        state.renaming = None;
        state.needs_refresh = true;
    }
}

/// Choose a different folder as the project's assets.
#[operator(
    id = "asset.select_folder",
    label = "Select Assets Folder",
    description = "Choose a different folder as the assets directory."
)]
pub fn asset_select_folder(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        if world.contains_resource::<ProjectFolderTask>() {
            return;
        }
        let current = world
            .get_resource::<ProjectWindowState>()
            .map(|state| state.current_directory.clone());
        let dialog = crate::native_dialog::dialog_starting_at(world, current)
            .set_title("Select assets directory");
        let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_folder().await });
        world.insert_resource(ProjectFolderTask(task));
    });
    OperatorResult::Finished
}

fn poll_project_folder_pick(world: &mut World) {
    let Some(mut task) = world.get_resource_mut::<ProjectFolderTask>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut task.0)) else {
        return;
    };
    world.remove_resource::<ProjectFolderTask>();
    let Some(handle) = result else {
        return;
    };
    let path = handle.path().to_path_buf();
    {
        let mut state = world.resource_mut::<ProjectWindowState>();
        state.root_directory = path.clone();
        state.expanded.clear();
        state.expanded.insert(path.clone());
        state.show_folder(path.clone());
        state.needs_tree_refresh = true;
    }
    let mut commands = world.commands();
    watch_directory(&path, &mut commands);
    world.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: AssetFileKind) -> DirEntry {
        DirEntry {
            path: PathBuf::from("/project/assets").join(name),
            file_name: name.to_string(),
            is_directory: false,
            texture_info: None,
            kind,
            folder: None,
        }
    }

    #[test]
    fn every_filter_is_named_by_a_word_a_caller_can_spell() {
        for filter in KindFilter::ALL {
            assert_eq!(
                KindFilter::from_id(filter.id()),
                Some(filter),
                "{} does not read back",
                filter.id()
            );
        }
    }

    #[test]
    fn a_folder_survives_every_filter() {
        let mut folder = entry("content", AssetFileKind::Scene);
        folder.is_directory = true;
        let kinds = AssetKinds::default();
        for filter in KindFilter::ALL {
            assert!(filter.keeps(&folder, &kinds), "{}", filter.id());
        }
    }

    #[test]
    fn the_prefab_filter_keeps_prefabs_and_drops_scenes() {
        let kinds = AssetKinds::default();
        assert!(KindFilter::Prefabs.keeps(&entry("rat.bsn", AssetFileKind::Prefab), &kinds));
        assert!(!KindFilter::Prefabs.keeps(&entry("town.bsn", AssetFileKind::Scene), &kinds));
    }

    #[test]
    fn the_image_filter_reads_the_extension() {
        let kinds = AssetKinds::default();
        assert!(KindFilter::Images.keeps(&entry("bark.png", AssetFileKind::Scene), &kinds));
        assert!(!KindFilter::Images.keeps(&entry("town.bsn", AssetFileKind::Scene), &kinds));
    }

    #[derive(Resource, Default)]
    struct RowStore(Option<Entity>);

    fn spawn_a_folder_row(mut commands: Commands, mut store: ResMut<RowStore>) {
        let parent = commands.spawn(Node::default()).id();
        spawn_folder_row(
            &mut commands,
            parent,
            Path::new("/project/assets"),
            false,
            &HashSet::new(),
        );
        store.0 = Some(parent);
    }

    fn app_with_a_folder_row() -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ))
        .init_asset::<Image>()
        .add_observer(on_folder_toggled)
        .add_observer(on_folder_disclosure_change)
        .init_resource::<ProjectWindowState>()
        .init_resource::<RowStore>();
        let system = app.world_mut().register_system(spawn_a_folder_row);
        app.world_mut().run_system(system).expect("the row spawns");
        app.world_mut().flush();
        app
    }

    /// A folder row opens on a feathers disclosure toggle, and the row's open
    /// state is that toggle's `Checked`.
    #[test]
    fn a_folder_row_opens_on_a_feathers_disclosure_toggle() {
        let mut app = app_with_a_folder_row();

        let mut toggles = app.world_mut().query_filtered::<(Entity, &FolderDisclosure), (
            With<FeathersDisclosureToggle>,
            With<TreeNodeExpandToggle>,
        )>();
        let (disclosure, link) = toggles
            .iter(app.world())
            .next()
            .expect("the folder row carries a feathers disclosure toggle");
        let row = link.0;
        assert!(
            app.world().get::<Checked>(disclosure).is_none(),
            "a folder starts closed"
        );

        app.world_mut().trigger(ValueChange {
            source: disclosure,
            value: true,
            is_final: true,
        });
        app.world_mut().flush();

        assert!(
            app.world().get::<Checked>(disclosure).is_some(),
            "opening the folder checks its toggle"
        );
        assert!(
            app.world()
                .get::<TreeNodeExpanded>(row)
                .is_some_and(|expanded| expanded.0),
            "opening the folder expands the row"
        );
    }

    /// A folder row's label is laid out to its own text, so it must not opt in
    /// to being cut down to the room the row has: a cut narrows the label, and
    /// the narrower label lowers the budget, until "assets" reads "a".
    #[test]
    fn a_folder_row_label_does_not_opt_in_to_the_ellipsis() {
        let mut app = app_with_a_folder_row();
        let mut labels = app
            .world_mut()
            .query_filtered::<Entity, With<TreeRowLabel>>();
        let label = labels
            .iter(app.world())
            .next()
            .expect("the folder row carries a label");
        assert!(
            app.world()
                .get::<jackdaw_feathers::tree_view::TreeRowLabelEllipsis>(label)
                .is_none(),
            "the label is never cut to fit",
        );
    }

    /// The two retired panels named their menu items under their own ids, and
    /// a menu or keymap that still spells one reaches what it always did.
    #[test]
    fn the_action_ids_the_retired_panels_used_still_resolve() {
        for prefix in ["project", "asset_browser", "project_files"] {
            assert_eq!(
                operator_for_action(&format!("{prefix}.delete")),
                Some("file.delete")
            );
            assert_eq!(
                operator_for_action(&format!("{prefix}.convert_to_binary")),
                Some("file.convert_to_binary")
            );
            assert_eq!(
                operator_for_action(&format!("{prefix}.new_asset")),
                Some(crate::new_asset::AssetNewPickerOp::ID)
            );
        }
        assert_eq!(operator_for_action("something.else"), None);
    }

    /// A search at the root answers for the whole tree below it, and each
    /// result says which folder it came from.
    #[test]
    fn a_search_at_the_root_finds_a_file_two_folders_down() {
        let temp = tempfile::tempdir().expect("tempdir");
        let deep = temp.path().join("content").join("mobs");
        std::fs::create_dir_all(&deep).expect("the folders are made");
        std::fs::write(deep.join("giant_rat.bsn"), "rat").expect("the file is written");
        std::fs::write(temp.path().join("town.bsn"), "town").expect("the file is written");

        let mut state = ProjectWindowState::at(temp.path());
        state.search = "rat".to_string();
        let entries = scan_current_directory(&mut state, &AssetKinds::default());

        let found = entries
            .iter()
            .find(|entry| entry.file_name == "giant_rat.bsn")
            .expect("the search reaches two folders down");
        assert_eq!(
            found.folder.as_deref(),
            Some("content/mobs"),
            "the result carries the folder it was found in"
        );
        assert!(
            !entries.iter().any(|entry| entry.file_name == "town.bsn"),
            "the search still narrows what is shown"
        );
    }

    /// A tree too big to read in full says so, rather than quietly answering
    /// for the part of it the walk reached.
    #[test]
    fn a_search_that_gives_up_early_says_so() {
        let temp = tempfile::tempdir().expect("tempdir");
        let deep = temp.path().join("content");
        std::fs::create_dir_all(&deep).expect("the folder is made");
        for index in 0..4 {
            std::fs::write(deep.join(format!("rat_{index}.bsn")), "rat")
                .expect("the file is written");
        }

        let (found, stopped) = walk_below(temp.path(), 2);
        assert!(stopped, "the walk gave up before the bottom of the tree");
        assert!(found.len() < 4, "and answered for only part of it");

        let (found, stopped) = walk_below(temp.path(), 100);
        assert!(!stopped, "a tree it can read in full is not cut short");
        assert_eq!(found.len(), 4);
    }

    /// With nothing narrowing the view the tiles are the folder itself, so a
    /// deep file is not dragged up into it.
    #[test]
    fn an_unnarrowed_folder_lists_only_its_own_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let deep = temp.path().join("content");
        std::fs::create_dir_all(&deep).expect("the folder is made");
        std::fs::write(deep.join("giant_rat.bsn"), "rat").expect("the file is written");

        let mut state = ProjectWindowState::at(temp.path());
        let entries = scan_current_directory(&mut state, &AssetKinds::default());

        assert!(
            !entries
                .iter()
                .any(|entry| entry.file_name == "giant_rat.bsn"),
            "a file a folder down is reached by opening the folder"
        );
        assert!(entries.iter().any(|entry| entry.is_directory));
    }

    /// The tree says where the tiles are: the row of the folder shown is
    /// marked, and only that one.
    #[test]
    fn the_tree_marks_the_shown_folder() {
        let mut app = app_with_a_folder_row();
        app.add_systems(Update, mark_the_shown_folder);

        let folder = PathBuf::from("/project/assets");
        app.world_mut()
            .resource_mut::<ProjectWindowState>()
            .current_directory = folder.clone();
        app.update();

        let mut marked = app
            .world_mut()
            .query_filtered::<Entity, With<ShownFolder>>();
        assert_eq!(
            marked.iter(app.world()).count(),
            1,
            "the folder the tiles are showing is the marked one"
        );

        app.world_mut()
            .resource_mut::<ProjectWindowState>()
            .current_directory = PathBuf::from("/project/assets/elsewhere");
        app.update();

        let mut marked = app
            .world_mut()
            .query_filtered::<Entity, With<ShownFolder>>();
        assert_eq!(
            marked.iter(app.world()).count(),
            0,
            "a folder the tiles left is no longer marked"
        );
    }

    /// Selecting something deep in the tree opens the rows down to it, so it
    /// is not shown under a closed ancestor.
    #[test]
    fn project_select_expands_the_tree() {
        let root = PathBuf::from("/project/assets");
        let deep = root.join("content").join("mobs");
        let mut state = ProjectWindowState::at(root.clone());
        assert!(!state.is_expanded(&deep));

        state.show_folder(deep.clone());

        assert!(state.is_expanded(&deep), "the folder shown is open");
        assert!(
            state.is_expanded(&root.join("content")),
            "and so is every row on the way down to it"
        );
        assert!(state.needs_tree_refresh, "the tree is rebuilt to show them");
    }

    /// A prefab is pictured, a material is pictured on a sphere, and a folder
    /// keeps its icon.
    #[test]
    fn a_prefab_tile_is_pictured_and_a_folder_is_not() {
        let kinds = AssetKinds::default();
        assert_eq!(
            tile_subject(&entry("rat.bsn", AssetFileKind::Prefab), &kinds),
            Some(crate::thumbnail::Subject::Prefab)
        );
        assert_eq!(
            tile_subject(&entry("town.bsn", AssetFileKind::Scene), &kinds),
            Some(crate::thumbnail::Subject::Scene)
        );
        assert_eq!(
            tile_subject(&entry("tree.glb", AssetFileKind::Scene), &kinds),
            Some(crate::thumbnail::Subject::Model)
        );

        let mut folder = entry("content", AssetFileKind::Scene);
        folder.is_directory = true;
        assert_eq!(tile_subject(&folder, &kinds), None);
        assert_eq!(
            tile_subject(&entry("bark.png", AssetFileKind::Scene), &kinds),
            None
        );
    }

    #[test]
    fn a_name_already_taken_gets_a_number() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let first = unused_name(tmp.path(), "New Folder");
        std::fs::create_dir_all(&first).expect("the folder is made");
        let second = unused_name(tmp.path(), "New Folder");
        assert_eq!(second.file_name().unwrap(), "New Folder 2");
    }
}
