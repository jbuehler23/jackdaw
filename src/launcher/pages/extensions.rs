//! The Extensions page: the modules, editor extensions and game crates of a
//! project, or the global defaults every project starts from.
//!
//! Global defaults are the editor's extension list. A project turns
//! extensions on or off over it in the `[extensions]` table of its
//! `jackdaw.toml`, applied when the project opens, and chooses its modules
//! in `[modules]` and its Cargo.toml. Changes wait as pending until Apply.

use std::collections::BTreeMap;
use std::path::Path;

use bevy::feathers::controls::{
    ButtonVariant, FeathersMenu, FeathersMenuButton, FeathersMenuItem, FeathersMenuPopup,
    FeathersToggleSwitch,
};
use bevy::feathers::theme::ThemedText;
use bevy::input_focus::tab_navigation::TabGroup;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui_widgets::Button;
use jackdaw_api_internal::lifecycle::ExtensionCatalog;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::tokens;
use jackdaw_project_build::modules::{GameDependencies, ModuleCatalog, ModuleSet};
use jackdaw_project_build::project_manifest::ProjectManifest;

use super::super::ui::{self, ToggleOperatorCall, Tone};
use super::super::{
    ExtensionFilter, ExtensionItem, ExtensionsView, Launcher, LauncherField, LauncherPage,
};
use super::TextSlot;

#[derive(Component, Default, Clone)]
pub struct ExtensionsRegion;

#[derive(Component, Default, Clone)]
pub struct ExtensionsScopeRegion;

pub fn page() -> impl Scene {
    bsn! {
        Node {
            flex_grow: 1.0,
            min_height: px(0),
            flex_direction: FlexDirection::Column,
            row_gap: px(12),
            padding: UiRect::axes(px(20), px(16)),
        }
        Children [
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(8),
            }
            Children [
                Text("Extensions and plugins")
                TextFont { font_size: tokens::TEXT_SIZE_LG }
                TextColor(tokens::TEXT_PRIMARY)
                --
                ExtensionsScopeRegion Node { flex_grow: 1.0, min_width: px(0) }
                --
                @{ui::dim("Filter")}
                --
                Node { width: px(200) }
                TextSlot { field: LauncherField::ExtensionQuery, placeholder: "Filter" }
            ]
            --
            ExtensionsRegion
            Node {
                flex_grow: 1.0,
                min_height: px(0),
                flex_direction: FlexDirection::Column,
            }
        ]
    }
}

/// One row on the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub item: ExtensionItem,
    pub name: String,
    pub description: String,
    /// One line beside the name.
    pub detail: String,
    /// On, counting pending changes.
    pub on: bool,
    pub pending: bool,
    /// Why it needs attention, if it does.
    pub attention: Option<String>,
    pub toggleable: bool,
    /// What turning it on changes in the game, for the details pane.
    pub changes: Vec<String>,
    /// Whether it is installed in this editor.
    pub installed: bool,
}

/// What the page lists for its scope.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub modules: Vec<Entry>,
    pub extensions: Vec<Entry>,
    /// The game's crates, with the module that provides each, if any.
    pub plugins: Vec<(String, Option<String>)>,
}

fn project_modules(root: &Path) -> ModuleSet {
    crate::scaffold::project_modules(root)
        .unwrap_or_else(|_| ProjectManifest::read(root).modules(root))
}

/// What turning module `id` on, or off, writes into the game.
fn module_changes(id: &str, on: bool) -> Vec<String> {
    let Some(module) = ModuleCatalog::builtin().get(id) else {
        return Vec::new();
    };
    let sign = if on { "+" } else { "-" };
    let mut changes = Vec::new();
    for feature in &module.runtime_features {
        changes.push(format!(
            "Cargo.toml {sign} jackdaw_runtime feature {feature}"
        ));
    }
    for dependency in &module.crates {
        changes.push(match &dependency.version {
            Some(requirement) => format!("Cargo.toml {sign} {} {requirement}", dependency.name),
            None => format!("Cargo.toml {sign} {}", dependency.name),
        });
    }
    changes.push(format!("jackdaw.toml [modules] {id} = {on}"));
    changes
}

/// The extension ids on in the editor's global list.
pub fn globally_enabled(world: &World) -> Vec<String> {
    crate::extension_resolution::resolve_enabled_list(world)
}

/// Build the page's listing from `view`'s scope and pending changes.
pub fn listing(world: &World, view: &ExtensionsView) -> Listing {
    let catalog = ModuleCatalog::builtin();
    let pending = |item: &ExtensionItem| view.pending.get(item).copied();
    let mut out = Listing::default();
    let project_modules = view.project.as_deref().map(project_modules);
    for module in catalog.modules() {
        let item = ExtensionItem::Module(module.id.clone());
        let included = crate::module_picker::editor_has_module(world, &module.id);
        let incompatible = module
            .unavailable
            .as_ref()
            .map(|reason| format!("not compatible: {reason}"));
        let (current, toggleable, detail) = match &project_modules {
            Some(modules) => {
                let used = modules.contains(&module.id);
                let detail = if let Some(incompatible) = &incompatible {
                    incompatible.clone()
                } else if module.requires.is_empty() {
                    String::new()
                } else {
                    format!("needs {}", catalog.names(&module.requires))
                };
                (used, used || incompatible.is_none(), detail)
            }
            None if incompatible.is_some() => (included, false, incompatible.unwrap_or_default()),
            None => (
                included,
                false,
                if included {
                    "built into this editor".to_string()
                } else {
                    "not in this editor".to_string()
                },
            ),
        };
        let on = pending(&item).unwrap_or(current);
        let attention = (project_modules.is_some() && on && !included)
            .then(|| "this editor does not include its tools".to_string());
        out.modules.push(Entry {
            item: item.clone(),
            name: module.name.clone(),
            description: module.description.clone(),
            detail,
            on,
            pending: pending(&item).is_some_and(|wanted| wanted != current),
            attention,
            toggleable,
            changes: module_changes(&module.id, on),
            installed: true,
        });
    }

    let globals = globally_enabled(world);
    let overrides = view
        .project
        .as_deref()
        .and_then(|root| ProjectManifest::read(root).extensions)
        .unwrap_or_default();
    if let Some(extensions) = world.get_resource::<ExtensionCatalog>() {
        let mut entries: Vec<(String, String, String)> = extensions
            .iter_with_content()
            .filter(|(id, _, _, kind)| {
                *kind == jackdaw_api::prelude::ExtensionKind::Regular
                    && !crate::extension_resolution::is_required(id)
                    && catalog.get(id).is_none()
            })
            .map(|(id, label, description, _)| (id, label, description))
            .collect();
        entries.sort_by(|a, b| a.1.cmp(&b.1));
        for (id, label, description) in entries {
            let item = ExtensionItem::Extension(id.clone());
            let global = globals.contains(&id);
            let current = overrides.get(&id).copied().unwrap_or(global);
            let on = pending(&item).unwrap_or(current);
            let mut detail = "installed".to_string();
            if view.project.is_some() {
                match overrides.get(&id) {
                    Some(true) => detail.push_str(" - on for this project"),
                    Some(false) => detail.push_str(" - off for this project"),
                    None => detail.push_str(if global {
                        " - on by default"
                    } else {
                        " - off by default"
                    }),
                }
            }
            out.extensions.push(Entry {
                item: item.clone(),
                name: if label.is_empty() { id.clone() } else { label },
                description,
                detail,
                on,
                pending: pending(&item).is_some_and(|wanted| wanted != current),
                attention: None,
                toggleable: true,
                changes: vec![if view.project.is_some() {
                    format!("jackdaw.toml [extensions] {id}")
                } else {
                    "the editor's extension list".to_string()
                }],
                installed: true,
            });
        }
    }

    let known = |id: &str| {
        world
            .get_resource::<ExtensionCatalog>()
            .is_some_and(|extensions| extensions.contains(id))
    };
    for (id, wanted) in &overrides {
        if !*wanted || known(id) || catalog.get(id).is_some() {
            continue;
        }
        out.extensions.push(Entry {
            item: ExtensionItem::Extension(id.clone()),
            name: id.clone(),
            description: "This project turns it on in its jackdaw.toml, but it is not installed \
                          in this editor, so it stays off. Install its .jdext to use it."
                .into(),
            detail: String::new(),
            on: false,
            pending: false,
            attention: Some("not installed in this editor".to_string()),
            toggleable: false,
            changes: vec![format!("jackdaw.toml [extensions] {id} = true")],
            installed: false,
        });
    }

    if let Some(root) = view.project.as_deref() {
        let manifest = ProjectManifest::read(root);
        let package_dir = manifest
            .package
            .as_deref()
            .map(|name| root.join(name))
            .filter(|dir| dir.join("Cargo.toml").is_file())
            .unwrap_or_else(|| root.to_path_buf());
        let game = GameDependencies::read(&package_dir, root);
        out.plugins = game
            .crates
            .into_iter()
            .map(|name| {
                let module = catalog
                    .providing_crate(&name)
                    .map(|module| module.name.clone());
                (name, module)
            })
            .collect();
    }
    out
}

fn keeps(view: &ExtensionsView, entry: &Entry) -> bool {
    let query = view.query.trim().to_lowercase();
    let matches = query.is_empty()
        || entry.name.to_lowercase().contains(&query)
        || entry.item.id().to_lowercase().contains(&query);
    let status = match view.filter {
        ExtensionFilter::All => true,
        ExtensionFilter::On => entry.on,
        ExtensionFilter::Off => !entry.on,
        ExtensionFilter::NeedsAttention => entry.attention.is_some(),
    };
    matches && status
}

/// Refill the page when its scope, filter, selection or pending changes do,
/// or when the launcher's state does.
pub fn rebuild(world: &mut World) {
    let stale = world.is_resource_changed::<ExtensionsView>()
        || world.is_resource_changed::<Launcher>()
        || world.is_resource_changed::<LauncherPage>();
    let mut regions = world.query_filtered::<Entity, With<ExtensionsRegion>>();
    let regions: Vec<Entity> = regions.iter(world).collect();
    let mut scopes = world.query_filtered::<Entity, With<ExtensionsScopeRegion>>();
    let scopes: Vec<Entity> = scopes.iter(world).collect();
    let fresh = |world: &World, entity: Entity| {
        world
            .entity(entity)
            .get_ref::<Node>()
            .is_some_and(|node| node.is_added())
    };
    let view = world.resource::<ExtensionsView>().clone();
    for region in regions {
        if !(stale || fresh(world, region) || world.entity(region).get::<Children>().is_none()) {
            continue;
        }
        let listing = listing(world, &view);
        world.entity_mut(region).despawn_children();
        if let Ok(mut spawned) = world.spawn_scene(content(&view, &listing)) {
            spawned.insert(ChildOf(region));
        }
    }
    for region in scopes {
        if !(stale || world.entity(region).get::<Children>().is_none()) {
            continue;
        }
        let launcher = world.resource::<Launcher>().clone();
        world.entity_mut(region).despawn_children();
        if let Ok(mut spawned) = world.spawn_scene(scope_picker(&view, &launcher)) {
            spawned.insert(ChildOf(region));
        }
    }
}

fn project_name(launcher: &Launcher, root: &Path) -> String {
    launcher.project(root).map_or_else(
        || {
            root.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        },
        |entry| entry.name.clone(),
    )
}

fn scope_picker(view: &ExtensionsView, launcher: &Launcher) -> impl Scene {
    let caption = match &view.project {
        Some(root) => project_name(launcher, root),
        None => "Global defaults".to_string(),
    };
    let mut items: ui::Scenes = vec![ui::boxed(scope_item(
        "Global defaults".into(),
        String::new(),
    ))];
    for project in launcher
        .projects
        .iter()
        .filter(|project| !project.folder_missing)
    {
        items.push(ui::boxed(scope_item(
            project.name.clone(),
            project.path.to_string_lossy().into_owned(),
        )));
    }
    let editor = format!(
        "jackdaw {}",
        super::super::model::version_label(&launcher.running.version)
    ) + &format!(" - Bevy {}", launcher.running.bevy);
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(8),
        }
        Children [
            @FeathersMenu
            Children [
                @FeathersMenuButton {
                    @caption: bsn! { Text(caption) ThemedText Node { margin: UiRect::right(px(6)) } },
                }
                --
                @FeathersMenuPopup
                ~{TabGroup::modal()}
                        Node { min_width: px(220) }
                        Children [ {items} ]
            ]
            --
            @{ui::dim(editor)}
        ]
    }
}

fn scope_item(caption: String, path: String) -> impl Scene {
    let call = ButtonOperatorCall::new("launcher.extensions").with_param("path", path);
    bsn! {
        @FeathersMenuItem {
            @caption: bsn! { Text(caption) ThemedText Node { margin: UiRect::right(px(6)) } },
        }
        ~{call}
    }
}

fn content(view: &ExtensionsView, listing: &Listing) -> impl Scene {
    let filters: Vec<_> = ExtensionFilter::ALL
        .into_iter()
        .map(|filter| {
            ui::filter_button(
                filter.label(),
                filter == view.filter,
                ButtonOperatorCall::new("launcher.extensions_filter")
                    .with_param("filter", filter.id()),
            )
        })
        .collect();
    let explanation = if view.project.is_some() {
        "This project's choices; extensions it does not set follow the global defaults"
    } else {
        "Global defaults apply to every project; a project can override them"
    };
    let modules: Vec<_> = listing
        .modules
        .iter()
        .filter(|entry| keeps(view, entry))
        .map(|entry| entry_row(entry, view.selected.as_ref() == Some(&entry.item)))
        .collect();
    let mut extensions: ui::Scenes = listing
        .extensions
        .iter()
        .filter(|entry| keeps(view, entry))
        .map(|entry| {
            ui::boxed(entry_row(
                entry,
                view.selected.as_ref() == Some(&entry.item),
            ))
        })
        .collect();
    if listing.extensions.is_empty() {
        extensions.push(ui::boxed(bsn! {
            Node { padding: UiRect::all(px(6)) }
            Children [
                @{ui::note(
                    "No editor extensions are installed. The editor's own windows and tools are \
                     part of it and are not listed here.",
                    tokens::TEXT_SECONDARY,
                )}
            ]
        }));
    }
    let modules_note = if view.project.is_some() {
        "official, shipped with each editor"
    } else {
        "chosen per project; shown for this editor"
    };
    let mut sections: ui::Scenes = vec![
        ui::boxed(ui::section(
            vec![
                ui::boxed(ui::primary("Modules")),
                ui::boxed(ui::dim(modules_note)),
            ],
            modules,
        )),
        ui::boxed(ui::section(
            vec![
                ui::boxed(ui::primary("Extensions")),
                ui::boxed(ui::dim("optional editor tools")),
                ui::boxed(ui::spacer()),
                ui::boxed(ui::button(
                    "Install .jdext...",
                    ButtonOperatorCall::new("launcher.install_extension"),
                    ButtonVariant::Normal,
                )),
            ],
            extensions,
        )),
    ];
    if view.project.is_some() {
        let chips: Vec<_> = listing
            .plugins
            .iter()
            .map(|(name, module)| {
                ui::chip(
                    match module {
                        Some(module) => format!("{name} - {module}"),
                        None => name.clone(),
                    },
                    Tone::Neutral,
                )
            })
            .collect();
        sections.push(ui::boxed(ui::section(
            vec![
                ui::boxed(ui::primary("Game plugins")),
                ui::boxed(ui::dim("crates in Cargo.toml")),
            ],
            vec![ui::boxed(bsn! {
                Node {
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: px(6),
                    row_gap: px(6),
                    padding: UiRect::all(px(4)),
                }
                Children [ {chips} ]
            })],
        )));
    }
    let details = details_pane(view, listing);
    let pending = pending_bar(view, listing);
    bsn! {
        Node {
            flex_grow: 1.0,
            min_height: px(0),
            flex_direction: FlexDirection::Column,
            row_gap: px(12),
        }
        Children [
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(6),
            }
            Children [
                @{ui::dim("Status")}
                --
                {filters}
                --
                @{ui::spacer()}
                --
                @{ui::dim(explanation)}
            ]
            --
            Node {
                flex_grow: 1.0,
                min_height: px(0),
                flex_direction: FlexDirection::Row,
                column_gap: px(12),
            }
            Children [
                Node {
                    flex_grow: 1.0,
                    min_width: px(0),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(10),
                    overflow: Overflow::scroll_y(),
                }
                ScrollPosition::default()
                bevy::ui_widgets::ScrollArea
                Hovered::default()
                Children [ {sections} ]
                --
                @{details}
            ]
            --
            {pending}
        ]
    }
}

fn entry_row(entry: &Entry, selected: bool) -> impl Scene {
    let select = ButtonOperatorCall::new("launcher.extensions_select")
        .with_param("kind", entry.item.kind())
        .with_param("id", entry.item.id().to_string());
    let toggle = ToggleOperatorCall(
        ButtonOperatorCall::new("launcher.extensions_toggle")
            .with_param("kind", entry.item.kind())
            .with_param("id", entry.item.id().to_string()),
    );
    let background = if selected {
        tokens::SELECTED_BG
    } else {
        Color::NONE
    };
    let action: ui::Scenes = if entry.installed {
        Vec::new()
    } else {
        vec![ui::boxed(ui::button(
            "Install .jdext...",
            ButtonOperatorCall::new("launcher.install_extension"),
            ButtonVariant::Normal,
        ))]
    };
    let (detail, color) = match (&entry.attention, entry.pending) {
        (Some(attention), _) => (attention.clone(), tokens::CHIP_WARNING_TEXT),
        (None, true) => (
            format!("pending - turns {}", if entry.on { "on" } else { "off" }),
            tokens::CHIP_CAUTION_TEXT,
        ),
        (None, false) => (entry.detail.clone(), tokens::DOC_TAB_INACTIVE_LABEL),
    };
    let name = entry.name.clone();
    let on = entry.on;
    let disabled = !entry.toggleable;
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(8),
            min_height: px(30),
            padding: UiRect::horizontal(px(6)),
            border_radius: BorderRadius::all(px(4)),
        }
        BackgroundColor(background)
        Button
        Hovered::default()
        ~{select}
        Children [
            @FeathersToggleSwitch
            ~{toggle}
            @{ui::toggle_state(on, disabled)}
            --
            Node { width: px(160), flex_shrink: 0.0 } Children [ @{ui::primary(name)} ]
            --
            Node { flex_grow: 1.0, min_width: px(0) }
            Children [ @{ui::text(detail, tokens::TEXT_SIZE_SM, color)} ]
            --
            {action}
        ]
    }
}

fn details_pane(view: &ExtensionsView, listing: &Listing) -> impl Scene {
    let selected = view.selected.as_ref().and_then(|item| {
        listing
            .modules
            .iter()
            .chain(&listing.extensions)
            .find(|entry| &entry.item == item)
    });
    let content: ui::Scenes = match selected {
        None => vec![ui::boxed(ui::note(
            "Select a module or extension to see what it does and what turning it on changes.",
            tokens::TEXT_SECONDARY,
        ))],
        Some(entry) => {
            let kind = match entry.item {
                ExtensionItem::Module(_) => "Module - jackdaw",
                ExtensionItem::Extension(_) => "Extension",
            };
            let mut lines: ui::Scenes = vec![
                ui::boxed(ui::primary(entry.name.clone())),
                ui::boxed(ui::dim(format!("{kind} - {}", entry.item.id()))),
                ui::boxed(ui::note(entry.description.clone(), tokens::TEXT_SECONDARY)),
                ui::boxed(ui::dim("Changes")),
            ];
            for change in &entry.changes {
                lines.push(ui::boxed(ui::note(change.clone(), tokens::TEXT_PRIMARY)));
            }
            if let Some(attention) = &entry.attention {
                lines.push(ui::boxed(ui::note(
                    attention.clone(),
                    tokens::CHIP_WARNING_TEXT,
                )));
            }
            lines
        }
    };
    bsn! {
        Node {
            width: px(300),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            row_gap: px(8),
            padding: UiRect::all(px(12)),
            border_radius: BorderRadius::all(px(4)),
        }
        BackgroundColor(tokens::ELEVATED_BG)
        Children [ {content} ]
    }
}

/// The pending changes as one line.
pub fn pending_summary(listing: &Listing) -> Option<String> {
    let mut parts = Vec::new();
    for entry in listing.modules.iter().chain(&listing.extensions) {
        if entry.pending {
            let verb = if entry.on { "turn on" } else { "turn off" };
            parts.push(format!("{verb} {}", entry.name));
        }
    }
    (!parts.is_empty()).then(|| format!("Pending: {}", parts.join(", ")))
}

fn pending_bar(view: &ExtensionsView, listing: &Listing) -> Box<dyn SceneList> {
    let summary = pending_summary(listing);
    let Some(summary) = summary else {
        return match &view.status {
            Some(status) => ui::boxed(ui::note(status.clone(), tokens::TEXT_SECONDARY)),
            None => Box::new(bsn_list! {}),
        };
    };
    let extension_changes = listing.extensions.iter().any(|entry| entry.pending);
    let (caption, restart) = if extension_changes {
        ("Apply and restart", true)
    } else {
        ("Apply", false)
    };
    let apply = ButtonOperatorCall::new("launcher.extensions_apply").with_param("restart", restart);
    ui::boxed(bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(8),
            padding: UiRect::axes(px(10), px(8)),
            border_radius: BorderRadius::all(px(4)),
        }
        BackgroundColor(tokens::WINDOW_BG)
        Children [
            Node { flex_grow: 1.0, min_width: px(0) } Children [ @{ui::note(summary, tokens::TEXT_PRIMARY)} ]
            --
            @{ui::button("Revert", ButtonOperatorCall::new("launcher.extensions_revert"), ButtonVariant::Plain)}
            --
            @{ui::button(caption, apply, ButtonVariant::Primary)}
        ]
    })
}

/// Record a toggle as pending, following module requirements, and drop it
/// when it puts the entry back as it was.
pub fn stage_toggle(world: &mut World, item: ExtensionItem, enabled: bool) {
    let view = world.resource::<ExtensionsView>().clone();
    let base = listing(
        world,
        &ExtensionsView {
            pending: BTreeMap::new(),
            ..view.clone()
        },
    );
    let current = |item: &ExtensionItem| {
        base.modules
            .iter()
            .chain(&base.extensions)
            .find(|entry| &entry.item == item)
            .map(|entry| entry.on)
    };
    let Some(entry) = base
        .modules
        .iter()
        .chain(&base.extensions)
        .find(|entry| entry.item == item)
    else {
        return;
    };
    if !entry.toggleable {
        return;
    }
    let mut pending = view.pending.clone();
    match &item {
        ExtensionItem::Module(id) => {
            let catalog = ModuleCatalog::builtin();
            let mut modules: ModuleSet = base
                .modules
                .iter()
                .map(|entry| {
                    let on = pending.get(&entry.item).copied().unwrap_or(entry.on);
                    (entry.item.id().to_string(), on)
                })
                .filter(|(_, on)| *on)
                .map(|(id, _)| id)
                .collect();
            if enabled {
                catalog.enable(&mut modules, id);
            } else {
                catalog.disable(&mut modules, id);
            }
            for module in catalog.modules() {
                let item = ExtensionItem::Module(module.id.clone());
                let wanted = modules.contains(&module.id);
                if current(&item) == Some(wanted) {
                    pending.remove(&item);
                } else {
                    pending.insert(item, wanted);
                }
            }
        }
        ExtensionItem::Extension(_) => {
            if current(&item) == Some(enabled) {
                pending.remove(&item);
            } else {
                pending.insert(item.clone(), enabled);
            }
        }
    }
    let mut view = world.resource_mut::<ExtensionsView>();
    if view.pending != pending {
        view.pending = pending;
    }
    if view.selected.as_ref() != Some(&item) {
        view.selected = Some(item);
    }
}

/// Write the pending changes: a project's into its `jackdaw.toml` and
/// Cargo.toml, the global defaults into the editor's extension list.
pub fn apply(world: &mut World, restart: bool) {
    let view = world.resource::<ExtensionsView>().clone();
    if view.pending.is_empty() {
        return;
    }
    let result = match &view.project {
        Some(root) => apply_to_project(world, root, &view),
        None => {
            for (item, enabled) in &view.pending {
                if let ExtensionItem::Extension(id) = item {
                    jackdaw_api_internal::extensions_config::set_extension_enabled(
                        id.clone(),
                        *enabled,
                    );
                }
            }
            Ok("Saved the global defaults.".to_string())
        }
    };
    let mut current = world.resource_mut::<ExtensionsView>();
    match result {
        Ok(message) => {
            current.pending.clear();
            current.status = Some(message);
        }
        Err(error) => current.status = Some(format!("Not applied: {error}")),
    }
    let data = super::super::LauncherData::read();
    if world.resource::<Launcher>().0 != data {
        world.resource_mut::<Launcher>().0 = data;
    }
    if restart && view.project.is_none() && crate::restart::can_restart().is_some() {
        crate::restart::restart_jackdaw();
    }
}

fn apply_to_project(world: &World, root: &Path, view: &ExtensionsView) -> Result<String, String> {
    let base = listing(
        world,
        &ExtensionsView {
            pending: BTreeMap::new(),
            ..view.clone()
        },
    );
    let module_changes = view
        .pending
        .keys()
        .any(|item| matches!(item, ExtensionItem::Module(_)));
    if module_changes {
        let modules: ModuleSet = base
            .modules
            .iter()
            .filter(|entry| view.pending.get(&entry.item).copied().unwrap_or(entry.on))
            .map(|entry| entry.item.id().to_string())
            .collect();
        let plan = crate::scaffold::plan_module_change(root, &modules)
            .map_err(|error| error.to_string())?;
        if !plan.is_empty() {
            crate::scaffold::apply_import_plan(&plan).map_err(|error| error.to_string())?;
        }
    }
    let extension_changes: Vec<(String, bool)> = view
        .pending
        .iter()
        .filter_map(|(item, enabled)| match item {
            ExtensionItem::Extension(id) => Some((id.clone(), *enabled)),
            ExtensionItem::Module(_) => None,
        })
        .collect();
    if !extension_changes.is_empty() {
        let path = jackdaw_project_build::project_manifest::path(root);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let mut overrides = ProjectManifest::read(root).extensions.unwrap_or_default();
        for (id, enabled) in extension_changes {
            overrides.insert(id, enabled);
        }
        let written = jackdaw_project_build::project_manifest::with_extensions(&text, &overrides)?;
        std::fs::write(&path, written).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(format!(
        "Saved to {}. The editor applies it when the project opens.",
        super::projects::display_path(root)
    ))
}

/// Turn on or off the extensions a project's `[extensions]` table names,
/// for this session, without changing the global list.
pub fn apply_project_overrides(world: &mut World, root: &Path) {
    let Some(overrides) = ProjectManifest::read(root).extensions else {
        return;
    };
    let catalog_has = |world: &World, id: &str| {
        world
            .get_resource::<ExtensionCatalog>()
            .is_some_and(|catalog| catalog.contains(id))
    };
    for (id, enabled) in overrides {
        if crate::extension_resolution::is_required(&id) || !catalog_has(world, &id) {
            continue;
        }
        if enabled {
            jackdaw_api_internal::lifecycle::enable_extension(world, &id);
        } else {
            jackdaw_api_internal::lifecycle::disable_extension(world, &id);
        }
    }
}
