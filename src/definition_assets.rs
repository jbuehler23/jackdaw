//! Editing one asset file: opening it, writing its fields and saving it back.
//!
//! A file says which type it holds, so the editor finds a kind's files by
//! reading them rather than by where they sit, and
//! [`crate::asset_index::AssetIndex`] holds what every one of them loaded to. A
//! type the editor has compiled in loads through [`jackdaw_bsn::load_bsn_assets`]
//! and saves through [`jackdaw_bsn::serialize_assets_to_bsn`]; a type the open
//! project reported in its schema is known no other way, so its files load as
//! the patch they hold and save back through the same emitter. Either way a
//! file holds only what the value changes from its default.
//!
//! Opening an asset puts it in the inspector: the open asset rides on an editor
//! entity carrying [`DefinitionAssetEdit`], the field rows read the value at
//! the path it names instead of a component, and their edits come back here as
//! [`SetDefinitionField`] undo entries.

use std::path::{Path, PathBuf};

use bevy::asset::{ReflectAsset, UntypedHandle};
use bevy::prelude::*;
use bevy::reflect::{GetPath, ReflectRef, prelude::ReflectDefault};
use jackdaw_api::prelude::{AssetKind, AssetKinds};
use jackdaw_api_internal::operator::report_to_caller;
use jackdaw_bsn::{BsnPatch, BsnPatches, BsnStructData, BsnStructFields, CatalogAssetRef};
use jackdaw_commands::CommandHistory;

use crate::EditorEntity;
use crate::asset_files::{AssetFileKind, asset_file_text, read_asset_kind};
use crate::asset_index::{AssetEntry, AssetIndex, AssetValue, absolute_path, indexed_path};
use crate::commands::EditorCommand;
use crate::prelude::*;
use crate::project::ProjectRoot;

/// The first `<kind>_N` name with neither an entry nor a file of its own in
/// the folder the new file lands in.
fn next_free_name(world: &World, kind: &str, dir: &Path) -> String {
    let index = world.resource::<AssetIndex>();
    let mut counter = 1u32;
    loop {
        let candidate = format!("{kind}_{counter}");
        let taken = index.of_kind(kind).any(|entry| entry.name() == candidate)
            || definition_file_path(dir, &candidate).exists();
        if !taken {
            return candidate;
        }
        counter += 1;
    }
}

/// The asset open in the inspector, on its own editor entity.
#[derive(Component)]
#[require(EditorEntity)]
pub struct DefinitionAssetEdit {
    pub kind: String,
    pub name: String,
    pub type_path: String,
    /// The file being edited, as the index keys it.
    pub path: PathBuf,
    /// Whether the asset has been edited since it was loaded or saved.
    pub dirty: bool,
}

/// The entity carrying the open asset, if one is open.
#[derive(Resource, Default)]
pub struct OpenDefinition(pub Option<Entity>);

/// Strip what cannot appear in a file stem, so an asset name always maps to
/// exactly one file.
pub fn sanitize_definition_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "definition".to_string()
    } else {
        cleaned
    }
}

/// Whether a path names a file to write rather than the folder to write in.
fn names_a_file(path: &Path) -> bool {
    !path.is_dir()
        && path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("bsn"))
}

/// The file an asset of this name lands in, in a folder of the user's choosing.
pub fn definition_file_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}.bsn", sanitize_definition_name(name)))
}

/// The name a file gives what it holds: everything before the first dot of its
/// file name, so `torch.item.bsn` holds `torch`.
pub fn definition_name_of(path: &Path) -> String {
    let file = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .trim_start_matches('.');
    file.split('.').next().unwrap_or(file).to_string()
}

/// The folder a new asset lands in: the one the browser is showing when it is
/// under the project's assets, and the assets directory otherwise.
pub fn new_definition_dir(world: &World) -> Option<PathBuf> {
    let assets = world.get_resource::<ProjectRoot>()?.assets_dir();
    let showing = world
        .get_resource::<crate::asset_browser::AssetBrowserState>()
        .map(|state| state.current_directory.clone());
    match showing {
        Some(dir) if dir.starts_with(&assets) && dir.is_dir() => Some(dir),
        _ => Some(assets),
    }
}

/// Read one asset file into a value. A file that cannot be read or parsed, or
/// that holds a value of another type, is reported and skipped.
pub fn read_asset_file(world: &mut World, kind: &AssetKind, path: &Path) -> Option<AssetValue> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            warn!("Failed to read {}: {err}", path.display());
            return None;
        }
    };
    if kind.schema_backed() {
        return read_schema_asset(kind, path, &text);
    }
    let type_id = registered_type_id(world, &kind.type_path)?;
    let entries = match jackdaw_bsn::load_bsn_assets(world, &text) {
        Ok(entries) => entries,
        Err(err) => {
            warn!("Failed to parse {}: {err}", path.display());
            return None;
        }
    };
    let entry = entries.into_iter().next()?;
    if entry.handle.type_id() != type_id {
        warn!("{} does not hold a {}", path.display(), kind.type_path);
        return None;
    }
    Some(AssetValue::Handle(entry.handle))
}

/// Read a file whose type the editor knows only as schema: the patch it holds
/// is the value.
fn read_schema_asset(kind: &AssetKind, path: &Path, text: &str) -> Option<AssetValue> {
    let ast = match jackdaw_bsn::parse_bsn_text(text) {
        Ok(ast) => ast,
        Err(err) => {
            warn!("Failed to parse {}: {err}", path.display());
            return None;
        }
    };
    let data = ast
        .roots
        .iter()
        .find_map(|&root| patch_of_root(&ast, root))
        .unwrap_or_else(|| empty_patch(&kind.type_path));
    if data.type_path != kind.type_path {
        warn!(
            "{} holds a {} where a {} was expected",
            path.display(),
            data.type_path,
            kind.type_path
        );
        return None;
    }
    Some(AssetValue::Schema(Box::new(data)))
}

/// The struct patch a document root carries, with a bare type reading as a
/// value that authors nothing.
fn patch_of_root(ast: &jackdaw_bsn::SceneBsnAst, root: Entity) -> Option<BsnStructData> {
    let patches = ast.get_patches(root)?;
    patches
        .0
        .iter()
        .find_map(|&patch| match ast.get_patch(patch) {
            Some(BsnPatch::Struct(data)) => Some(data.clone()),
            Some(BsnPatch::Type(type_path)) => Some(empty_patch(type_path)),
            _ => None,
        })
}

fn empty_patch(type_path: &str) -> BsnStructData {
    BsnStructData {
        type_path: type_path.to_string(),
        fields: BsnStructFields::default(),
    }
}

/// The name the root of an existing file carries, so a save writes the file
/// back under the name it already spells.
fn root_name_of(text: &str) -> Option<String> {
    let ast = jackdaw_bsn::parse_bsn_text(text).ok()?;
    ast.roots.iter().find_map(|&root| {
        ast.get_patches(root)?
            .0
            .iter()
            .find_map(|&patch| match ast.get_patch(patch) {
                Some(BsnPatch::Name(name)) => Some(name.clone()),
                _ => None,
            })
    })
}

/// Write one asset to the file it was opened from or created in, under the
/// root name that file already carries. An identical rewrite is skipped so the
/// asset watcher does not reload behind an unchanged save.
pub fn write_asset_file(
    world: &World,
    name: &str,
    value: &AssetValue,
    path: &Path,
) -> std::io::Result<PathBuf> {
    let existing = std::fs::read_to_string(path).ok();
    let name = existing
        .as_deref()
        .and_then(root_name_of)
        .unwrap_or_else(|| name.to_string());
    let text = asset_text(world, &name, value).unwrap_or_default();
    if text.trim().is_empty() {
        return Err(std::io::Error::other(format!(
            "nothing to write for '{name}'"
        )));
    }
    if existing.is_some_and(|existing| existing == text) {
        return Ok(path.to_path_buf());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::scene_io::save::write_atomic(path, text.as_bytes())?;
    Ok(path.to_path_buf())
}

/// The text one asset saves as: the stamp and the header naming its type, then
/// an asset catalog entry for a compiled type or the patch it holds for a
/// schema-backed one.
fn asset_text(world: &World, name: &str, value: &AssetValue) -> Option<String> {
    let body = match value {
        AssetValue::Handle(handle) => jackdaw_bsn::serialize_assets_to_bsn(
            world,
            &[CatalogAssetRef {
                name: sanitize_definition_name(name),
                type_id: handle.type_id(),
                asset_id: handle.id(),
            }],
        ),
        AssetValue::Schema(data) => {
            emit_definition_patch(&sanitize_definition_name(name), (**data).clone())
        }
        AssetValue::Unloaded => return None,
    };
    if body.trim().is_empty() {
        return None;
    }
    Some(asset_file_text(&asset_type_path(world, value)?, &body))
}

/// The type an asset's value holds, as its files name it.
fn asset_type_path(world: &World, value: &AssetValue) -> Option<String> {
    match value {
        AssetValue::Handle(handle) => {
            let registry = world.resource::<AppTypeRegistry>().read();
            Some(
                registry
                    .get(handle.type_id())?
                    .type_info()
                    .type_path()
                    .to_string(),
            )
        }
        AssetValue::Schema(data) => Some(data.type_path.clone()),
        AssetValue::Unloaded => None,
    }
}

/// Emit one named patch as a document of its own.
fn emit_definition_patch(name: &str, data: BsnStructData) -> String {
    let mut ast = jackdaw_bsn::SceneBsnAst::default();
    let name_patch = ast.world.spawn(BsnPatch::Name(name.to_string())).id();
    let type_patch = ast.world.spawn(BsnPatch::Struct(data)).id();
    let root = ast
        .world
        .spawn(BsnPatches(vec![name_patch, type_patch]))
        .id();
    ast.add_to_roots(root);
    jackdaw_bsn::emit_scene(&ast)
}

/// A fresh default value of a registered kind.
pub fn default_asset_value(world: &mut World, kind: &AssetKind) -> Option<AssetValue> {
    if kind.schema_backed() {
        return Some(AssetValue::Schema(Box::new(empty_patch(&kind.type_path))));
    }
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let registration = registry.get_with_type_path(&kind.type_path)?;
    let reflect_asset = registration.data::<ReflectAsset>()?;
    let value = registration.data::<ReflectDefault>()?.default();
    Some(AssetValue::Handle(
        reflect_asset.add(world, value.as_partial_reflect()),
    ))
}

fn registered_type_id(world: &World, type_path: &str) -> Option<std::any::TypeId> {
    let registry = world.resource::<AppTypeRegistry>().read();
    Some(registry.get_with_type_path(type_path)?.type_id())
}

// -- Reading and writing an asset's fields ----------------------------------

/// The value at an indexed path, if the index loaded one.
fn value_at(world: &World, path: &Path) -> Option<AssetValue> {
    world
        .get_resource::<AssetIndex>()?
        .get(path)
        .map(|entry| entry.value.clone())
}

/// The asset a definition-editing entity stands for, reflected out of its
/// store. `None` when the entity is not editing an asset of `type_path`.
pub fn definition_value<'w>(
    world: &'w World,
    entity: Entity,
    type_path: &str,
    registry: &bevy::reflect::TypeRegistry,
) -> Option<&'w dyn Reflect> {
    let edit = world.get::<DefinitionAssetEdit>(entity)?;
    if edit.type_path != type_path {
        return None;
    }
    let handle = world
        .get_resource::<AssetIndex>()?
        .get(&edit.path)?
        .value
        .handle()?;
    let reflect_asset = registry
        .get_with_type_path(type_path)?
        .data::<ReflectAsset>()?;
    reflect_asset.get(world, handle.id())
}

/// The file the open card is editing, as the index keys it.
pub fn open_definition_path(world: &World) -> Option<PathBuf> {
    let entity = world.get_resource::<OpenDefinition>()?.0?;
    Some(world.get::<DefinitionAssetEdit>(entity)?.path.clone())
}

/// The value the open card is editing.
pub fn open_definition_value(world: &World) -> Option<AssetValue> {
    value_at(world, &open_definition_path(world)?)
}

/// The schema the project reported for a kind's type, when the editor knows
/// the type no other way.
pub fn definition_schema(world: &World, kind: &str) -> Option<jackdaw_schema::TypeSchema> {
    let definition = definition_of_kind(world, kind)?;
    world
        .get_resource::<crate::project_types::ProjectTypes>()?
        .asset(&definition.type_path)
        .cloned()
}

/// The whole of a schema-backed asset as JSON: what its file authors, over
/// what its type defaults to.
pub fn schema_definition_json(world: &World, kind: &str, path: &Path) -> Option<serde_json::Value> {
    let schema = definition_schema(world, kind)?;
    let value = value_at(world, path)?;
    let data = value.schema()?;
    let types = world.get_resource::<crate::project_types::ProjectTypes>()?;
    Some(crate::schema_values::value_json(
        world, types, &schema, data,
    ))
}

/// Whether this entity is editing an asset of `type_path`.
fn edits_definition(world: &World, entity: Entity, type_path: &str) -> bool {
    world
        .get::<DefinitionAssetEdit>(entity)
        .is_some_and(|edit| edit.type_path == type_path)
}

/// The open asset's entity, while the inspector is showing it and it is
/// editing `type_path`. Selecting a scene entity again hands the same type's
/// edits back to the scene.
fn open_edit_of(world: &World, type_path: &str) -> Option<Entity> {
    let entity = world.get_resource::<OpenDefinition>()?.0?;
    let shown = world
        .get_resource::<crate::selection::Selection>()
        .is_some_and(|selection| selection.primary() == Some(entity));
    (shown && edits_definition(world, entity, type_path)).then_some(entity)
}

/// The baseline a drag started from, so the undo entry a drag commits restores
/// what the field held before the first tick rather than after the last one.
#[derive(Resource, Default)]
struct DefinitionEditSession {
    field: Option<(String, String)>,
    baseline: Option<serde_json::Value>,
}

fn field_as_json(
    world: &World,
    entity: Entity,
    type_path: &str,
    field_path: &str,
) -> Option<serde_json::Value> {
    let edit = world.get::<DefinitionAssetEdit>(entity)?;
    if edit.type_path != type_path {
        return None;
    }
    definition_field_json(world, &edit.path, type_path, field_path)
}

/// One field of the asset at a path, whichever way its value is held.
fn definition_field_json(
    world: &World,
    path: &Path,
    type_path: &str,
    field_path: &str,
) -> Option<serde_json::Value> {
    let kind = world.get_resource::<AssetIndex>()?.get(path)?.kind.clone();
    match value_at(world, path)? {
        AssetValue::Handle(handle) => asset_field_json(world, &handle, type_path, field_path),
        AssetValue::Schema(_) => {
            let whole = schema_definition_json(world, &kind, path)?;
            let steps = crate::schema_values::parse_path(field_path);
            crate::schema_values::json_at(&whole, &steps).cloned()
        }
        AssetValue::Unloaded => None,
    }
}

/// One field of the asset behind `handle`. A field naming an asset reports the
/// path it names, which is the only spelling that sets it again.
fn asset_field_json(
    world: &World,
    handle: &UntypedHandle,
    type_path: &str,
    field_path: &str,
) -> Option<serde_json::Value> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let reflect_asset = registry
        .get_with_type_path(type_path)?
        .data::<ReflectAsset>()?;
    let value = reflect_asset.get(world, handle.id())?;
    let field = if field_path.is_empty() {
        value.as_partial_reflect()
    } else {
        value.reflect_path(field_path).ok()?
    };
    let server = world.get_resource::<AssetServer>();
    let index = world.get_resource::<crate::asset_index::AssetIndex>();
    if let Some(path) = crate::typed_values::asset_path_json(&registry, server, index, field) {
        return Some(path);
    }
    crate::inspector::reflect_fields::reflect_to_json(field, &registry)
}

/// Write one field of the asset at a path, and mark whatever card is editing
/// it as having unsaved changes.
fn write_field(
    world: &mut World,
    path: &Path,
    type_path: &str,
    field_path: &str,
    json: &serde_json::Value,
) -> bool {
    let Some(value) = value_at(world, path) else {
        return false;
    };
    let written = match value {
        AssetValue::Handle(handle) => {
            write_asset_field(world, &handle, type_path, field_path, json)
        }
        AssetValue::Schema(_) => write_schema_field(world, path, field_path, json),
        AssetValue::Unloaded => false,
    };
    if written {
        mark_dirty(world, path);
    }
    written
}

fn write_asset_field(
    world: &mut World,
    handle: &UntypedHandle,
    type_path: &str,
    field_path: &str,
    json: &serde_json::Value,
) -> bool {
    let spelled = json
        .as_str()
        .and_then(|text| text_value_for_asset_field(world, handle, type_path, field_path, text));
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let Some(reflect_asset) = registry
        .get_with_type_path(type_path)
        .and_then(|registration| registration.data::<ReflectAsset>())
    else {
        return false;
    };
    let Some(value) = reflect_asset.get_mut(world, handle.id()) else {
        return false;
    };
    let field = if field_path.is_empty() {
        Some(value.as_partial_reflect_mut())
    } else {
        value.reflect_path_mut(field_path).ok()
    };
    let Some(field) = field else {
        return false;
    };
    match spelled {
        Some(spelled) => field.try_apply(spelled.as_ref()).is_ok(),
        None => {
            crate::commands::apply_json_to_reflect(field, json, &registry);
            true
        }
    }
}

/// The value a plain string stands for in one field of an asset: an asset path
/// or a colour, which no JSON spelling reaches.
fn text_value_for_asset_field(
    world: &World,
    handle: &UntypedHandle,
    type_path: &str,
    field_path: &str,
    text: &str,
) -> Option<Box<dyn bevy::reflect::PartialReflect>> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let reflect_asset = registry
        .get_with_type_path(type_path)?
        .data::<ReflectAsset>()?;
    let value = reflect_asset.get(world, handle.id())?;
    let field = if field_path.is_empty() {
        value.as_partial_reflect()
    } else {
        value.reflect_path(field_path).ok()?
    };
    let type_id = field.get_represented_type_info()?.type_id();
    let references = jackdaw_bsn::apply_reference_map(world);
    if crate::typed_values::takes_asset_path(&registry, type_id) && !references.contains_key(text) {
        report_missing_asset(text);
    }
    let server = world.get_resource::<AssetServer>();
    crate::typed_values::text_value_for_field(&registry, server, Some(&references), type_id, text)
}

/// Say when a path names nothing under the project's assets, without refusing
/// it: a file that arrives later loads on its own.
fn report_missing_asset(path: &str) {
    let file = path.split('#').next().unwrap_or(path);
    if file.is_empty() || file.starts_with('@') {
        return;
    }
    let Some(assets) = crate::project::open_project_assets_dir() else {
        return;
    };
    if !assets.join(file).exists() {
        warn!("{file} is not under this project's assets yet");
    }
}

/// Write one field of a value the editor knows only as schema. A field set
/// back to what its type defaults to stops being authored at all, so the file
/// keeps holding only what the asset changes.
fn write_schema_field(
    world: &mut World,
    path: &Path,
    field_path: &str,
    json: &serde_json::Value,
) -> bool {
    let Some(kind) = world
        .get_resource::<AssetIndex>()
        .and_then(|index| index.get(path))
        .map(|entry| entry.kind.clone())
    else {
        return false;
    };
    let Some(schema) = definition_schema(world, &kind) else {
        warn!("this project reported no schema for its {kind} files");
        return false;
    };
    let Some(mut data) = value_at(world, path)
        .as_ref()
        .and_then(AssetValue::schema)
        .cloned()
    else {
        return false;
    };
    if world
        .get_resource::<crate::project_types::ProjectTypes>()
        .is_none()
    {
        return false;
    }
    let written = world.resource_scope(|world, types: Mut<crate::project_types::ProjectTypes>| {
        let mut whole = crate::schema_values::value_json(world, &types, &schema, &data);
        let steps = crate::schema_values::parse_path(field_path);
        if steps.is_empty() {
            whole = json.clone();
        } else if !crate::schema_values::json_set(&mut whole, &steps, json.clone()) {
            return false;
        }
        let touched: Vec<String> = match steps.first() {
            None => schema
                .fields
                .iter()
                .map(|field| field.name.clone())
                .collect(),
            Some(crate::schema_values::Step::Field(name)) => vec![name.clone()],
            Some(crate::schema_values::Step::Index(_)) => return false,
        };
        for field_name in touched {
            let Some(field) = schema.fields.iter().find(|field| field.name == field_name) else {
                return false;
            };
            let Some(new) = whole.get(&field_name) else {
                continue;
            };
            if crate::schema_values::default_field_json(&schema, &field_name).as_ref() == Some(new)
            {
                crate::schema_values::set_authored(&mut data, &schema, &field_name, None);
                continue;
            }
            let Some(value) =
                crate::schema_values::bsn_for_json(world, &types, &field.type_path, new)
            else {
                return false;
            };
            crate::schema_values::set_authored(&mut data, &schema, &field_name, Some(value));
        }
        true
    });
    if written && let Some(entry) = world.resource_mut::<AssetIndex>().get_mut(path) {
        entry.value = AssetValue::Schema(Box::new(data));
    }
    written
}

/// The entity editing the file at this path, if one is open.
fn open_card_for(world: &mut World, path: &Path) -> Option<Entity> {
    let mut edits = world.query::<(Entity, &DefinitionAssetEdit)>();
    edits
        .iter(world)
        .find(|(_, edit)| edit.path == path)
        .map(|(entity, _)| entity)
}

/// Whether the card editing this file holds edits that are not on disk.
pub fn card_has_unsaved_edits(world: &World, path: &Path) -> bool {
    let Some(entity) = world
        .get_resource::<OpenDefinition>()
        .and_then(|open| open.0)
    else {
        return false;
    };
    world
        .get::<DefinitionAssetEdit>(entity)
        .is_some_and(|edit| edit.path == path && edit.dirty)
}

/// Close the card editing this file, for a file that has left the project.
pub fn close_card_for(world: &mut World, path: &Path) {
    let editing = world
        .get_resource::<OpenDefinition>()
        .and_then(|open| open.0)
        .and_then(|entity| world.get::<DefinitionAssetEdit>(entity))
        .is_some_and(|edit| edit.path == path);
    if editing {
        close_open_definition(world);
    }
}

/// Close the card when the kind it was opened under is no longer registered.
pub fn close_card_of_missing_kind(world: &mut World, kinds: &[String]) {
    let open_kind = world
        .get_resource::<OpenDefinition>()
        .and_then(|open| open.0)
        .and_then(|entity| world.get::<DefinitionAssetEdit>(entity))
        .map(|edit| edit.kind.clone());
    if let Some(kind) = open_kind
        && !kinds.contains(&kind)
    {
        close_open_definition(world);
    }
}

fn mark_dirty(world: &mut World, path: &Path) {
    let Some(entity) = open_card_for(world, path) else {
        return;
    };
    if let Some(mut edit) = world.get_mut::<DefinitionAssetEdit>(entity) {
        edit.dirty = true;
    }
}

/// Set one field of an asset, as one undo entry.
///
/// Keyed by the file rather than by the entity the asset was open on, so undo
/// still reaches it after the card has been closed and reopened.
pub struct SetDefinitionField {
    pub path: PathBuf,
    pub type_path: String,
    pub field_path: String,
    pub old_json: serde_json::Value,
    pub new_json: serde_json::Value,
    /// Whether applying this edit changes which rows the inspector shows, as
    /// an enum variant or a list length does.
    pub rebuilds_rows: bool,
}

impl SetDefinitionField {
    /// Write the value, reporting whether the asset took it.
    fn apply(&self, world: &mut World, json: &serde_json::Value) -> bool {
        if !write_field(world, &self.path, &self.type_path, &self.field_path, json) {
            return false;
        }
        let open = open_card_for(world, &self.path);
        if self.rebuilds_rows
            && let Some(open) = open
            && let Some(mut pending) =
                world.get_resource_mut::<crate::inspector::PendingInspectorRebuild>()
        {
            pending.0 = Some(open);
        }
        true
    }
}

impl EditorCommand for SetDefinitionField {
    fn execute(&mut self, world: &mut World) {
        let json = self.new_json.clone();
        self.apply(world, &json);
    }

    fn undo(&mut self, world: &mut World) {
        let json = self.old_json.clone();
        self.apply(world, &json);
    }

    fn description(&self) -> &str {
        "Set definition field"
    }
}

/// Commit a field edit to the open asset, pushing one undo entry.
///
/// Returns whether the edit landed on an asset; a caller whose edit is refused
/// here writes to the selection as usual. A value the asset will not take mints
/// no history, so undo does not walk back over a no-op.
pub(crate) fn commit_definition_field(
    world: &mut World,
    type_path: &str,
    field_path: &str,
    new_json: &serde_json::Value,
) -> bool {
    let Some(entity) = open_edit_of(world, type_path) else {
        return false;
    };
    let Some(path) = world
        .get::<DefinitionAssetEdit>(entity)
        .map(|edit| edit.path.clone())
    else {
        return false;
    };
    let Some(current) = field_as_json(world, entity, type_path, field_path) else {
        return false;
    };
    let old_json = take_baseline(world, type_path, field_path).unwrap_or(current);
    let rebuilds_rows = field_rebuilds_rows(world, entity, type_path, field_path);
    let command = SetDefinitionField {
        path,
        type_path: type_path.to_string(),
        field_path: field_path.to_string(),
        old_json,
        new_json: new_json.clone(),
        rebuilds_rows,
    };
    if !command.apply(world, new_json) {
        return false;
    }
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(command));
    true
}

/// The path a field of the open asset names, whichever way the asset is held.
pub(crate) fn asset_field_text(
    world: &World,
    entity: Entity,
    type_path: &str,
    field_path: &str,
) -> Option<String> {
    let json = field_as_json(world, entity, type_path, field_path)?;
    Some(json.as_str().unwrap_or_default().to_string())
}

/// The path a field of the asset behind a handle names.
pub(crate) fn handle_field_text(
    world: &World,
    handle: &UntypedHandle,
    field_path: &str,
) -> Option<String> {
    let type_path = handle_type_path(world, handle)?;
    let json = asset_field_json(world, handle, &type_path, field_path)?;
    Some(json.as_str().unwrap_or_default().to_string())
}

/// Set one field of the asset a handle points at, as one undo entry.
///
/// An asset with a file of its own is edited through that file, so undo reaches
/// it the way it reaches the open card. One the editor only holds in memory,
/// such as a material created and not yet saved, is written straight to its
/// store and mints no history, since there is no file to key an entry by.
pub(crate) fn commit_handle_field(
    world: &mut World,
    handle: &UntypedHandle,
    field_path: &str,
    new_json: &serde_json::Value,
) -> bool {
    let entry = world
        .get_resource::<AssetIndex>()
        .and_then(|index| index.by_handle(handle))
        .map(|entry| (entry.path.clone(), entry.type_path.clone()));
    let Some((path, type_path)) = entry else {
        let Some(type_path) = handle_type_path(world, handle) else {
            return false;
        };
        return write_asset_field(world, handle, &type_path, field_path, new_json);
    };
    let Some(old_json) = asset_field_json(world, handle, &type_path, field_path) else {
        return false;
    };
    let command = SetDefinitionField {
        path,
        type_path,
        field_path: field_path.to_string(),
        old_json,
        new_json: new_json.clone(),
        rebuilds_rows: false,
    };
    if !command.apply(world, new_json) {
        return false;
    }
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(command));
    true
}

/// The type a handle's asset store holds, as its files name it.
fn handle_type_path(world: &World, handle: &UntypedHandle) -> Option<String> {
    let registry = world.resource::<AppTypeRegistry>().read();
    Some(
        registry
            .get(handle.type_id())?
            .type_info()
            .type_path()
            .to_string(),
    )
}

/// Write a field of the open asset without an undo entry, for the ticks of a
/// drag.
pub(crate) fn preview_definition_field(
    world: &mut World,
    type_path: &str,
    field_path: &str,
    new_json: &serde_json::Value,
) -> bool {
    let Some(entity) = open_edit_of(world, type_path) else {
        return false;
    };
    let Some(path) = world
        .get::<DefinitionAssetEdit>(entity)
        .map(|edit| edit.path.clone())
    else {
        return false;
    };
    remember_baseline(world, entity, type_path, field_path);
    write_field(world, &path, type_path, field_path, new_json)
}

/// Record what the field held before a drag's first tick.
fn remember_baseline(world: &mut World, entity: Entity, type_path: &str, field_path: &str) {
    let field = (type_path.to_string(), field_path.to_string());
    if world
        .get_resource::<DefinitionEditSession>()
        .is_some_and(|session| session.field.as_ref() == Some(&field))
    {
        return;
    }
    let baseline = field_as_json(world, entity, type_path, field_path);
    let mut session = world.get_resource_or_init::<DefinitionEditSession>();
    session.field = Some(field);
    session.baseline = baseline;
}

/// Take the baseline a drag on this field left, if there is one.
fn take_baseline(
    world: &mut World,
    type_path: &str,
    field_path: &str,
) -> Option<serde_json::Value> {
    let mut session = world.get_resource_or_init::<DefinitionEditSession>();
    let matches = session
        .field
        .as_ref()
        .is_some_and(|(known_type, known_field)| {
            known_type == type_path && known_field == field_path
        });
    session.field = None;
    let baseline = session.baseline.take();
    matches.then_some(baseline).flatten()
}

/// Whether the field holds a value whose shape decides the rows shown for it.
fn field_rebuilds_rows(world: &World, entity: Entity, type_path: &str, field_path: &str) -> bool {
    if let Some(kind) = world
        .get::<DefinitionAssetEdit>(entity)
        .filter(|edit| is_schema_backed(world, &edit.path))
        .map(|edit| edit.kind.clone())
    {
        return schema_field_rebuilds_rows(world, &kind, field_path);
    }
    let registry = world.resource::<AppTypeRegistry>().read();
    let Some(value) = definition_value(world, entity, type_path, &registry) else {
        return false;
    };
    let field = if field_path.is_empty() {
        Some(value.as_partial_reflect())
    } else {
        value.reflect_path(field_path).ok()
    };
    field.is_some_and(|field| {
        matches!(
            field.reflect_ref(),
            ReflectRef::Enum(_) | ReflectRef::List(_) | ReflectRef::Array(_)
        )
    })
}

/// Whether the file at this path holds a value the editor knows only as the
/// project's schema.
fn is_schema_backed(world: &World, path: &Path) -> bool {
    world
        .get_resource::<AssetIndex>()
        .and_then(|index| index.get(path))
        .is_some_and(|entry| entry.value.schema().is_some())
}

/// The elements of a list field on a schema-backed asset, as the JSON a field
/// edit takes. `None` when the entity is editing something else.
pub(crate) fn schema_list_items(
    world: &World,
    entity: Entity,
    type_path: &str,
    field_path: &str,
) -> Option<Vec<serde_json::Value>> {
    let edit = world.get::<DefinitionAssetEdit>(entity)?;
    if edit.type_path != type_path || !is_schema_backed(world, &edit.path) {
        return None;
    }
    let held = definition_field_json(world, &edit.path, type_path, field_path)?;
    held.as_array().cloned()
}

/// A fresh element for a list field on a schema-backed asset, from what its
/// item type defaults to.
pub(crate) fn schema_default_list_item(
    world: &World,
    entity: Entity,
    type_path: &str,
    field_path: &str,
) -> Option<serde_json::Value> {
    let edit = world.get::<DefinitionAssetEdit>(entity)?;
    if edit.type_path != type_path || !is_schema_backed(world, &edit.path) {
        return None;
    }
    let types = world.get_resource::<crate::project_types::ProjectTypes>()?;
    let steps = crate::schema_values::parse_path(field_path);
    let field_type = crate::schema_values::field_type_path(types, type_path, &steps)?;
    let item_type = crate::schema_values::list_item_type_path(&field_type)?;
    if let Some(schema) = types.type_schema(item_type) {
        return crate::schema_values::type_default_json(schema);
    }
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let default = registry
        .get_with_type_path(item_type)?
        .data::<ReflectDefault>()?
        .default();
    crate::inspector::reflect_fields::reflect_to_json(default.as_partial_reflect(), &registry)
}

/// Whether a schema-backed field decides which rows are shown for it.
fn schema_field_rebuilds_rows(world: &World, kind: &str, field_path: &str) -> bool {
    let Some(definition) = definition_of_kind(world, kind) else {
        return false;
    };
    let Some(types) = world.get_resource::<crate::project_types::ProjectTypes>() else {
        return false;
    };
    let steps = crate::schema_values::parse_path(field_path);
    if steps.is_empty() {
        return true;
    }
    crate::schema_values::field_type_path(types, &definition.type_path, &steps)
        .is_some_and(|type_path| crate::schema_values::shapes_its_own_rows(types, &type_path))
}

// -- Opening, saving and creating -------------------------------------------

/// The kind whose type a file says it holds, read from the file itself.
pub fn kind_of_file(world: &World, path: &Path) -> Option<AssetKind> {
    let kinds = world.get_resource::<AssetKinds>()?;
    let AssetFileKind::Asset { type_path } = read_asset_kind(path, kinds) else {
        return None;
    };
    kinds.by_type_path(&type_path).cloned()
}

/// The entry for a file, reading and indexing it if the walk has not reached it
/// yet.
fn entry_for_file(world: &mut World, path: &Path) -> Option<AssetEntry> {
    let indexed = indexed_path(world, path)?;
    if crate::asset_index::is_catalog_file(&indexed) {
        return None;
    }
    if let Some(entry) = world
        .get_resource::<AssetIndex>()
        .and_then(|index| index.get(&indexed))
    {
        return Some(entry.clone());
    }
    let file = absolute_path(world, &indexed);
    let kind = kind_of_file(world, &file)?;
    let value = crate::asset_index::load_asset_value(world, &kind, &file)?;
    let mtime = std::fs::metadata(&file)
        .and_then(|meta| meta.modified())
        .ok()?;
    let entry = AssetEntry {
        path: indexed,
        kind: kind.kind.clone(),
        type_path: kind.type_path.clone(),
        file_kind: AssetFileKind::Asset {
            type_path: kind.type_path,
        },
        value,
        mtime,
    };
    world.resource_mut::<AssetIndex>().insert(entry.clone());
    Some(entry)
}

/// Load an asset file, or reuse the loaded one, and put it in the inspector.
pub fn open_definition_file(world: &mut World, path: &Path) -> bool {
    let Some(entry) = entry_for_file(world, path) else {
        return false;
    };
    let Some(kind) = definition_of_kind(world, &entry.kind) else {
        return false;
    };
    if matches!(entry.value, AssetValue::Unloaded) {
        warn!(
            "{} is opened by the panel that loads its kind",
            path.display()
        );
        return false;
    }
    show_definition(world, &kind, &entry.name(), entry.path);
    true
}

fn show_definition(world: &mut World, kind: &AssetKind, name: &str, path: PathBuf) {
    close_open_definition(world);
    let entity = world
        .spawn((
            Name::new(format!("{} ({})", name, kind.label)),
            DefinitionAssetEdit {
                kind: kind.kind.clone(),
                name: name.to_string(),
                type_path: kind.type_path.clone(),
                path,
                dirty: false,
            },
        ))
        .id();
    world.resource_mut::<OpenDefinition>().0 = Some(entity);
    crate::selection::select_only(world, entity);
}

/// Drop the editing entity for whatever asset was open. The asset itself stays
/// loaded and indexed.
pub fn close_open_definition(world: &mut World) {
    let Some(entity) = world.resource_mut::<OpenDefinition>().0.take() else {
        return;
    };
    if let Ok(entity_mut) = world.get_entity_mut(entity) {
        entity_mut.despawn();
    }
    if world
        .get_resource::<crate::selection::Selection>()
        .is_some_and(|selection| selection.entities.contains(&entity))
    {
        crate::selection::clear_selection_in_world(world);
    }
}

/// Write the open asset back to its file, reporting the name it saved under.
fn save_open_definition(world: &mut World, entity: Entity) -> Option<String> {
    let path = world
        .get::<DefinitionAssetEdit>(entity)
        .map(|edit| edit.path.clone())?;
    save_indexed_asset(world, &path)
}

/// Write the asset a file path names back to that file, without taking the
/// inspector off whatever it is showing.
fn save_definition_at(world: &mut World, path: &Path) -> Option<String> {
    let Some(entry) = entry_for_file(world, path) else {
        warn!("asset.save: {} is not an asset file", path.display());
        return None;
    };
    save_indexed_asset(world, &entry.path)
}

/// Write one indexed asset back to the file it came from.
fn save_indexed_asset(world: &mut World, path: &Path) -> Option<String> {
    let entry = world.get_resource::<AssetIndex>()?.get(path)?.clone();
    let name = entry.name();
    let file = absolute_path(world, path);
    if let Err(err) = write_asset_file(world, &name, &entry.value, &file) {
        warn!("asset.save: failed to write '{name}': {err}");
        return None;
    }
    crate::asset_index::note_written(world, &file);
    if let Some(open) = open_card_for(world, path)
        && let Some(mut edit) = world.get_mut::<DefinitionAssetEdit>(open)
    {
        edit.dirty = false;
    }
    Some(name)
}

// -- Operators --------------------------------------------------------------

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<AssetNewOp>()
        .register_operator::<AssetOpenOp>()
        .register_operator::<AssetSaveOp>()
        .register_operator::<AssetDeleteOp>()
        .register_operator::<AssetSetOp>()
        .register_operator::<AssetPickOp>()
        .register_operator::<AssetClearOp>()
        .register_operator::<AssetListOp>();
}

/// The kind saved materials list under, so a material is browsed, opened and
/// edited through the same index as any other asset.
pub const MATERIAL_KIND: &str = "material";

/// The kind an animation graph file holds.
pub const ANIMATION_GRAPH_KIND: &str = "animation_graph";

/// The kind a packed prefab holds.
pub const PREFAB_KIND: &str = "prefab";

/// The types the editor has compiled in.
fn compiled_kinds() -> [AssetKind; 3] {
    use jackdaw_api_internal::lucide_icons::Icon;
    [
        AssetKind::compiled(
            MATERIAL_KIND,
            "Material",
            "bevy_pbr::pbr_material::StandardMaterial",
        )
        .with_icon(Icon::Palette),
        AssetKind::compiled(
            ANIMATION_GRAPH_KIND,
            "Animation Graph",
            <jackdaw_animation_runtime::graph::AnimationGraphDef as bevy::reflect::TypePath>::type_path(),
        )
        .with_icon(Icon::Workflow),
        AssetKind::compiled(PREFAB_KIND, "Prefab", jackdaw_prefab::components::PREFAB_TYPE)
            .with_icon(Icon::Package),
    ]
}

pub(crate) fn plugin(app: &mut App) {
    {
        let mut kinds = app.world_mut().get_resource_or_init::<AssetKinds>();
        for kind in compiled_kinds() {
            kinds.register(kind);
        }
    }
    app.init_resource::<DefinitionEditSession>()
        .init_resource::<OpenDefinition>();
}

fn definition_of_kind(world: &World, kind: &str) -> Option<AssetKind> {
    world
        .get_resource::<AssetKinds>()
        .and_then(|types| types.by_kind(kind))
        .cloned()
}

/// Create an asset file of a registered type and open it.
#[operator(
    id = "asset.new",
    label = "New Asset",
    description = "Create an asset file of a registered type and open it in the inspector.",
    allows_undo = false,
    params(
        r#type(String, doc = "Kind of asset to create, as its type registered it."),
        name(
            String,
            doc = "Name to create it under. Defaults to the next free name."
        ),
        path(
            String,
            doc = "Folder to create it in, or the file to write. Defaults to the \
                   folder the browser is showing."
        )
    )
)]
pub fn asset_new(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let Some(kind) = params.as_str("type").map(str::to_owned) else {
        warn!("asset.new: no type given");
        return OperatorResult::Cancelled;
    };
    let name = params.as_str("name").map(str::to_owned);
    let path = params.as_str("path").map(PathBuf::from);
    commands.queue(move |world: &mut World| {
        let dir = path.as_ref().map(|path| resolve_project_path(world, path));
        new_definition(world, &kind, name.as_deref(), dir.as_deref());
    });
    OperatorResult::Finished
}

fn new_definition(world: &mut World, kind: &str, name: Option<&str>, dir: Option<&Path>) {
    let Some((definition, name, path)) = create_definition(world, kind, name, dir) else {
        return;
    };
    show_definition(world, &definition, &name, path);
    report_to_caller(world, format!("Created {kind} '{name}'"));
}

/// Write a fresh default value of a registered kind to its file and index it.
pub(crate) fn create_definition(
    world: &mut World,
    kind: &str,
    name: Option<&str>,
    dir: Option<&Path>,
) -> Option<(AssetKind, String, PathBuf)> {
    let Some(definition) = definition_of_kind(world, kind) else {
        warn!("asset.new: '{kind}' is not a registered asset type");
        return None;
    };
    let (dir, file) = match dir {
        Some(file) if names_a_file(file) => (
            file.parent().unwrap_or(file).to_path_buf(),
            Some(file.to_path_buf()),
        ),
        Some(dir) => (dir.to_path_buf(), None),
        None => {
            let Some(dir) = new_definition_dir(world) else {
                warn!("asset.new: no project is open");
                return None;
            };
            (dir, None)
        }
    };
    let asked_for = name.map(sanitize_definition_name);
    let named_by_path = file.as_deref().map(definition_name_of);
    let name = match named_by_path.or_else(|| asked_for.clone()) {
        Some(name) => sanitize_definition_name(&name),
        None => next_free_name(world, kind, &dir),
    };
    if asked_for.is_some_and(|asked| asked != name) {
        warn!("asset.new: the file asked for names this {kind} '{name}'");
    }
    if world
        .resource::<AssetIndex>()
        .of_kind(kind)
        .any(|entry| entry.name() == name)
    {
        warn!("asset.new: a {kind} named '{name}' already exists");
        return None;
    }
    let path = file.unwrap_or_else(|| definition_file_path(&dir, &name));
    if path.exists() {
        warn!("asset.new: {} is already there", path.display());
        return None;
    }
    let Some(value) = default_asset_value(world, &definition) else {
        warn!(
            "asset.new: {} has no registered default",
            definition.type_path
        );
        return None;
    };
    let path = match write_asset_file(world, &name, &value, &path) {
        Ok(path) => path,
        Err(err) => {
            warn!("asset.new: failed to write '{name}': {err}");
            return None;
        }
    };
    let Some(indexed) = crate::asset_index::index_written(world, &path, &definition, value) else {
        warn!(
            "asset.new: {} is outside this project's assets",
            path.display()
        );
        return None;
    };
    Some((definition, name, indexed))
}

/// Open an asset file in the inspector.
#[operator(
    id = "asset.open",
    label = "Open Asset",
    description = "Open an asset file in the inspector.",
    allows_undo = false,
    params(path(String, doc = "File to open, as a path under the project."))
)]
pub fn asset_open(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let Some(path) = params.as_str("path").map(PathBuf::from) else {
        warn!("asset.open: no path given");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let path = resolve_project_path(world, &path);
        if !open_definition_file(world, &path) {
            warn!("asset.open: {} is not an asset file", path.display());
        }
    });
    OperatorResult::Finished
}

/// Write an asset back to its file.
#[operator(
    id = "asset.save",
    label = "Save Asset",
    description = "Write the open asset back to its file.",
    allows_undo = false,
    params(path(String, doc = "Asset file to save. Defaults to the open asset."))
)]
pub fn asset_save(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let path = params.as_str("path").map(PathBuf::from);
    commands.queue(move |world: &mut World| {
        let saved = match &path {
            Some(path) => {
                let path = resolve_project_path(world, path);
                save_definition_at(world, &path)
            }
            None => {
                let Some(entity) = world.resource::<OpenDefinition>().0 else {
                    warn!("asset.save: no asset is open");
                    return;
                };
                save_open_definition(world, entity)
            }
        };
        if let Some(name) = saved {
            report_to_caller(world, format!("Saved '{name}'"));
        }
    });
    OperatorResult::Finished
}

/// Delete an asset file and forget what it held.
#[operator(
    id = "asset.delete",
    label = "Delete Asset",
    description = "Delete an asset file and drop it from this project.",
    allows_undo = false,
    params(path(String, doc = "Asset file to delete."))
)]
pub fn asset_delete(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let Some(path) = params.as_str("path").map(PathBuf::from) else {
        warn!("asset.delete: no path given");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let path = resolve_project_path(world, &path);
        delete_definition(world, &path);
    });
    OperatorResult::Finished
}

fn delete_definition(world: &mut World, path: &Path) {
    let Some(definition) = kind_of_file(world, path) else {
        warn!("asset.delete: {} is not an asset file", path.display());
        return;
    };
    if !definition.scanned() {
        warn!(
            "asset.delete: a {} is removed by whoever loads it",
            definition.kind
        );
        return;
    }
    match std::fs::remove_file(path) {
        Ok(()) => info!("Removed {}", path.display()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            warn!("asset.delete: failed to remove {}: {err}", path.display());
            return;
        }
    }
    let Some(indexed) = indexed_path(world, path) else {
        return;
    };
    world.resource_mut::<AssetIndex>().remove(&indexed);
    close_card_for(world, &indexed);
}

/// Set a field of the open asset, for edits driven from outside the inspector.
#[operator(
    id = "asset.set",
    label = "Set Asset Field",
    description = "Set a field of the open asset.",
    allows_undo = false,
    params(
        field(String, doc = "Field path on the asset, for example 'stack_size'."),
        value(
            String,
            doc = "Value to set: JSON, a plain scalar, an asset path for a slot \
                   that holds one, or a colour as 'r,g,b', a hex code or a name."
        )
    )
)]
pub fn asset_set(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let (Some(field), Some(value)) = (
        params.as_str("field").map(str::to_owned),
        params.as_str("value").map(str::to_owned),
    ) else {
        warn!("asset.set: both field and value are required");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        set_definition_field(world, &field, &value);
    });
    OperatorResult::Finished
}

fn set_definition_field(world: &mut World, field: &str, value: &str) {
    let Some(entity) = world.resource::<OpenDefinition>().0 else {
        warn!("asset.set: no asset is open");
        return;
    };
    let Some(type_path) = world
        .get::<DefinitionAssetEdit>(entity)
        .map(|edit| edit.type_path.clone())
    else {
        return;
    };
    if world
        .get_resource::<crate::selection::Selection>()
        .and_then(crate::selection::Selection::primary)
        != Some(entity)
    {
        crate::selection::select_only(world, entity);
    }
    let json = serde_json::from_str::<serde_json::Value>(value)
        .unwrap_or_else(|_| serde_json::Value::String(value.to_string()));
    if commit_definition_field(world, &type_path, field, &json) {
        report_to_caller(world, format!("Set {field}"));
    } else {
        warn!("asset.set: {type_path} did not take '{value}' for '{field}'");
    }
}

/// Drive the Pick beside an asset field, so the row's own choice can be made
/// without the pointer.
#[operator(
    id = "asset.pick",
    label = "Pick Asset",
    description = "Choose the file an asset field names, or put up the list to choose from.",
    allows_undo = false,
    params(
        field(
            String,
            doc = "Field path of the asset field, for example 'materials[0]'."
        ),
        value(
            String,
            doc = "File to assign, as a path under the project's assets. Left \
                   out, the picker opens on the field instead."
        )
    )
)]
pub(crate) fn asset_pick(
    params: In<OperatorParameters>,
    rows: Query<(Entity, &crate::inspector::asset_row::AssetFieldRow)>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(field) = params.as_str("field").map(str::to_owned) else {
        warn!("asset.pick: no field given");
        return OperatorResult::Cancelled;
    };
    let Some(row) = showing_asset_field(&rows, &field) else {
        warn!("asset.pick: no asset field is showing for '{field}'");
        return OperatorResult::Cancelled;
    };
    let value = params.as_str("value").map(str::to_owned);
    commands.queue(move |world: &mut World| match value {
        Some(value) => {
            if crate::inspector::asset_row::commit_asset_row(world, row, &value) {
                report_to_caller(world, format!("Set {field}"));
            } else {
                warn!("asset.pick: '{field}' did not take '{value}'");
            }
        }
        None => crate::inspector::asset_row::open_asset_picker(world, row),
    });
    OperatorResult::Finished
}

/// The row writing this field, for an operator that names a field rather than
/// clicking a row.
fn showing_asset_field(
    rows: &Query<(Entity, &crate::inspector::asset_row::AssetFieldRow)>,
    field: &str,
) -> Option<Entity> {
    rows.iter()
        .find(|(_, row)| row.field_path == field)
        .map(|(entity, _)| entity)
}

/// Leave an asset field naming nothing.
#[operator(
    id = "asset.clear",
    label = "Clear Asset",
    description = "Leave an asset field naming no file.",
    allows_undo = false,
    params(field(String, doc = "Field path of the asset field to clear."))
)]
pub(crate) fn asset_clear(
    params: In<OperatorParameters>,
    rows: Query<(Entity, &crate::inspector::asset_row::AssetFieldRow)>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(field) = params.as_str("field").map(str::to_owned) else {
        warn!("asset.clear: no field given");
        return OperatorResult::Cancelled;
    };
    let Some(row) = showing_asset_field(&rows, &field) else {
        warn!("asset.clear: no asset field is showing for '{field}'");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        if crate::inspector::asset_row::commit_asset_row(world, row, "") {
            report_to_caller(world, format!("Cleared {field}"));
        }
    });
    OperatorResult::Finished
}

/// Report the files of a kind this project holds.
#[operator(
    id = "asset.list",
    label = "List Assets",
    description = "Report the files of a registered asset type this project holds.",
    allows_undo = false,
    params(r#type(String, doc = "Kind of asset to list."))
)]
pub fn asset_list(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let Some(kind) = params.as_str("type").map(str::to_owned) else {
        warn!("asset.list: no type given");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        if definition_of_kind(world, &kind).is_none() {
            warn!("asset.list: '{kind}' is not a registered asset type");
            return;
        }
        let paths = world.resource::<AssetIndex>().paths_of_kind(&kind);
        report_to_caller(world, format!("{kind}: {}", paths.join(", ")));
    });
    OperatorResult::Finished
}

/// Accept both a path under the project and one relative to its assets
/// directory, so a caller can pass what the asset browser lists.
fn resolve_project_path(world: &World, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let Some(project) = world.get_resource::<ProjectRoot>() else {
        return path.to_path_buf();
    };
    let under_root = project.root.join(path);
    if under_root.exists() {
        return under_root;
    }
    project.assets_dir().join(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_names_what_it_holds_by_the_stem_before_its_first_dot() {
        for (file, expected) in [
            ("torch.item.bsn", "torch"),
            ("torch.bsn", "torch"),
            ("torch", "torch"),
            (".torch.item.bsn", "torch"),
        ] {
            assert_eq!(
                definition_name_of(Path::new(file)),
                expected,
                "{file} names an asset"
            );
        }
    }

    #[test]
    fn the_root_name_a_file_spells_is_read_back_however_it_is_written() {
        assert_eq!(
            root_name_of("#torch\njackdaw::Item {\n}\n").as_deref(),
            Some("torch")
        );
        assert_eq!(
            root_name_of("#\"torch.item\"\njackdaw::Item {\n}\n").as_deref(),
            Some("torch.item"),
            "a quoted name is the name, without its quotes"
        );
        assert_eq!(
            root_name_of("jackdaw::Item {\n}\n"),
            None,
            "a root that names nothing leaves the name to the caller"
        );
    }

    #[test]
    fn names_sanitize_to_one_file_each() {
        assert_eq!(sanitize_definition_name("torch"), "torch");
        assert_eq!(sanitize_definition_name("a/b"), "a_b");
        assert_eq!(sanitize_definition_name("../escape"), ".._escape");
        assert_eq!(sanitize_definition_name("  "), "definition");
    }
}
