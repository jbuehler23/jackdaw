//! The launcher's dialogs: Install editor, Open with a different editor,
//! Uninstall, Doctor, Settings and Downloads. One at a time, over the whole
//! window, in any app state.

use bevy::feathers::controls::{
    ButtonVariant, FeathersMenu, FeathersMenuButton, FeathersMenuItem, FeathersMenuPopup,
};
use bevy::feathers::theme::ThemedText;
use bevy::input_focus::tab_navigation::TabGroup;
use bevy::prelude::*;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::tokens;
use jackdaw_project_build::modules::ModuleCatalog;
use jackdaw_releases::{Channel, version};

use super::super::jd::{IndexStatus, JdJobs, JobStatus};
use super::super::model::{ProjectEditor, editor_chip, format_bytes, version_label};
use super::super::project_actions::{OpenChoice, backup_destination, copy_destination};
use super::super::ui::{self, Tone};
use super::super::{InstallForm, Launcher, LauncherDialog, OpenWithForm};
use super::projects::display_path;
use crate::preflight::CheckStatus;
use crate::project_select::PreflightState;

/// The open dialog's root, over the whole window.
#[derive(Component, Default, Clone)]
pub struct LauncherDialogRoot;

/// Show the dialog `LauncherDialog` names, refilled when what it shows
/// changes.
pub fn sync_dialog(world: &mut World) {
    let dialog_changed = world.is_resource_changed::<LauncherDialog>();
    let dialog = world.resource::<LauncherDialog>().clone();
    let follows = match &dialog {
        LauncherDialog::None => false,
        LauncherDialog::Downloads => world.is_resource_changed::<JdJobs>(),
        LauncherDialog::Doctor => world.is_resource_changed::<PreflightState>(),
        LauncherDialog::Settings => world.is_resource_changed::<IndexStatus>(),
        _ => world.is_resource_changed::<Launcher>(),
    };
    if !dialog_changed && !follows {
        return;
    }
    let mut roots = world.query_filtered::<Entity, With<LauncherDialogRoot>>();
    let roots: Vec<Entity> = roots.iter(world).collect();
    for root in roots {
        world.entity_mut(root).despawn();
    }
    let body: Box<dyn SceneList> = match &dialog {
        LauncherDialog::None => return,
        LauncherDialog::InstallEditor(form) => {
            ui::boxed(install_editor(form, world.resource::<Launcher>()))
        }
        LauncherDialog::OpenWith(form) => ui::boxed(open_with(form, world.resource::<Launcher>())),
        LauncherDialog::Uninstall { version, projects } => ui::boxed(uninstall(version, projects)),
        LauncherDialog::Doctor => ui::boxed(doctor(world.resource::<PreflightState>())),
        LauncherDialog::Settings => ui::boxed(settings(
            world.resource::<Launcher>(),
            world.resource::<IndexStatus>(),
            &world.resource::<super::super::JdProgram>().0,
        )),
        LauncherDialog::Downloads => ui::boxed(downloads(world.resource::<JdJobs>())),
    };
    if let Err(error) = world.spawn_scene(backdrop(body)) {
        error!("Could not show the launcher dialog: {error:?}");
    }
}

fn backdrop(body: Box<dyn SceneList>) -> impl Scene {
    bsn! {
        LauncherDialogRoot
        Node {
            position_type: PositionType::Absolute,
            left: px(0),
            top: px(0),
            width: percent(100),
            height: percent(100),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
        }
        BackgroundColor({tokens::DIALOG_BACKDROP.with_alpha(0.55)})
        GlobalZIndex(60)
        Children [ {body} ]
    }
}

fn frame(title: String, width: f32, content: ui::Scenes, footer: ui::Scenes) -> impl Scene {
    bsn! {
        Node {
            width: px(width),
            max_width: percent(94),
            max_height: percent(94),
            flex_direction: FlexDirection::Column,
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(4)),
        }
        BackgroundColor(tokens::PANEL_BG)
        BorderColor::all(tokens::COMPONENT_CARD_BORDER)
        BoxShadow::new(Color::BLACK.with_alpha(0.9), px(0), px(0), px(1), px(4))
        Children [
            Node {
                min_height: px(36),
                padding: UiRect::horizontal(px(12)),
                align_items: AlignItems::Center,
                border_radius: BorderRadius::top(px(4)),
            }
            BackgroundColor(tokens::WINDOW_BG)
            Children [ @{ui::text(title, tokens::TEXT_SIZE_LG, tokens::TEXT_PRIMARY)} ]
            --
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(12),
                padding: UiRect::all(px(12)),
                min_height: px(0),
                flex_shrink: 1.0,
                overflow: Overflow::scroll_y(),
            }
            ScrollPosition::default()
            bevy::ui_widgets::ScrollArea
            bevy::picking::hover::Hovered::default()
            Children [ {content} ]
            --
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::FlexEnd,
                column_gap: px(8),
                padding: UiRect::axes(px(12), px(10)),
                border: UiRect::top(px(1)),
            }
            BorderColor::all(tokens::ELEVATED_BG)
            Children [ {footer} ]
        ]
    }
}

fn close(caption: &'static str) -> Box<dyn SceneList> {
    ui::boxed(ui::button(
        caption,
        ButtonOperatorCall::new("launcher.close_dialog"),
        ButtonVariant::Normal,
    ))
}

/// A card with a radio, a title and a description, for one option.
fn option_card(
    selected: bool,
    enabled: bool,
    call: ButtonOperatorCall,
    title: ui::Scenes,
    lines: ui::Scenes,
) -> impl Scene {
    let outline = if selected {
        tokens::SELECTED_BORDER
    } else {
        Color::NONE
    };
    let radio: Box<dyn SceneList> = if enabled {
        ui::boxed(ui::radio(selected, call.clone()))
    } else {
        ui::boxed(bsn! { Node { width: px(18) } })
    };
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            column_gap: px(8),
            padding: UiRect::all(px(10)),
            border: UiRect::all(px(2)),
            border_radius: BorderRadius::all(px(4)),
        }
        BackgroundColor(tokens::ELEVATED_BG)
        BorderColor::all(outline)
        Children [
            {radio}
            --
            Node {
                flex_grow: 1.0,
                min_width: px(0),
                flex_direction: FlexDirection::Column,
                row_gap: px(4),
            }
            Children [
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(8),
                    flex_wrap: FlexWrap::Wrap,
                }
                Children [ {title} ]
                --
                {lines}
            ]
        ]
    }
}

fn releases_for(launcher: &Launcher, form: &InstallForm) -> Vec<(String, String)> {
    let Some(index) = launcher.index.index() else {
        return Vec::new();
    };
    let mut releases: Vec<&jackdaw_releases::Release> = index
        .releases
        .iter()
        .filter(|release| release.channel == form.channel && !release.yanked)
        .collect();
    releases.sort_by(|a, b| version::compare(&b.version, &a.version));
    if !form.show_older {
        let mut seen = Vec::new();
        releases.retain(|release| {
            let key = if release.channel == Channel::BleedingEdge {
                release.version.clone()
            } else {
                release.bevy.clone()
            };
            if seen.contains(&key) {
                false
            } else {
                seen.push(key);
                true
            }
        });
        if form.channel == Channel::BleedingEdge {
            releases.truncate(5);
        }
    }
    releases
        .into_iter()
        .map(|release| {
            let mut label = editor_chip(&release.version, &release.bevy);
            if launcher.installs.get(&release.version).is_some() {
                label.push_str(" - installed");
            } else if release.known_good {
                label.push_str(" - known good");
            }
            (release.version.clone(), label)
        })
        .collect()
}

fn version_picker(launcher: &Launcher, form: &InstallForm) -> impl Scene {
    let caption = launcher
        .release(&form.version)
        .map(|release| {
            let mut label = editor_chip(&release.version, &release.bevy);
            let latest = match release.channel {
                Channel::Stable => launcher
                    .release(&release.bevy)
                    .is_some_and(|newest| newest.version == release.version),
                Channel::BleedingEdge => launcher
                    .release("bleeding-edge")
                    .is_some_and(|newest| newest.version == release.version),
            };
            if latest {
                label.push_str(" - Latest");
            }
            label
        })
        .unwrap_or_else(|| version_label(&form.version));
    let items: Vec<_> = releases_for(launcher, form)
        .into_iter()
        .map(|(version, label)| {
            let call =
                ButtonOperatorCall::new("launcher.install_version").with_param("version", version);
            bsn! {
                @FeathersMenuItem {
                    @caption: bsn! { Text(label) ThemedText },
                }
                ~{call}
            }
        })
        .collect();
    bsn! {
        @FeathersMenu
        Node { flex_grow: 1.0 }
        Children [
            @FeathersMenuButton {
                @caption: bsn! { Text(caption) ThemedText Node { margin: UiRect::right(px(6)) } },
            }
            Node { flex_grow: 1.0 }
            --
            @FeathersMenuPopup
            ~{TabGroup::modal()}
            Node { min_width: px(320) }
            Children [ {items} ]
        ]
    }
}

fn field_row(label: &'static str, value: impl Scene) -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(10),
        }
        Children [
            Node { width: px(90), flex_shrink: 0.0 } Children [ @{ui::dim(label)} ]
            --
            Node { flex_grow: 1.0, min_width: px(0), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8) } Children [ @{value} ]
        ]
    }
}

fn install_editor(form: &InstallForm, launcher: &Launcher) -> impl Scene {
    let release = launcher.release(&form.version).cloned();
    let existing = form.existing.is_some();
    let title = match &form.existing {
        Some(version) => format!("Modules for jackdaw {}", version_label(version)),
        None => "Install editor".to_string(),
    };
    let mut content: ui::Scenes = Vec::new();
    if !existing {
        let channel_option = |channel: Channel, title: &'static str, line: &'static str| {
            let call = ButtonOperatorCall::new("launcher.install_channel")
                .with_param("channel", channel.as_str());
            let mut heading: ui::Scenes = vec![ui::boxed(ui::primary(title))];
            if channel == Channel::BleedingEdge {
                heading.push(ui::boxed(ui::chip("Bevy main", Tone::Caution)));
            }
            ui::boxed(option_card(
                form.channel == channel,
                true,
                call,
                heading,
                vec![ui::boxed(ui::note(line, tokens::TEXT_SECONDARY))],
            ))
        };
        content.push(ui::boxed(ui::dim("Channel")));
        content.push(channel_option(
            Channel::Stable,
            "Stable",
            "Released builds, one line per Bevy version. Use this for projects.",
        ));
        content.push(channel_option(
            Channel::BleedingEdge,
            "Bleeding Edge",
            "Nightly builds against Bevy main. Kept 30 days, plus a weekly known-good build.",
        ));
        if launcher.index.index().is_none() {
            content.push(ui::boxed(ui::note(
                "The release index has not been fetched, so no releases can be listed. Check \
                 for updates on the Editors page once online.",
                tokens::CHIP_WARNING_TEXT,
            )));
        }
        let older = ButtonOperatorCall::new("launcher.install_show_older")
            .with_param("enabled", !form.show_older);
        let older_caption = if form.show_older {
            "Newest only"
        } else {
            "Show older"
        };
        content.push(ui::boxed(bsn! {
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(10),
            }
            Children [
                Node { width: px(90), flex_shrink: 0.0 } Children [ @{ui::dim("Version")} ]
                --
                @{version_picker(launcher, form)}
                --
                @{ui::button(older_caption, older, ButtonVariant::Plain)}
            ]
        }));
    }
    content.push(ui::boxed(field_row(
        "Platform",
        ui::secondary(format!("{} (detected)", launcher.host_target)),
    )));
    let location = launcher
        .launcher_dir
        .as_ref()
        .map(|dir| {
            display_path(
                &dir.join(jackdaw_releases::installs::EDITORS_DIR)
                    .join(&form.version),
            )
        })
        .unwrap_or_default();
    content.push(ui::boxed(field_row("Location", ui::path_text(location))));

    let catalog = ModuleCatalog::builtin();
    let ships_modules = release
        .as_ref()
        .is_some_and(|release| release.modules_index.is_some());
    let installed: Vec<String> = form
        .existing
        .as_deref()
        .and_then(|version| launcher.installs.get(version))
        .map(|install| install.modules.clone())
        .unwrap_or_default();
    let mut rows: ui::Scenes = Vec::new();
    for module in catalog.modules() {
        let call =
            ButtonOperatorCall::new("launcher.install_module").with_param("id", module.id.clone());
        let locked = installed.contains(&module.id);
        let note = if locked {
            "installed".to_string()
        } else if ships_modules {
            module.description.clone()
        } else {
            format!("{} - built into this release", module.description)
        };
        rows.push(ui::boxed(bsn! {
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(8),
                min_height: px(26),
            }
            Children [
                Node { width: px(140), flex_shrink: 0.0 } Children [ @{ui::checkbox(module.name.clone(), form.modules.contains(&module.id), locked, call)} ]
                --
                Node { flex_grow: 1.0, min_width: px(0) } Children [ @{ui::note(note, tokens::DOC_TAB_INACTIVE_LABEL)} ]
            ]
        }));
    }
    content.push(ui::boxed(ui::section(
        vec![
            ui::boxed(ui::primary("Modules")),
            ui::boxed(ui::dim("editor tools; games add the matching crates")),
        ],
        rows,
    )));
    if !existing {
        content.push(ui::boxed(bsn! {
            Node {
                padding: UiRect::axes(px(10), px(8)),
                border_radius: BorderRadius::all(px(4)),
            }
            BackgroundColor(tokens::WINDOW_BG)
            Children [
                @{ui::note(
                    "Extensions you installed are built for one editor. After installing, rebuild \
                     them for this one from their projects.",
                    tokens::TEXT_SECONDARY,
                )}
            ]
        }));
    }

    let artifact = release
        .as_ref()
        .and_then(|release| release.artifact_for(launcher.host_target));
    let summary = match artifact {
        Some(artifact) => {
            let mut text = format!("Download {}", format_bytes(artifact.size));
            if let Some(unpacked) = artifact.installed_size {
                text.push_str(&format!(" - {} on disk", format_bytes(unpacked)));
            }
            text.push_str(" - signed by jackdaw");
            text
        }
        None if existing => "Fetches the chosen modules".to_string(),
        None => format!("No build for {}", launcher.host_target),
    };
    let notes = ButtonOperatorCall::new("launcher.release_notes")
        .with_param("version", form.version.clone());
    let confirm = if existing { "Apply" } else { "Install" };
    let footer: ui::Scenes = vec![
        ui::boxed(
            bsn! { Node { flex_grow: 1.0, min_width: px(0) } Children [ @{ui::dim(summary)} ] },
        ),
        ui::boxed(ui::button("Release notes", notes, ButtonVariant::Plain)),
        close("Cancel"),
        ui::boxed(ui::button(
            confirm,
            ButtonOperatorCall::new("launcher.install_confirm"),
            ButtonVariant::Primary,
        )),
    ];
    frame(title, 640.0, content, footer)
}

fn open_with(form: &OpenWithForm, launcher: &Launcher) -> impl Scene {
    let entry = launcher.project(&form.project);
    let name = entry.map_or_else(
        || {
            form.project
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        },
        |entry| entry.name.clone(),
    );
    let pinned = entry.and_then(|entry| entry.pinned.clone());
    let pinned_bevy = entry.and_then(|entry| entry.bevy.clone());
    let missing_query = entry.and_then(|entry| match &entry.editor {
        ProjectEditor::Missing { query } => Some(query.clone()),
        _ => None,
    });
    let pinned_label = pinned
        .as_deref()
        .map(|pinned| version::line_of(pinned).unwrap_or_else(|| version_label(pinned)))
        .unwrap_or_else(|| "its editor".into());
    let title = match (&missing_query, &pinned_bevy) {
        (Some(_), Some(bevy)) => format!("{name} needs jackdaw {pinned_label} (Bevy {bevy})"),
        _ => format!("Open {name}"),
    };
    let intro = match (&pinned, &pinned_bevy, &missing_query) {
        (Some(pinned), Some(bevy), Some(_)) => format!(
            "This project was set up with jackdaw {} for Bevy {bevy}. No editor for it is \
             installed.",
            version_label(pinned)
        ),
        (Some(pinned), _, None) => format!(
            "This project was set up with jackdaw {}. Choose how to open it.",
            version_label(pinned)
        ),
        _ => "Choose how to open this project.".to_string(),
    };

    let target = match &form.choice {
        OpenChoice::OpenCopy { version } | OpenChoice::Convert { version } => version.clone(),
        _ => launcher.newest_editor(),
    };
    let target_install = launcher.installs.get(&target);
    let target_bevy = target_install.map_or(launcher.running.bevy.clone(), |install| {
        install.bevy.clone()
    });
    let target_label = editor_chip(&target, &target_bevy);
    let target_line = if target_bevy == "main" {
        "main".to_string()
    } else {
        target_bevy.clone()
    };

    let mut options: ui::Scenes = Vec::new();
    if let Some(query) = &missing_query {
        let release = launcher.release(query);
        let size = launcher
            .download_size(query)
            .map(|bytes| format!("{}. ", format_bytes(bytes)))
            .unwrap_or_default();
        let mut heading: ui::Scenes = vec![ui::boxed(ui::primary(format!(
            "Install jackdaw {} and open",
            release.map_or_else(
                || version_label(query),
                |release| version_label(&release.version)
            )
        )))];
        let available = release.is_some();
        if available {
            heading.push(ui::boxed(ui::chip("Recommended", Tone::Success)));
        }
        let line = if available {
            format!("{size}Opens in that editor; nothing in the project changes.")
        } else {
            "The release index does not list it; check for updates, or pick another option."
                .to_string()
        };
        options.push(ui::boxed(option_card(
            form.choice == OpenChoice::InstallAndOpen,
            available,
            ButtonOperatorCall::new("launcher.open_with_choice").with_param("choice", "install"),
            heading,
            vec![ui::boxed(ui::note(line, tokens::TEXT_SECONDARY))],
        )));
    }
    let copy = copy_destination(&form.project, &target_line);
    let copy_name = copy
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut copy_lines: ui::Scenes = vec![ui::boxed(ui::note(
        format!(
            "Copies the project to {copy_name}, then opens the copy. Scenes migrate on load; \
             project code builds once its Cargo.toml moves to Bevy {target_bevy}."
        ),
        tokens::TEXT_SECONDARY,
    ))];
    let others = editor_choices(launcher, "copy", &target);
    if let Some(others) = others {
        copy_lines.push(others);
    }
    options.push(ui::boxed(option_card(
        matches!(form.choice, OpenChoice::OpenCopy { .. }),
        true,
        ButtonOperatorCall::new("launcher.open_with_choice")
            .with_param("choice", "copy")
            .with_param("version", target.clone()),
        vec![ui::boxed(ui::primary(format!(
            "Open a copy with jackdaw {target_label}"
        )))],
        copy_lines,
    )));
    let backup = backup_destination(&form.project);
    let backup_name = backup
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    options.push(ui::boxed(option_card(
        matches!(form.choice, OpenChoice::Convert { .. }),
        true,
        ButtonOperatorCall::new("launcher.open_with_choice")
            .with_param("choice", "convert")
            .with_param("version", target.clone()),
        vec![ui::boxed(ui::primary(format!(
            "Convert this project to {}",
            version_label(&target)
        )))],
        vec![ui::boxed(ui::note(
            format!("Backs up to {backup_name} first, then converts in place."),
            tokens::TEXT_SECONDARY,
        ))],
    )));
    options.push(ui::boxed(option_card(
        form.choice == OpenChoice::ScenesOnly,
        true,
        ButtonOperatorCall::new("launcher.open_with_choice").with_param("choice", "scenes"),
        vec![ui::boxed(ui::primary("Look at scenes only"))],
        vec![ui::boxed(ui::note(
            "Opens here without changing the project. Its code will not build in this editor.",
            tokens::TEXT_SECONDARY,
        ))],
    )));

    let confirm = match form.choice {
        OpenChoice::InstallAndOpen => "Install and open",
        OpenChoice::OpenCopy { .. } => "Copy and open",
        OpenChoice::Convert { .. } => "Back up and convert",
        OpenChoice::ScenesOnly => "Open",
    };
    let mut content: ui::Scenes = vec![ui::boxed(ui::note(intro, tokens::TEXT_SECONDARY))];
    content.extend(options);
    let footer: ui::Scenes = vec![
        close("Back"),
        ui::boxed(ui::button(
            confirm,
            ButtonOperatorCall::new("launcher.open_with_confirm"),
            ButtonVariant::Primary,
        )),
    ];
    frame(title, 660.0, content, footer)
}

/// A menu of the installed editors a copy or conversion can move to, when
/// there is more than one.
fn editor_choices(
    launcher: &Launcher,
    choice: &'static str,
    current: &str,
) -> Option<Box<dyn SceneList>> {
    let mut versions: Vec<(String, String)> =
        vec![(launcher.running.version.clone(), launcher.running.chip())];
    for install in launcher.other_installs() {
        versions.push((
            install.version.clone(),
            editor_chip(&install.version, &install.bevy),
        ));
    }
    if versions.len() < 2 {
        return None;
    }
    let caption = versions
        .iter()
        .find(|(version, _)| version == current)
        .map(|(_, label)| format!("With {label}"))
        .unwrap_or_else(|| "Choose an editor".into());
    let items: Vec<_> = versions
        .into_iter()
        .map(|(version, label)| {
            let call = ButtonOperatorCall::new("launcher.open_with_choice")
                .with_param("choice", choice)
                .with_param("version", version);
            bsn! {
                @FeathersMenuItem {
                    @caption: bsn! { Text(label) ThemedText },
                }
                ~{call}
            }
        })
        .collect();
    Some(ui::boxed(bsn! {
        @FeathersMenu
        Node { align_self: AlignSelf::FlexStart }
        Children [
            @FeathersMenuButton { @caption: bsn! { Text(caption) ThemedText Node { margin: UiRect::right(px(6)) } } }
            --
            @FeathersMenuPopup ~{TabGroup::modal()} Node { min_width: px(240) } Children [ {items} ]
        ]
    }))
}

fn uninstall(version: &str, projects: &[std::path::PathBuf]) -> impl Scene {
    let mut content: ui::Scenes = vec![ui::boxed(ui::note(
        "These projects open with this editor. After uninstalling it they ask for another one.",
        tokens::TEXT_SECONDARY,
    ))];
    for project in projects {
        content.push(ui::boxed(ui::path_text(display_path(project))));
    }
    let force = ButtonOperatorCall::new("launcher.uninstall_editor")
        .with_param("version", version.to_string())
        .with_param("force", true);
    let footer: ui::Scenes = vec![
        close("Cancel"),
        ui::boxed(ui::button(
            "Uninstall anyway",
            force,
            ButtonVariant::Primary,
        )),
    ];
    frame(
        format!("Uninstall jackdaw {}?", version_label(version)),
        520.0,
        content,
        footer,
    )
}

fn doctor(preflight: &PreflightState) -> impl Scene {
    let mut content: ui::Scenes = Vec::new();
    if preflight.results.is_empty() {
        content.push(ui::boxed(ui::secondary("Checking the environment...")));
    }
    for result in &preflight.results {
        let (tone, word) = match result.status {
            CheckStatus::Ok => (Tone::Success, "ok"),
            CheckStatus::Warn => (Tone::Caution, "warning"),
            CheckStatus::Fail => (Tone::Warning, "failed"),
        };
        let mut lines: ui::Scenes = vec![ui::boxed(ui::note(
            result.detail.clone(),
            tokens::TEXT_SECONDARY,
        ))];
        if let Some(fix) = &result.fix {
            lines.push(ui::boxed(ui::note(
                format!("Fix: {fix}"),
                tokens::TEXT_PRIMARY,
            )));
        }
        let label = result.label.clone();
        content.push(ui::boxed(bsn! {
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(4),
                padding: UiRect::axes(px(10), px(8)),
                border_radius: BorderRadius::all(px(4)),
            }
            BackgroundColor(tokens::ELEVATED_BG)
            Children [
                Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8) }
                Children [
                    @{ui::chip(word, tone)}
                    --
                    @{ui::primary(label)}
                ]
                --
                {lines}
            ]
        }));
    }
    let footer: ui::Scenes = vec![
        ui::boxed(ui::button(
            "Recheck",
            ButtonOperatorCall::new("launcher.recheck_environment"),
            ButtonVariant::Normal,
        )),
        close("Close"),
    ];
    frame("Doctor".into(), 600.0, content, footer)
}

fn settings(launcher: &Launcher, index: &IndexStatus, jd: &std::path::Path) -> impl Scene {
    let folder = launcher
        .launcher_dir
        .as_deref()
        .map(display_path)
        .unwrap_or_else(|| "none".into());
    let source = std::env::var(jackdaw_releases::INDEX_URL_VAR)
        .ok()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| jackdaw_releases::OFFICIAL_INDEX_URL.to_string());
    let status = match index {
        IndexStatus::Unknown => "not checked this session".to_string(),
        IndexStatus::Checking => "checking".to_string(),
        IndexStatus::Online { .. } => "fetched this session".to_string(),
        IndexStatus::Offline { reason, .. } => format!("offline: {reason}"),
    };
    let content: ui::Scenes = vec![
        ui::boxed(field_row("Editors", ui::path_text(folder))),
        ui::boxed(field_row("Index", ui::path_text(source))),
        ui::boxed(field_row(
            "Status",
            ui::note(status, tokens::TEXT_SECONDARY),
        )),
        ui::boxed(field_row("jd", ui::path_text(display_path(jd)))),
        ui::boxed(ui::note(
            "The launcher checks the index when it opens and only reports what is new; it \
             downloads nothing until asked.",
            tokens::TEXT_SECONDARY,
        )),
    ];
    let footer: ui::Scenes = vec![
        ui::boxed(ui::button(
            "Check for updates",
            ButtonOperatorCall::new("launcher.refresh_index"),
            ButtonVariant::Normal,
        )),
        close("Close"),
    ];
    frame("Settings".into(), 600.0, content, footer)
}

fn downloads(jobs: &JdJobs) -> impl Scene {
    let mut content: ui::Scenes = Vec::new();
    for job in jobs.listed().collect::<Vec<_>>().into_iter().rev() {
        let (tone, word) = match &job.status {
            JobStatus::Running => (Tone::Accent, "running"),
            JobStatus::Succeeded(_) => (Tone::Success, "done"),
            JobStatus::Failed(_) => (Tone::Warning, "failed"),
        };
        let fraction = job.fraction();
        let bar: ui::Scenes = match fraction {
            Some(fraction) if job.is_running() && job.stage.is_none() => {
                let width = (fraction * 100.0).clamp(0.0, 100.0);
                vec![ui::boxed(bsn! {
                    Node {
                        width: percent(100),
                        height: px(6),
                        border_radius: BorderRadius::all(px(3)),
                        overflow: Overflow::clip(),
                    }
                    BackgroundColor(tokens::WINDOW_BG)
                    Children [ Node { width: percent(width), height: percent(100) } BackgroundColor(tokens::ACCENT_BLUE) ]
                })]
            }
            _ => Vec::new(),
        };
        let cancel: ui::Scenes = if job.is_running() {
            vec![ui::boxed(ui::button(
                "Cancel",
                ButtonOperatorCall::new("launcher.cancel_job").with_param("job", job.id as i64),
                ButtonVariant::Plain,
            ))]
        } else {
            Vec::new()
        };
        let label = job.kind.label();
        let summary = job.summary();
        content.push(ui::boxed(bsn! {
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                padding: UiRect::axes(px(10), px(8)),
                border_radius: BorderRadius::all(px(4)),
            }
            BackgroundColor(tokens::ELEVATED_BG)
            Children [
                Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8) }
                Children [
                    @{ui::chip(word, tone)}
                    --
                    @{ui::primary(label)}
                    --
                    @{ui::spacer()}
                    --
                    {cancel}
                ]
                --
                {bar}
                --
                @{ui::note(summary, tokens::TEXT_SECONDARY)}
            ]
        }));
    }
    if content.is_empty() {
        content.push(ui::boxed(ui::secondary("Nothing downloaded this session.")));
    }
    frame("Downloads".into(), 560.0, content, vec![close("Close")])
}
