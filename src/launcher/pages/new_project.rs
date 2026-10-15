//! The New project page: editor, template, modules, extensions, name and
//! location, and what creating it will write.

use std::path::PathBuf;

use bevy::feathers::controls::{
    ButtonVariant, FeathersMenu, FeathersMenuButton, FeathersMenuItem, FeathersMenuPopup,
};
use bevy::feathers::theme::ThemedText;
use bevy::input_focus::tab_navigation::TabGroup;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use bevy::ui_widgets::Button;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::tokens;
use jackdaw_project_build::modules::ModuleCatalog;

use super::super::model::{editor_chip, version_label};
use super::super::ui::{self, Tone};
use super::super::{Launcher, LauncherField, NewProjectForm, Template, jd};
use super::TextSlot;

/// A part of the New project page refilled from the form.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NewProjectRegion {
    #[default]
    Editor,
    Templates,
    Modules,
    Extensions,
    Details,
    Git,
    Preview,
    Footer,
}

impl NewProjectRegion {
    /// Whether this part shows something that differs between `before`
    /// and `now`.
    pub fn follows_change(self, before: &NewProjectForm, now: &NewProjectForm) -> bool {
        match self {
            Self::Editor => before.editor != now.editor,
            Self::Templates | Self::Details | Self::Extensions => before.template != now.template,
            Self::Modules => before.template != now.template || before.modules != now.modules,
            Self::Git => before.git != now.git,
            Self::Preview => {
                before.name != now.name
                    || before.location != now.location
                    || before.template != now.template
                    || before.modules != now.modules
                    || before.editor != now.editor
                    || before.git != now.git
            }
            Self::Footer => before.status != now.status || before.creating != now.creating,
        }
    }
}

fn label(text: &'static str) -> impl Scene {
    ui::text(text, tokens::TEXT_SIZE_SM, tokens::DOC_TAB_INACTIVE_LABEL)
}

pub fn page() -> impl Scene {
    let back = ButtonOperatorCall::new("launcher.show_page").with_param("page", "projects");
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
                @{ui::icon_button(Icon::ArrowLeft, "", back, ButtonVariant::Plain)}
                --
                Node { flex_grow: 1.0 }
                Text("New project")
                TextFont { font_size: tokens::TEXT_SIZE_LG }
                TextColor(tokens::TEXT_PRIMARY)
                --
                @{label("Editor")}
                --
                ~{NewProjectRegion::Editor} Node { width: px(300) }
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
                    row_gap: px(12),
                    overflow: Overflow::scroll_y(),
                }
                ScrollPosition::default()
                bevy::ui_widgets::ScrollArea
                Hovered::default()
                Children [
                    @{label("Template")}
                    --
                    ~{NewProjectRegion::Templates} Node { flex_direction: FlexDirection::Column }
                    --
                    ~{NewProjectRegion::Modules} Node { flex_direction: FlexDirection::Column, row_gap: px(6) }
                    --
                    ~{NewProjectRegion::Extensions} Node { flex_direction: FlexDirection::Column, row_gap: px(6) }
                ]
                --
                Node {
                    width: px(340),
                    flex_shrink: 0.0,
                    flex_direction: FlexDirection::Column,
                    row_gap: px(8),
                    padding: UiRect::all(px(12)),
                    border_radius: BorderRadius::all(px(4)),
                }
                BackgroundColor(tokens::ELEVATED_BG)
                Children [
                    ~{NewProjectRegion::Details} Node { flex_direction: FlexDirection::Column, row_gap: px(6) }
                    --
                    @{label("Name")}
                    --
                    Node { width: percent(100) }
                    TextSlot { field: LauncherField::NewProjectName, placeholder: "my_game" }
                    --
                    @{label("Location")}
                    --
                    Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: px(6),
                    }
                    Children [
                        Node { flex_grow: 1.0, min_width: px(0) }
                        TextSlot { field: LauncherField::NewProjectLocation, placeholder: "Folder" }
                        --
                        @{ui::icon_button(Icon::FolderOpen, "", ButtonOperatorCall::new("launcher.new_project_browse"), ButtonVariant::Normal)}
                    ]
                    --
                    ~{NewProjectRegion::Git} Node { flex_direction: FlexDirection::Column }
                    --
                    ~{NewProjectRegion::Preview} Node { flex_direction: FlexDirection::Column }
                    --
                    @{ui::spacer()}
                    --
                    ~{NewProjectRegion::Footer} Node { flex_direction: FlexDirection::Column, row_gap: px(6) }
                ]
            ]
        ]
    }
}

/// The content of one part of the page.
pub fn region(
    region: NewProjectRegion,
    launcher: &Launcher,
    form: &NewProjectForm,
    catalog: Option<&jackdaw_api_internal::lifecycle::ExtensionCatalog>,
) -> impl Scene {
    let scene: Box<dyn SceneList> = match region {
        NewProjectRegion::Editor => ui::boxed(editor_picker(launcher, form)),
        NewProjectRegion::Templates => ui::boxed(templates(form)),
        NewProjectRegion::Modules => ui::boxed(modules(form, catalog)),
        NewProjectRegion::Extensions => ui::boxed(extensions(form, catalog)),
        NewProjectRegion::Details => ui::boxed(details(form)),
        NewProjectRegion::Git => ui::boxed(ui::checkbox(
            "Create a git repository",
            form.git,
            false,
            ButtonOperatorCall::new("launcher.new_project_git"),
        )),
        NewProjectRegion::Preview => ui::boxed(preview(launcher, form)),
        NewProjectRegion::Footer => ui::boxed(footer(form)),
    };
    bsn! {
        Node { flex_direction: FlexDirection::Column, row_gap: px(6) }
        Children [ {scene} ]
    }
}

/// The editor the form makes the project with, as its chip says.
fn chosen_editor(launcher: &Launcher, form: &NewProjectForm) -> String {
    match form
        .editor
        .as_deref()
        .and_then(|version| launcher.installs.get(version))
    {
        Some(install) => format!(
            "jackdaw {} - {}",
            editor_chip(&install.version, &install.bevy),
            super::super::model::channel_label(install.channel)
        ),
        None => format!(
            "jackdaw {} - {}",
            launcher.running.chip(),
            super::super::model::channel_label(launcher.running.channel)
        ),
    }
}

fn editor_picker(launcher: &Launcher, form: &NewProjectForm) -> impl Scene {
    let caption = chosen_editor(launcher, form);
    let mut items: ui::Scenes = vec![ui::boxed(editor_item(
        format!("jackdaw {} (this editor)", launcher.running.chip()),
        String::new(),
    ))];
    for install in launcher.other_installs() {
        items.push(ui::boxed(editor_item(
            format!("jackdaw {}", editor_chip(&install.version, &install.bevy)),
            install.version.clone(),
        )));
    }
    bsn! {
        @FeathersMenu
        Node { width: percent(100) }
        Children [
            @FeathersMenuButton {
                @caption: bsn! { Text(caption) ThemedText Node { margin: UiRect::right(px(6)) } },
            }
            Node { flex_grow: 1.0 }
            --
            @FeathersMenuPopup
            ~{TabGroup::modal()}
            Node { min_width: px(300) }
            Children [ {items} ]
        ]
    }
}

fn editor_item(caption: String, version: String) -> impl Scene {
    let call =
        ButtonOperatorCall::new("launcher.new_project_editor").with_param("version", version);
    bsn! {
        @FeathersMenuItem {
            @caption: bsn! { Text(caption) ThemedText Node { margin: UiRect::right(px(6)) } },
        }
        ~{call}
    }
}

/// A template card in the gallery.
#[derive(Component, Default, Clone, Debug)]
pub struct TemplateCard(pub &'static str);

fn template_card(template: Template, selected: bool) -> impl Scene {
    let call = ButtonOperatorCall::new("launcher.new_project_template")
        .with_param("template", template.id());
    let (background, outline) = if selected {
        (tokens::SELECTED_BG, tokens::SELECTED_BORDER)
    } else {
        (tokens::ELEVATED_BG, Color::NONE)
    };
    bsn! {
        TemplateCard({template.id()})
        Node {
            flex_grow: 1.0,
            flex_basis: px(0),
            height: px(120),
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::FlexEnd,
            row_gap: px(4),
            padding: UiRect::all(px(10)),
            border: UiRect::all(px(2)),
            border_radius: BorderRadius::all(px(4)),
        }
        BackgroundColor(background)
        BorderColor::all(outline)
        Button
        Hovered::default()
        ~{call}
        Children [
            @{ui::primary(template.title())}
            --
            @{ui::dim(template.summary())}
        ]
    }
}

fn templates(form: &NewProjectForm) -> impl Scene {
    let cards: Vec<_> = Template::ALL
        .into_iter()
        .map(|template| template_card(template, template == form.template))
        .collect();
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            column_gap: px(8),
        }
        Children [ {cards} ]
    }
}

fn modules(
    form: &NewProjectForm,
    catalog: Option<&jackdaw_api_internal::lifecycle::ExtensionCatalog>,
) -> impl Scene {
    let mut rows: ui::Scenes = Vec::new();
    if form.template == Template::Extension {
        rows.push(ui::boxed(ui::note(
            "An extension adds to the editor; modules are chosen per game.",
            tokens::TEXT_SECONDARY,
        )));
    } else {
        for module in ModuleCatalog::builtin().modules() {
            let on = form.modules.contains(&module.id);
            let included = crate::module_picker::catalog_has_module(catalog, &module.id);
            let call = ButtonOperatorCall::new("launcher.new_project_module")
                .with_param("id", module.id.clone());
            let unavailable = module.unavailable.as_ref().filter(|_| !on);
            let status = if unavailable.is_some() {
                ui::boxed(ui::chip("not compatible", Tone::Caution))
            } else if included {
                ui::boxed(ui::chip("tools built in", Tone::Success))
            } else {
                ui::boxed(ui::chip("not in this editor", Tone::Caution))
            };
            let description = match unavailable {
                Some(reason) => format!("{} Not compatible: {reason}.", module.description),
                None => module.description.clone(),
            };
            let disabled = unavailable.is_some();
            rows.push(ui::boxed(bsn! {
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(8),
                    min_height: px(30),
                }
                Children [
                    Node { width: px(160), flex_shrink: 0.0 } Children [ @{ui::checkbox(module.name.clone(), on, disabled, call)} ]
                    --
                    Node { flex_grow: 1.0, min_width: px(0) } Children [ @{ui::note(description, tokens::DOC_TAB_INACTIVE_LABEL)} ]
                    --
                    {status}
                ]
            }));
        }
    }
    bsn! {
        Node { flex_direction: FlexDirection::Column, row_gap: px(6) }
        Children [
            Node { flex_direction: FlexDirection::Row, column_gap: px(8), align_items: AlignItems::Center }
            Children [
                @{label("Modules")}
                --
                @{ui::dim("written to Cargo.toml, main.rs and jackdaw.toml [modules]")}
            ]
            --
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(2),
                padding: UiRect::axes(px(10), px(6)),
                border_radius: BorderRadius::all(px(4)),
            }
            BackgroundColor(tokens::ELEVATED_BG)
            Children [ {rows} ]
        ]
    }
}

/// The editor extensions turned on in the global list, which a new project
/// starts with. Built-in extensions are left out.
pub fn global_extensions(
    catalog: Option<&jackdaw_api_internal::lifecycle::ExtensionCatalog>,
) -> Vec<String> {
    let mut enabled: Vec<String> = jackdaw_api_internal::extensions_config::read_extension_config()
        .map(|config| {
            config
                .iter()
                .filter(|(_, entry)| entry.enabled)
                .map(|(id, _)| id.clone())
                .collect()
        })
        .unwrap_or_default();
    enabled.retain(|id| catalog.is_none_or(|catalog| !catalog.is_builtin(id)));
    enabled.sort();
    enabled
}

fn extensions(
    form: &NewProjectForm,
    catalog: Option<&jackdaw_api_internal::lifecycle::ExtensionCatalog>,
) -> impl Scene {
    let enabled = global_extensions(catalog);
    let mut chips: ui::Scenes = enabled
        .iter()
        .map(|id| ui::boxed(ui::chip(id.clone(), Tone::Neutral)))
        .collect();
    let note = if form.template == Template::Extension {
        "An extension project is loaded into the editor itself."
    } else if enabled.is_empty() {
        "No extensions are on in your global defaults."
    } else {
        "from your global defaults"
    };
    chips.push(ui::boxed(ui::dim(note)));
    let manage = ButtonOperatorCall::new("launcher.extensions");
    bsn! {
        Node { flex_direction: FlexDirection::Column, row_gap: px(6) }
        Children [
            @{label("Extensions")}
            --
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(8),
                row_gap: px(6),
                padding: UiRect::axes(px(10), px(8)),
                border_radius: BorderRadius::all(px(4)),
            }
            BackgroundColor(tokens::ELEVATED_BG)
            Children [
                {chips}
                --
                @{ui::spacer()}
                --
                @{ui::button("Global defaults...", manage, ButtonVariant::Normal)}
            ]
        ]
    }
}

fn details(form: &NewProjectForm) -> impl Scene {
    bsn! {
        Node { flex_direction: FlexDirection::Column, row_gap: px(6) }
        Children [
            @{ui::primary(form.template.title())}
            --
            @{ui::note(form.template.description(), tokens::TEXT_SECONDARY)}
        ]
    }
}

/// The folder the form would create.
pub fn destination(form: &NewProjectForm) -> Result<PathBuf, String> {
    let name =
        crate::scaffold::validated_project_name(&form.name).map_err(|error| error.to_string())?;
    let dest = form.location.join(name);
    let occupied = dest
        .read_dir()
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    if occupied {
        return Err(format!(
            "{} already exists and is not empty",
            dest.display()
        ));
    }
    Ok(dest)
}

/// The files the form would write, and what goes into them.
pub fn will_write(launcher: &Launcher, form: &NewProjectForm) -> Vec<String> {
    let (version, bevy) = match form
        .editor
        .as_deref()
        .and_then(|version| launcher.installs.get(version))
    {
        Some(install) => (install.version.clone(), install.bevy.clone()),
        None => (
            launcher.running.version.clone(),
            launcher.running.bevy.clone(),
        ),
    };
    if form.template == Template::Extension {
        return vec![
            format!("Cargo.toml - jackdaw_api {}", version_label(&version)),
            format!("jackdaw.toml - editor {}", version_label(&version)),
        ];
    }
    let catalog = ModuleCatalog::builtin();
    let mut crates = vec![
        format!("bevy {bevy}"),
        format!("jackdaw_runtime {}", version_label(&version)),
    ];
    for id in &form.modules {
        if let Some(module) = catalog.get(id) {
            for dependency in &module.crates {
                crates.push(match &dependency.version {
                    Some(requirement) => format!("{} {requirement}", dependency.name),
                    None => dependency.name.clone(),
                });
            }
        }
    }
    let modules = if form.modules.is_empty() {
        "no modules".to_string()
    } else {
        format!(
            "modules {}",
            form.modules.iter().cloned().collect::<Vec<_>>().join(" ")
        )
    };
    let mut lines = vec![
        format!("Cargo.toml - {}", crates.join(", ")),
        format!(
            "jackdaw.toml - editor {}, {modules}",
            version_label(&version)
        ),
    ];
    if form.template != Template::ThreeD {
        lines.push(format!("assets/scene.bsn - {}", form.template.summary()));
    }
    lines
}

fn preview(launcher: &Launcher, form: &NewProjectForm) -> impl Scene {
    let mut lines: ui::Scenes = vec![ui::boxed(ui::dim("Will write"))];
    match destination(form) {
        Ok(dest) => {
            lines.push(ui::boxed(ui::note(
                super::projects::display_path(&dest),
                tokens::TEXT_PRIMARY,
            )));
            for line in will_write(launcher, form) {
                lines.push(ui::boxed(ui::note(line, tokens::TEXT_SECONDARY)));
            }
        }
        Err(problem) => lines.push(ui::boxed(ui::note(problem, tokens::CHIP_WARNING_TEXT))),
    }
    bsn! {
        Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(4),
            padding: UiRect::all(px(8)),
            border_radius: BorderRadius::all(px(4)),
        }
        BackgroundColor(tokens::PANEL_BG)
        Children [ {lines} ]
    }
}

/// The New project page's Create button.
#[derive(Component, Default, Clone)]
pub struct CreateProjectButton;

fn footer(form: &NewProjectForm) -> impl Scene {
    let status: ui::Scenes = form
        .status
        .iter()
        .map(|status| ui::boxed(ui::note(status.clone(), tokens::CHIP_WARNING_TEXT)))
        .collect();
    let caption = if form.creating {
        "Creating..."
    } else {
        "Create project"
    };
    let create = ButtonOperatorCall::new("launcher.new_project_create");
    bsn! {
        Node { flex_direction: FlexDirection::Column, row_gap: px(6) }
        Children [
            {status}
            --
            @{ui::button(caption, create, ButtonVariant::Primary)}
            CreateProjectButton
            Node { height: px(30), width: percent(100) }
        ]
    }
}

/// A project being scaffolded by this editor.
#[derive(Resource)]
pub struct NewProjectTask(Task<Result<PathBuf, String>>);

/// Create the project `form` describes and open it: with this editor's own
/// templates, or with the chosen install's `jd new`.
pub fn create_project(world: &mut World, form: NewProjectForm) {
    let dest = match destination(&form) {
        Ok(dest) => dest,
        Err(problem) => {
            set_status(world, Some(problem), false);
            return;
        }
    };
    let name = dest
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    crate::project::save_last_new_project_location(&form.location);
    let install = form
        .editor
        .as_deref()
        .and_then(|version| world.resource::<Launcher>().installs.get(version).cloned());
    match install {
        Some(install) => {
            let jd = install
                .path
                .join(if cfg!(windows) { "jd.exe" } else { "jd" });
            if !jd.is_file() {
                set_status(
                    world,
                    Some(format!(
                        "jackdaw {} has no jd beside it; open that editor to create the project",
                        version_label(&install.version)
                    )),
                    false,
                );
                return;
            }
            let mut args = vec![
                "new".to_string(),
                name.clone(),
                "--path".into(),
                form.location.to_string_lossy().into_owned(),
            ];
            if form.template == Template::Extension {
                args.push("--extension".into());
            } else if form.modules.is_empty() {
                args.push("--no-modules".into());
            } else {
                args.push("--modules".into());
                args.push(form.modules.iter().cloned().collect::<Vec<_>>().join(","));
            }
            if !form.git {
                args.push("--no-git".into());
            }
            jd::start_job_with(
                world,
                jd,
                jd::JobKind::NewProject { name },
                args,
                Some(jd::AfterJob::NewProject {
                    project: dest,
                    version: install.version.clone(),
                    starter: form.template.starter(),
                }),
            );
            set_status(world, None, true);
        }
        None => {
            let location = form.location.clone();
            let task = AsyncComputeTaskPool::get().spawn(async move {
                crate::new_project::scaffold_project_with(
                    &name,
                    &location,
                    form.template.kind(),
                    &form.modules,
                    form.template.starter(),
                    form.git,
                )
                .map_err(|error| error.to_string())
            });
            world.insert_resource(NewProjectTask(task));
            set_status(world, None, true);
        }
    }
}

fn set_status(world: &mut World, status: Option<String>, creating: bool) {
    let mut form = world.resource_mut::<NewProjectForm>();
    if form.status != status {
        form.status = status;
    }
    if form.creating != creating {
        form.creating = creating;
    }
}

/// Open the project this editor finished scaffolding.
pub fn poll_new_project(world: &mut World) {
    let Some(mut task) = world.get_resource_mut::<NewProjectTask>() else {
        return;
    };
    let Some(result) = future::block_on(future::poll_once(&mut task.0)) else {
        return;
    };
    world.remove_resource::<NewProjectTask>();
    match result {
        Ok(dest) => {
            set_status(world, None, false);
            let name = dest
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            crate::project::touch_recent(&dest, &name);
            super::super::open_project(world, dest);
        }
        Err(error) => set_status(world, Some(error), false),
    }
}
