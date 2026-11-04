//! The project palette: short names such as `roof` bound to material files,
//! in named palettes of which one is active. Every material parameter accepts
//! an active palette name, and `palette.apply` swaps the whole look at once.
//!
//! The palettes live in `palettes.json` at the project root, beside
//! `jackdaw.toml`, so they are committed with the project and never ship with
//! the game: faces keep real material references, and the palette is only how
//! an author names them.

use std::collections::BTreeMap;
use std::path::PathBuf;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_commands::{CommandGroup, EditorCommand};
use jackdaw_feathers::button::{ButtonOperatorCall, ButtonProps, ButtonVariant, button};
use jackdaw_feathers::tokens;
use serde::{Deserialize, Serialize};

use crate::EditorEntity;
use crate::brush::refuse;
use crate::brush::{Brush, SetBrush};
use crate::commands::CommandHistory;
use crate::material_assets::MaterialRegistry;
use crate::worn_material::WornMaterial;

/// The file the palettes are kept in, at the project root.
pub const PALETTE_FILE: &str = "palettes.json";

const DEFAULT_PALETTE: &str = "default";

/// Every palette of the project and which one is active. Each palette maps a
/// name to a material file path under the assets folder.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaterialPalettes {
    pub active: String,
    pub palettes: BTreeMap<String, BTreeMap<String, String>>,
    /// Why the file could not be read. While set, the palettes are read-only
    /// so a later write never replaces what the file held.
    #[serde(skip)]
    pub unreadable: Option<String>,
}

impl Default for MaterialPalettes {
    fn default() -> Self {
        Self {
            active: DEFAULT_PALETTE.to_string(),
            palettes: BTreeMap::new(),
            unreadable: None,
        }
    }
}

impl MaterialPalettes {
    /// The material file the active palette binds `name` to.
    pub fn bound(&self, name: &str) -> Option<&str> {
        self.palettes
            .get(&self.active)?
            .get(name)
            .map(String::as_str)
    }
}

/// Raised whenever the palettes change, so the panels showing them refresh.
#[derive(Event, Clone, Copy, Debug)]
pub struct PalettesChanged;

/// Whether `text` can be a palette or palette name: 1 to 64 ASCII letters,
/// digits, `_` or `-`. With no `/` and no `.`, it can never read as a path.
pub fn is_palette_name(text: &str) -> bool {
    (1..=64).contains(&text.len())
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

/// [`MaterialRegistry::palette_path`] for a caller that may have no registry.
pub fn palette_path_in(registry: Option<&MaterialRegistry>, reference: &str) -> String {
    registry
        .map_or(reference, |registry| registry.palette_path(reference))
        .to_string()
}

/// The palettes as `jackdaw/status` reports them.
pub fn palette_status(world: &World) -> serde_json::Value {
    let Some(palettes) = world.get_resource::<MaterialPalettes>() else {
        return serde_json::Value::Null;
    };
    serde_json::json!({
        "active": palettes.active,
        "palettes": palettes.palettes,
        "error": palettes.unreadable,
    })
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<PaletteSetOp>()
        .register_operator::<PaletteApplyOp>();
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<MaterialPalettes>()
        .add_systems(OnEnter(crate::AppState::Editor), load_palettes)
        .add_systems(
            Update,
            load_palettes
                .run_if(resource_exists_and_changed::<crate::project::ProjectRoot>)
                .run_if(in_state(crate::AppState::Editor)),
        )
        .add_observer(on_palettes_changed)
        .add_observer(on_palette_section_added)
        .add_observer(on_preset_row_added);
}

fn palette_file(world: &World) -> Option<PathBuf> {
    world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|project| project.root.join(PALETTE_FILE))
}

/// Read the open project's palettes; a project with none starts empty. A file
/// that does not parse leaves the palettes empty and read-only.
pub fn load_palettes(world: &mut World) {
    let palettes = match palette_file(world).map(std::fs::read_to_string) {
        Some(Ok(text)) => serde_json::from_str(&text).unwrap_or_else(|err| {
            warn!("{PALETTE_FILE} could not be read: {err}");
            MaterialPalettes {
                unreadable: Some(err.to_string()),
                ..default()
            }
        }),
        _ => MaterialPalettes::default(),
    };
    if *world.resource::<MaterialPalettes>() != palettes {
        publish(world, palettes);
    }
}

/// Make `palettes` the live ones and hand the active bindings to the
/// resolvers.
fn publish(world: &mut World, palettes: MaterialPalettes) {
    let active = palettes
        .palettes
        .get(&palettes.active)
        .cloned()
        .unwrap_or_default();
    if let Some(mut registry) = world.get_resource_mut::<MaterialRegistry>() {
        registry.palette = active;
    }
    world.insert_resource(palettes);
    world.trigger(PalettesChanged);
}

/// Refuse a change while the file could not be read, reading it again first in
/// case it has been fixed.
fn writable(world: &mut World) -> Result<(), String> {
    if world.resource::<MaterialPalettes>().unreadable.is_some() {
        load_palettes(world);
    }
    match &world.resource::<MaterialPalettes>().unreadable {
        Some(err) => Err(format!(
            "{PALETTE_FILE} could not be read ({err}); fix it before changing palettes"
        )),
        None => Ok(()),
    }
}

fn write_palettes(world: &World, palettes: &MaterialPalettes) -> Result<(), String> {
    let file = palette_file(world).ok_or("no project is open")?;
    let text = serde_json::to_string_pretty(palettes).map_err(|err| err.to_string())?;
    std::fs::write(&file, text + "\n").map_err(|err| format!("{}: {err}", file.display()))
}

/// Make `palettes` the project's, on disk first so a failed write changes
/// nothing.
fn store(world: &mut World, palettes: MaterialPalettes) -> Result<(), String> {
    writable(world)?;
    write_palettes(world, &palettes)?;
    publish(world, palettes);
    Ok(())
}

/// Point every binding at `from` to `to`, across every palette, or drop those
/// bindings when the material is gone.
pub fn repoint_bindings(world: &mut World, from: &str, to: Option<&str>) {
    let Some(mut palettes) = world.get_resource::<MaterialPalettes>().cloned() else {
        return;
    };
    let mut touched = Vec::new();
    for (palette, bindings) in &mut palettes.palettes {
        for (name, path) in bindings.iter_mut() {
            if path == from {
                touched.push(format!("{palette}/{name}"));
                if let Some(to) = to {
                    *path = to.to_string();
                }
            }
        }
        if to.is_none() {
            bindings.retain(|_, path| path != from);
        }
    }
    if touched.is_empty() {
        return;
    }
    if to.is_none() {
        warn!(
            "{from} is gone, so these palette names stand for nothing now: {}",
            touched.join(", ")
        );
    }
    if let Err(why) = store(world, palettes) {
        warn!("palette: {why}");
    }
}

/// Bind `name` to the material file `path` in `palette`, the active one when
/// `None`; `None` for `path` unbinds it. Returns whether anything changed.
pub fn bind_path(
    world: &mut World,
    palette: Option<&str>,
    name: &str,
    path: Option<String>,
) -> Result<bool, String> {
    if !is_palette_name(name) {
        return Err(format!(
            "`name` {name:?} must be 1 to 64 ASCII letters, digits, `_` or `-`"
        ));
    }
    writable(world)?;
    let mut palettes = world.resource::<MaterialPalettes>().clone();
    let palette = palette.unwrap_or(&palettes.active).to_string();
    if !is_palette_name(&palette) {
        return Err(format!(
            "`palette` {palette:?} must be 1 to 64 ASCII letters, digits, `_` or `-`"
        ));
    }
    let bindings = palettes.palettes.entry(palette).or_default();
    match path {
        Some(path) => bindings.insert(name.to_string(), path),
        None => bindings.remove(name),
    };
    if palettes == *world.resource::<MaterialPalettes>() {
        return Ok(false);
    }
    store(world, palettes)?;
    Ok(true)
}

/// The file path a material reference resolves to, for a material saved to a
/// file of its own.
fn material_path(world: &World, reference: &str) -> Result<String, String> {
    let handle = crate::material_params::material_named(world, reference)
        .ok_or_else(|| format!("`material` {reference:?} names no material in this project"))?;
    crate::material_assets::filed_path(world, &handle)
        .ok_or_else(|| format!("`material` {reference:?} has no file; save it first"))
}

/// Bind a short name to a material in a palette, so every material parameter
/// can name it.
#[operator(
    id = "palette.set",
    writes_disk = true,
    label = "Set Palette Name",
    description = "Bind a short palette name such as roof to a material.",
    allows_undo = false,
    params(
        name(
            String,
            doc = "The palette name, such as roof: ASCII letters, digits, `_` and `-`."
        ),
        material(
            String,
            doc = "The material it stands for: a file path, a material name or another \
                   palette name. Empty unbinds the name."
        ),
        palette(
            String,
            doc = "The palette to bind it in, such as winter; made when new. Defaults to the \
                   active palette."
        ),
    )
)]
pub(crate) fn palette_set(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let result = match (params.as_str("name"), params.get("material")) {
        (Some(name), Some(_)) => match params.as_str("material").unwrap_or_default() {
            "" => Ok(None),
            reference => material_path(world, reference).map(Some),
        }
        .and_then(|path| bind_path(world, params.as_str("palette"), name, path)),
        _ => Err("name the palette `name` and the `material` it stands for".into()),
    };
    match result {
        Ok(true) => {
            set_outcome(world, Outcome::Changed);
            OperatorResult::Finished
        }
        Ok(false) => {
            set_outcome(world, Outcome::Unchanged);
            OperatorResult::Finished
        }
        Err(why) => refuse(world, "palette.set", why),
    }
}

/// Make another palette the active one. Undo takes it back.
struct SwitchPalette {
    from: String,
    to: String,
}

impl SwitchPalette {
    fn activate(world: &mut World, palette: &str) {
        let mut palettes = world.resource::<MaterialPalettes>().clone();
        palettes.active = palette.to_string();
        if let Err(why) = store(world, palettes) {
            warn!("palette: {why}");
        }
    }
}

impl EditorCommand for SwitchPalette {
    fn execute(&mut self, world: &mut World) {
        Self::activate(world, &self.to);
    }

    fn undo(&mut self, world: &mut World) {
        Self::activate(world, &self.from);
    }

    fn description(&self) -> &str {
        "Switch palette"
    }
}

/// What the active palette's materials turn into under another palette.
struct Swap {
    from: AssetId<StandardMaterial>,
    to: Handle<StandardMaterial>,
    path: String,
}

/// What switching to `target` changes: each material the active palette binds
/// paired with what `target` binds under the same name, the names `target`
/// leaves unbound, and the bindings naming a material the project no longer
/// holds.
struct SwapPlan {
    swaps: Vec<Swap>,
    unbound: Vec<String>,
    stale: Vec<String>,
}

fn plan_swaps(world: &World, target: &str) -> SwapPlan {
    let palettes = world.resource::<MaterialPalettes>();
    let index = world.get_resource::<crate::asset_index::AssetIndex>();
    let registry = world.resource::<MaterialRegistry>();
    let lookup = |path: &str| {
        crate::material_assets::material_of_reference(index, registry, path)
            .filter(|handle| *handle != Handle::default())
    };
    let wanted = palettes.palettes.get(target);
    let mut plan = SwapPlan {
        swaps: Vec::new(),
        unbound: Vec::new(),
        stale: Vec::new(),
    };
    for (name, path) in palettes
        .palettes
        .get(&palettes.active)
        .into_iter()
        .flatten()
    {
        let Some(new_path) = wanted.and_then(|wanted| wanted.get(name)) else {
            plan.unbound.push(name.clone());
            continue;
        };
        let (Some(from), Some(to)) = (lookup(path), lookup(new_path)) else {
            plan.stale.push(name.clone());
            continue;
        };
        if from != to {
            plan.swaps.push(Swap {
                from: from.id(),
                to,
                path: new_path.clone(),
            });
        }
    }
    plan
}

/// The commands that dress every scene brush face and mesh wearing a swapped
/// material in its replacement.
fn recolour(world: &mut World, swaps: &[Swap]) -> Vec<Box<dyn EditorCommand>> {
    let swap_of = |id: AssetId<StandardMaterial>| swaps.iter().find(|swap| swap.from == id);
    let mut commands: Vec<Box<dyn EditorCommand>> = Vec::new();
    let brushes: Vec<(Entity, Brush)> = world
        .query_filtered::<(Entity, &Brush), Without<EditorEntity>>()
        .iter(world)
        .filter(|(_, brush)| {
            brush
                .faces
                .iter()
                .any(|face| swap_of(face.material.id()).is_some())
        })
        .map(|(entity, brush)| (entity, brush.clone()))
        .collect();
    for (entity, old) in brushes {
        let mut new = old.clone();
        for face in &mut new.faces {
            if let Some(swap) = swap_of(face.material.id()) {
                face.material = swap.to.clone();
            }
        }
        commands.push(Box::new(SetBrush {
            entity,
            old,
            new,
            label: "Apply palette".into(),
        }));
    }
    let meshes: Vec<(Entity, AssetId<StandardMaterial>)> = world
        .query_filtered::<(Entity, &MeshMaterial3d<StandardMaterial>), (Without<Brush>, Without<EditorEntity>)>()
        .iter(world)
        .map(|(entity, material)| (entity, material.0.id()))
        .collect();
    for (entity, id) in meshes {
        let Some(swap) = swap_of(id) else {
            continue;
        };
        let authored = world
            .get_resource::<jackdaw_bsn::SceneBsnAst>()
            .is_some_and(|doc| doc.ast_for(entity).is_some());
        if !authored {
            continue;
        }
        if let Some(command) = crate::inspector::material_row::WearMaterial::new(
            world,
            entity,
            WornMaterial::Standard(swap.to.clone()),
            Some(swap.path.clone()),
        ) {
            commands.push(Box::new(command));
        }
    }
    commands
}

/// Switch to another palette, putting its materials on everything that wears
/// the active palette's, as one undo step. Materials outside the palette stay.
#[operator(
    id = "palette.apply",
    writes_disk = true,
    label = "Apply Palette",
    description = "Swap the whole look: switch to another palette and recolour everything that \
                   wears the active palette's materials.",
    allows_undo = false,
    params(palette(String, doc = "The palette to switch to, such as winter."))
)]
pub(crate) fn palette_apply(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    match apply_palette(world, params.as_str("palette")) {
        Ok(outcome) => {
            set_outcome(world, outcome);
            OperatorResult::Finished
        }
        Err(why) => refuse(world, "palette.apply", why),
    }
}

fn apply_palette(world: &mut World, target: Option<&str>) -> Result<Outcome, String> {
    let target = target.ok_or("name the `palette` to switch to")?.to_string();
    writable(world)?;
    let palettes = world.resource::<MaterialPalettes>();
    if !palettes.palettes.contains_key(&target) {
        let known: Vec<&str> = palettes.palettes.keys().map(String::as_str).collect();
        return Err(format!(
            "no palette `{target}`; the palettes are {}",
            if known.is_empty() {
                "none yet".to_string()
            } else {
                known.join(", ")
            }
        ));
    }
    if palettes.active == target {
        return Ok(Outcome::Unchanged);
    }
    let from = palettes.active.clone();
    let plan = plan_swaps(world, &target);

    let mut switched = world.resource::<MaterialPalettes>().clone();
    switched.active = target.clone();
    store(world, switched)?;

    let mut recoloured = recolour(world, &plan.swaps);
    for command in &mut recoloured {
        command.execute(world);
    }
    let dressed = recoloured.len();
    let mut group: Vec<Box<dyn EditorCommand>> = vec![Box::new(SwitchPalette {
        from,
        to: target.clone(),
    })];
    group.extend(recoloured);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(CommandGroup {
            commands: group,
            label: "Apply palette".into(),
        }));
    report_to_caller(
        world,
        format!("palette.apply: `{target}` is active; {dressed} nodes recoloured"),
    );
    if !plan.unbound.is_empty() {
        warn_caller(
            world,
            format!(
                "palette.apply: `{target}` binds no {}, so what wears them is left as it was",
                plan.unbound.join(", ")
            ),
        );
    }
    if !plan.stale.is_empty() {
        warn_caller(
            world,
            format!(
                "palette.apply: {} name a material this project no longer holds, so what wears \
                 them is left as it was",
                plan.stale.join(", ")
            ),
        );
    }
    Ok(Outcome::Changed)
}

// -- Material browser rows ----------------------------------------------------

/// The material browser's row of presets, each making a material from one.
#[derive(Component)]
pub struct MaterialPresetRow;

/// The material browser's palette section: the palettes to switch between and
/// what the active one binds.
#[derive(Component)]
pub struct MaterialPaletteSection;

fn on_preset_row_added(event: On<Add<MaterialPresetRow>>, mut commands: Commands) {
    let row = event.entity;
    for preset in &crate::material_params::PRESETS {
        commands.spawn((
            button(ButtonProps::new(preset.name).with_variant(ButtonVariant::Ghost)),
            ButtonOperatorCall::new(crate::material_browser::MaterialCreateOp::ID)
                .with_param("preset", preset.name),
            ChildOf(row),
        ));
    }
}

fn on_palette_section_added(event: On<Add<MaterialPaletteSection>>, mut commands: Commands) {
    let section = event.entity;
    commands.queue(move |world: &mut World| fill_palette_section(world, section));
}

fn on_palettes_changed(
    _: On<PalettesChanged>,
    sections: Query<Entity, With<MaterialPaletteSection>>,
    mut commands: Commands,
) {
    for section in &sections {
        commands.queue(move |world: &mut World| fill_palette_section(world, section));
    }
}

fn small_text(world: &mut World, parent: Entity, text: String, color: Color) {
    world.spawn((
        Text::new(text),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(color),
        ChildOf(parent),
    ));
}

fn fill_palette_section(world: &mut World, section: Entity) {
    let Ok(mut entity) = world.get_entity_mut(section) else {
        return;
    };
    entity.despawn_children();
    let palettes = world.resource::<MaterialPalettes>().clone();
    if palettes.palettes.is_empty() {
        small_text(
            world,
            section,
            "No palette names yet".to_string(),
            tokens::TEXT_SECONDARY,
        );
        return;
    }
    let switcher = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                column_gap: Val::Px(tokens::SPACING_XS),
                row_gap: Val::Px(tokens::SPACING_XS),
                ..default()
            },
            ChildOf(section),
        ))
        .id();
    for name in palettes.palettes.keys() {
        let variant = if *name == palettes.active {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Default
        };
        world.spawn((
            button(ButtonProps::new(name.clone()).with_variant(variant)),
            ButtonOperatorCall::new(PaletteApplyOp::ID).with_param("palette", name.clone()),
            ChildOf(switcher),
        ));
    }
    for (name, path) in palettes
        .palettes
        .get(&palettes.active)
        .into_iter()
        .flatten()
    {
        small_text(
            world,
            section,
            format!("{name}: {}", jackdaw_bsn::asset_stem(path)),
            tokens::TEXT_PRIMARY,
        );
    }
}
