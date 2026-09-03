//! Project Files panel: a file tree view with live filesystem watching.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, mpsc};

use bevy::feathers::controls::FeathersDisclosureToggle;
use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::ValueChange;
use jackdaw_api::prelude::*;
use jackdaw_feathers::{file_browser, icons::IconFont, tokens};
use jackdaw_widgets::tree_view::{
    TreeChildrenPopulated, TreeNodeExpandToggle, TreeNodeExpanded, TreeRowChildren, TreeRowContent,
    TreeRowLabel,
};

// EditorEntity not needed for project file nodes

pub struct ProjectFilesPlugin;

impl Plugin for ProjectFilesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ProjectFilesState>()
            .init_resource::<PendingProjectFilesAction>()
            .add_systems(OnEnter(crate::AppState::Editor), setup_project_files)
            .add_systems(
                Update,
                (check_project_watcher, refresh_project_tree)
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_observer(handle_directory_expand)
            .add_observer(on_directory_toggled)
            .add_observer(on_directory_disclosure_change)
            .add_observer(on_project_files_context_action);
    }
}

/// Records the path the project files context menu was opened on so
/// the action observer can resolve the click.
#[derive(Resource, Default)]
struct PendingProjectFilesAction {
    path: PathBuf,
}

fn on_project_files_context_action(
    event: On<jackdaw_widgets::context_menu::ContextMenuAction>,
    mut commands: Commands,
    pending: Res<PendingProjectFilesAction>,
    mut state: ResMut<jackdaw_widgets::context_menu::ContextMenuState>,
) {
    if event.action != "project_files.delete" {
        return;
    }
    let path = pending.path.to_string_lossy().into_owned();
    commands.operator("file.delete").param("path", path).call();
    if let Some(menu) = state.menu_entity.take()
        && let Ok(mut ec) = commands.get_entity(menu)
    {
        ec.despawn();
    }
}

/// State for the project files panel.
#[derive(Resource, Default)]
pub struct ProjectFilesState {
    pub root_directory: PathBuf,
    pub needs_refresh: bool,
    pub initialized: bool,
}

/// Marker on the project files tree container.
#[derive(Component)]
pub struct ProjectFilesTree;

/// Component on tree nodes representing a filesystem path.
#[derive(Component)]
pub struct ProjectFileNode(pub PathBuf);

/// Marker for directory nodes (have expandable children).
#[derive(Component)]
pub struct ProjectFileIsDir;

/// File watcher resource for the project root.
#[derive(Resource)]
struct ProjectFileWatcher {
    _watcher: notify::RecommendedWatcher,
    receiver: Mutex<mpsc::Receiver<()>>,
}

/// Initial setup: read project root and set up file watcher.
fn setup_project_files(
    project_root: Option<Res<crate::project::ProjectRoot>>,
    mut state: ResMut<ProjectFilesState>,
    mut commands: Commands,
) {
    let root = project_root
        .map(|p| p.root.clone())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    state.root_directory = root.clone();
    state.needs_refresh = true;
    state.initialized = false;

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
    if let Ok(mut w) = watcher {
        use notify::Watcher;
        if w.watch(&root, notify::RecursiveMode::Recursive).is_ok() {
            commands.insert_resource(ProjectFileWatcher {
                _watcher: w,
                receiver: Mutex::new(rx),
            });
        }
    }
}

/// Poll the file watcher for changes.
fn check_project_watcher(
    watcher: Option<Res<ProjectFileWatcher>>,
    mut state: ResMut<ProjectFilesState>,
) {
    let Some(watcher) = watcher else { return };
    let Ok(rx) = watcher.receiver.lock() else {
        return;
    };
    if rx.try_recv().is_ok() {
        // Drain any additional pending events
        while rx.try_recv().is_ok() {}
        state.needs_refresh = true;
    }
}

/// Rebuild the root-level tree when `needs_refresh` is set.
fn refresh_project_tree(
    mut state: ResMut<ProjectFilesState>,
    tree_query: Query<(Entity, Option<&Children>), With<ProjectFilesTree>>,
    mut commands: Commands,
    icon_font: Option<Res<IconFont>>,
) {
    if !state.needs_refresh {
        return;
    }
    state.needs_refresh = false;

    let Ok((tree_entity, existing_children)) = tree_query.single() else {
        return;
    };

    // Clear existing children
    if let Some(children) = existing_children {
        for child in children.iter() {
            commands.entity(child).despawn();
        }
    }

    let Some(icon_font) = icon_font else { return };

    // Scan root directory
    let root = &state.root_directory;
    if !root.is_dir() {
        return;
    }

    let mut entries = scan_directory(root);
    entries.sort_by(|a, b| {
        // Directories first, then alphabetical
        b.1.cmp(&a.1).then_with(|| {
            a.0.file_name()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .cmp(&b.0.file_name().unwrap_or_default().to_ascii_lowercase())
        })
    });

    for (path, is_dir) in entries {
        spawn_file_tree_row(&mut commands, tree_entity, &path, is_dir, &icon_font.0);
    }

    state.initialized = true;
}

/// Handle directory expansion: lazily populate children.
fn handle_directory_expand(
    event: On<bevy::picking::events::Pointer<bevy::picking::events::Click>>,
    toggle_query: Query<&ChildOf, With<TreeNodeExpandToggle>>,
    content_query: Query<&ChildOf, With<TreeRowContent>>,
    mut tree_nodes: Query<(
        &mut TreeNodeExpanded,
        &mut TreeChildrenPopulated,
        &Children,
        &ProjectFileNode,
    )>,
    children_containers: Query<Entity, With<TreeRowChildren>>,
    mut commands: Commands,
    icon_font: Option<Res<IconFont>>,
    file_dirs: Query<(), With<ProjectFileIsDir>>,
) {
    let clicked = event.event_target();

    // Walk up: click target -> TreeRowContent -> TreeNode
    let tree_node_entity = if let Ok(toggle_parent) = toggle_query.get(clicked) {
        // Clicked on the expand toggle itself
        let content_entity = toggle_parent.parent();
        if let Ok(content_parent) = content_query.get(content_entity) {
            content_parent.parent()
        } else {
            return;
        }
    } else if let Ok(content_parent) = content_query.get(clicked) {
        // Clicked on the content row
        content_parent.parent()
    } else {
        return;
    };

    // Only handle directory nodes
    if file_dirs.get(tree_node_entity).is_err() {
        return;
    }

    let Ok((mut expanded, mut populated, children, file_node)) =
        tree_nodes.get_mut(tree_node_entity)
    else {
        return;
    };

    // Toggle expanded state
    expanded.0 = !expanded.0;

    // Find the TreeRowChildren container
    let Some(children_entity) = children
        .iter()
        .find(|c| children_containers.get(*c).is_ok())
    else {
        return;
    };

    if expanded.0 && !populated.0 {
        // First expansion: scan and populate children
        populated.0 = true;

        let Some(icon_font) = icon_font else { return };
        let dir_path = &file_node.0;

        let mut entries = scan_directory(dir_path);
        entries.sort_by(|a, b| {
            b.1.cmp(&a.1).then_with(|| {
                a.0.file_name()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .cmp(&b.0.file_name().unwrap_or_default().to_ascii_lowercase())
            })
        });

        for (path, is_dir) in entries {
            spawn_file_tree_row(&mut commands, children_entity, &path, is_dir, &icon_font.0);
        }
    }
}

/// Scan a directory and return (path, `is_directory`) entries.
fn scan_directory(dir: &Path) -> Vec<(PathBuf, bool)> {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    read_dir
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            let is_dir = path.is_dir();
            // Skip hidden files/directories (starting with .)
            let name = path.file_name()?.to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            // Skip target directory
            if name == "target" {
                return None;
            }
            Some((path, is_dir))
        })
        .collect()
}

/// Links a directory row's disclosure control to the row it opens.
#[derive(Component)]
struct DirectoryDisclosure(Entity);

/// Open or close one directory row. Both the row click and the disclosure
/// control raise this, so the two agree on what a toggle does.
#[derive(EntityEvent)]
struct ToggleDirectory {
    entity: Entity,
}

fn on_directory_disclosure_change(
    change: On<ValueChange<bool>>,
    disclosures: Query<&DirectoryDisclosure>,
    mut commands: Commands,
) {
    let Ok(row) = disclosures.get(change.source) else {
        return;
    };
    commands.trigger(ToggleDirectory { entity: row.0 });
}

/// Flip the row's expanded flag, show or hide its children container, and
/// point the disclosure the way the row now stands.
fn on_directory_toggled(
    event: On<ToggleDirectory>,
    mut expanded_query: Query<&mut TreeNodeExpanded>,
    children_query: Query<&Children>,
    children_containers: Query<(), With<TreeRowChildren>>,
    contents: Query<(), With<TreeRowContent>>,
    toggles: Query<(), With<TreeNodeExpandToggle>>,
    mut nodes: Query<&mut Node>,
    mut commands: Commands,
) {
    let row = event.entity;
    let Ok(mut expanded) = expanded_query.get_mut(row) else {
        return;
    };
    expanded.0 = !expanded.0;
    let is_expanded = expanded.0;

    let Ok(children) = children_query.get(row) else {
        return;
    };
    for child in children.iter() {
        if children_containers.contains(child)
            && let Ok(mut node) = nodes.get_mut(child)
        {
            node.display = if is_expanded {
                Display::Flex
            } else {
                Display::None
            };
        }
        if !contents.contains(child) {
            continue;
        }
        let Ok(content_children) = children_query.get(child) else {
            continue;
        };
        for toggle in content_children.iter() {
            if !toggles.contains(toggle) {
                continue;
            }
            let mut toggle = commands.entity(toggle);
            if is_expanded {
                toggle.insert(Checked);
            } else {
                toggle.remove::<Checked>();
            }
        }
    }
}

/// Spawn a single file/directory tree row.
fn spawn_file_tree_row(
    commands: &mut Commands,
    parent: Entity,
    path: &Path,
    is_dir: bool,
    icon_font: &Handle<Font>,
) {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let node_entity = commands
        .spawn((
            // Use the node entity itself as the "source" since we don't have scene entities
            ProjectFileNode(path.to_path_buf()),
            TreeNodeExpanded(false),
            TreeChildrenPopulated(false),
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    // Note: We intentionally do NOT add TreeNode(self) here. TreeNode is a
    // relationship component that would warn about self-referencing. Project file
    // nodes use ProjectFileNode instead of TreeNode for identification.

    if is_dir {
        commands.entity(node_entity).insert(ProjectFileIsDir);
    }

    // Clickable row content
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
                ..Default::default()
            },
            ChildOf(node_entity),
        ))
        .id();

    // Hover effects
    commands.entity(content).observe(
        |hover: On<Pointer<Over>>, mut bg: Query<&mut BackgroundColor>| {
            if let Ok(mut bg) = bg.get_mut(hover.event_target()) {
                bg.0 = tokens::HOVER_BG;
            }
        },
    );
    commands.entity(content).observe(
        |out: On<Pointer<Out>>, mut bg: Query<&mut BackgroundColor>| {
            if let Ok(mut bg) = bg.get_mut(out.event_target()) {
                bg.0 = Color::NONE;
            }
        },
    );

    // Right-click: open a per-row context menu with a Delete entry.
    let rmb_path = path.to_path_buf();
    commands.entity(content).observe(
        move |click: On<Pointer<Click>>,
              mut commands: Commands,
              windows: Query<&Window>,
              mut state: ResMut<jackdaw_widgets::context_menu::ContextMenuState>| {
            if click.event().button != PointerButton::Secondary {
                return;
            }
            let cursor_pos = windows
                .single()
                .ok()
                .and_then(bevy::prelude::Window::cursor_position)
                .unwrap_or_default();
            if let Some(existing) = state.menu_entity.take()
                && let Ok(mut ec) = commands.get_entity(existing)
            {
                ec.despawn();
            }
            let path_owned = rmb_path.clone();
            let menu = jackdaw_feathers::context_menu::spawn_context_menu(
                &mut commands,
                cursor_pos,
                None,
                &[("project_files.delete", "Delete")],
            );
            state.menu_entity = Some(menu);
            commands.insert_resource(PendingProjectFilesAction { path: path_owned });
        },
    );

    if is_dir {
        commands
            .spawn_scene(bsn! { @FeathersDisclosureToggle })
            .insert((
                TreeNodeExpandToggle,
                DirectoryDisclosure(node_entity),
                ChildOf(content),
            ));

        // Directory label (no icon, just text)
        commands.spawn((
            TreeRowLabel,
            Text::new(file_name),
            TextFont {
                font_size: tokens::TEXT_SIZE,
                ..Default::default()
            },
            TextColor(tokens::TEXT_PRIMARY),
            ChildOf(content),
        ));

        // Children container (initially hidden)
        commands.spawn((
            TreeRowChildren,
            Node {
                flex_direction: FlexDirection::Column,
                padding: UiRect::left(Val::Px(16.0)),
                margin: UiRect::left(Val::Px(tokens::SPACING_SM)),
                border: UiRect::left(Val::Px(1.0)),
                width: Val::Percent(100.0),
                display: Display::None,
                ..Default::default()
            },
            BorderColor::all(tokens::CONNECTION_LINE),
            ChildOf(node_entity),
        ));

        let node_for_click = node_entity;
        commands
            .entity(content)
            .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
                commands.trigger(ToggleDirectory {
                    entity: node_for_click,
                });
            });
    } else {
        // File icon based on extension
        let icon = file_browser::file_icon(&file_name);

        commands.spawn((
            Text::new(String::from(icon.unicode())),
            TextFont {
                font: icon_font.clone().into(),
                font_size: tokens::ICON_SM,
                ..Default::default()
            },
            TextColor(tokens::FILE_ICON_COLOR),
            Node {
                width: Val::Px(15.0),
                flex_shrink: 0.0,
                ..Default::default()
            },
            ChildOf(content),
        ));

        // File label
        commands.spawn((
            TreeRowLabel,
            Text::new(file_name),
            TextFont {
                font_size: tokens::TEXT_SIZE,
                ..Default::default()
            },
            TextColor(tokens::TEXT_PRIMARY),
            ChildOf(content),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct RowStore(Option<Entity>);

    fn spawn_dir_row(mut commands: Commands, mut store: ResMut<RowStore>) {
        let parent = commands.spawn(Node::default()).id();
        spawn_file_tree_row(
            &mut commands,
            parent,
            Path::new("/project/assets"),
            true,
            &Handle::default(),
        );
        store.0 = Some(parent);
    }

    /// A directory row opens on a feathers disclosure toggle, and the row's
    /// open state is that toggle's `Checked`.
    #[test]
    fn a_directory_row_opens_on_a_feathers_disclosure_toggle() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ))
        .init_asset::<Image>()
        .add_observer(on_directory_toggled)
        .add_observer(on_directory_disclosure_change)
        .init_resource::<RowStore>();

        let system_id = app.world_mut().register_system(spawn_dir_row);
        app.world_mut().run_system(system_id).unwrap();
        app.world_mut().flush();

        let mut toggles = app.world_mut().query_filtered::<(
            Entity,
            &DirectoryDisclosure,
        ), (
            With<FeathersDisclosureToggle>,
            With<TreeNodeExpandToggle>,
        )>();
        let (disclosure, link) = toggles
            .iter(app.world())
            .next()
            .expect("the directory row carries a feathers disclosure toggle");
        let row = link.0;
        assert!(
            app.world().get::<Checked>(disclosure).is_none(),
            "a directory starts closed"
        );

        app.world_mut().trigger(ValueChange {
            source: disclosure,
            value: true,
            is_final: true,
        });
        app.world_mut().flush();

        assert!(
            app.world().get::<Checked>(disclosure).is_some(),
            "opening the directory checks its toggle"
        );
        assert!(
            app.world()
                .get::<TreeNodeExpanded>(row)
                .is_some_and(|expanded| expanded.0),
            "opening the directory expands the row"
        );
    }

    /// A file row's label is laid out to its own text, so it must not opt
    /// in to being cut down to the room the row has: a cut narrows the
    /// label, and the narrower label lowers the budget, until "assets"
    /// reads "a".
    #[test]
    fn a_row_label_does_not_opt_in_to_the_ellipsis() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ))
        .init_asset::<Image>()
        .init_resource::<RowStore>();

        let system_id = app.world_mut().register_system(spawn_dir_row);
        app.world_mut().run_system(system_id).unwrap();
        app.world_mut().flush();

        let mut labels = app.world_mut().query_filtered::<Entity, With<TreeRowLabel>>();
        let label = labels
            .iter(app.world())
            .next()
            .expect("the directory row carries a label");
        assert!(
            app.world()
                .get::<jackdaw_feathers::tree_view::TreeRowLabelEllipsis>(label)
                .is_none(),
            "the label is never cut to fit",
        );
    }
}
