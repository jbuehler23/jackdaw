//! Design-time binding preview: the Preview Context panel.
//!
//! An authored binding reads game state that does not exist in the editor.
//! Preview supplies it: one scratch entity stands in for the game's subject,
//! the open UI scene's root points at it with [`BindContext`], and
//! `jackdaw_bind`'s evaluator runs in the editor for as long as the toggle is
//! on. Scrubbing a field on the scratch entity moves the real widgets in the
//! 2D viewport, with no build and no Play session.
//!
//! # Nothing here is authored content
//!
//! The scratch entity carries [`EditorEntity`] and is never registered in the
//! scene document, so it cannot save, undo, or appear in the outliner. The
//! same holds for the `BindContext` this inserts on the scene root: the
//! document is maintained by explicit patches, and preview writes none, so a
//! save taken mid-session is byte-identical to one taken with preview off.
//!
//! Evaluating also writes to the authored widgets themselves (a `Node`'s
//! width, a `Text`, a `Visibility`). Those writes are live ECS state, not
//! document state, but leaving them behind would show the user a scene they
//! did not author, so every write target is snapshotted when the session
//! starts and restored when it ends.
//!
//! # Bound properties are read-only during a session
//!
//! While the evaluator owns a property, the user cannot author it: a drag or
//! an inspector commit would race a value that is rewritten every frame, and
//! whichever landed last would be baked into the document. So the session
//! publishes its write targets ([`PreviewWriteTargets`]) and the two
//! document-writing edit paths, [`crate::commands::push_layout_edit`] for
//! stage gestures and `crate::commands::field_edit_commit` for inspector
//! fields, refuse an edit that lands on one, with [`PREVIEW_EDIT_REFUSED`] in
//! the log. The set is empty whenever preview is off, so the check costs a
//! hash lookup.
//!
//! Play-in-editor projection writes the same components from the running
//! game. Nothing here arbitrates between the two: run a projection and a
//! preview at once and both write the same components, with the last writer
//! winning.
//!
//! # What can be previewed
//!
//! `jackdaw_bind` resolves a component read against the context entity's real
//! ECS components, through the editor's type registry. So a binding can be
//! previewed exactly when the editor links the Rust type it reads. A project
//! component the editor knows only as extracted schema ([`ProjectTypes`]) has
//! no Rust type here and cannot be constructed as a real component at all;
//! those rows render disabled, and say that previewing them needs the game
//! running.
//!
//! A `Res(Type).field` read is previewed too, which no context entity can
//! carry: the session stands the resource up in the editor world for as long
//! as it runs, and takes it back out at the end. A resource the editor was
//! already holding is left alone, being real editor state, and its rows say so
//! rather than offering a scrub that would move it.

use std::any::TypeId;

use bevy::ecs::reflect::ReflectComponent;
use bevy::feathers::controls::FeathersCheckbox;
use bevy::feathers::theme::ThemedText;
use bevy::feathers::tokens as feathers_tokens;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::reflect::{
    GetPath, PartialReflect, ReflectRef, TypeInfo, TypeRegistration, TypeRegistry,
};
use bevy::ui::{Checked, InteractionDisabled};
use bevy::ui_widgets::ValueChange;
use jackdaw_bind::{BindContext, BindPath, Binding, Bindings, ParsedPath, ResolvedBindings};
use jackdaw_feathers::field_row::{FieldRowProps, spawn_field_row};
use jackdaw_feathers::number_input::{
    NumberInputPrecision, ScrubNumberInput, ScrubNumberInputValue,
};
use jackdaw_feathers::text_edit::{TextEditCommitEvent, TextEditProps, text_edit};
use jackdaw_feathers::tokens;
use jackdaw_feathers::tooltip::Tooltip;

use crate::EditorEntity;
use crate::project_types::ProjectTypes;

/// Catalog id of the preview dock window.
pub const PREVIEW_CONTEXT_WINDOW_ID: &str = "jackdaw.preview_context";

/// Name the scratch subject carries, so a developer reading the world dump
/// can tell it from authored content at a glance.
pub const PREVIEW_SUBJECT_NAME: &str = "Preview Subject";

/// Why a schema-only row cannot be scrubbed.
const SCHEMA_ONLY_NOTE: &str = "preview needs the game running (PIE)";

/// Why a referenced type resolved to nothing at all.
const UNKNOWN_NOTE: &str = "no type of this name is registered";

/// Why a resource the editor itself holds is left alone.
const EDITOR_OWNED_NOTE: &str =
    "the editor holds this resource; scrubbing it would move the editor";

/// Why a linked type the editor cannot build a value of has no rows.
const UNCONSTRUCTIBLE_NOTE: &str = "the editor cannot build a value of this type";

/// Why a linked type the editor built has no rows anyway.
const NO_FIELDS_NOTE: &str = "this type has no fields to scrub";

/// What the editor says when it refuses an authored edit to a property the
/// running preview is driving.
pub const PREVIEW_EDIT_REFUSED: &str = "stop preview to edit a bound property";

/// The components a live session's evaluator writes, as (entity, type).
///
/// Empty whenever preview is off, which is what makes
/// [`preview_writes_type_path`] cheap enough to sit on the editor's edit
/// paths.
#[derive(Resource, Default)]
pub struct PreviewWriteTargets(bevy::platform::collections::HashSet<(Entity, TypeId)>);

impl PreviewWriteTargets {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether the running preview owns `type_id` on `entity`.
    pub fn contains(&self, entity: Entity, type_id: TypeId) -> bool {
        !self.0.is_empty() && self.0.contains(&(entity, type_id))
    }
}

/// Whether a live preview session is writing `type_id` on `entity`, so an
/// authored edit to it has to be refused.
pub fn preview_writes(world: &World, entity: Entity, type_id: TypeId) -> bool {
    world
        .get_resource::<PreviewWriteTargets>()
        .is_some_and(|targets| targets.contains(entity, type_id))
}

/// [`preview_writes`] by reflect path, for callers that hold a type path
/// rather than a `TypeId`.
pub fn preview_writes_type_path(world: &World, entity: Entity, type_path: &str) -> bool {
    let Some(targets) = world.get_resource::<PreviewWriteTargets>() else {
        return false;
    };
    if targets.is_empty() {
        return false;
    }
    let Some(registry) = world.get_resource::<AppTypeRegistry>() else {
        return false;
    };
    let registry = registry.read();
    registry
        .get_with_type_path(type_path)
        .or_else(|| registry.get_with_short_type_path(type_path))
        .is_some_and(|registration| targets.contains(entity, registration.type_id()))
}

/// The live preview session.
///
/// `subject` is the scratch entity, `root` the UI scene root carrying the
/// editor's `BindContext`. Both are `None` while preview is off, and
/// `subject` is also `None` when preview is on but the document holds no UI
/// scene to point anywhere.
#[derive(Resource, Default)]
pub struct PreviewSession {
    on: bool,
    subject: Option<Entity>,
    root: Option<Entity>,
    /// The types currently attached, in panel order.
    subjects: Vec<PreviewSubject>,
    /// Authored component state the evaluator overwrites, restored on stop.
    restore: Vec<WriteTarget>,
    /// The resources this session stood up in the editor world, as (type, the
    /// entity it was put on), to take back out when it ends. Only ever types
    /// the editor did not already hold. The entity is recorded rather than
    /// looked up again at teardown: the world's resource entity moving means
    /// something else claimed the resource, which the session must not drop.
    resources: Vec<(TypeId, Entity)>,
}

impl PreviewSession {
    /// Whether the toggle is on.
    pub fn is_on(&self) -> bool {
        self.on
    }

    /// The scratch entity this session's bindings read, if it has one.
    pub fn subject(&self) -> Option<Entity> {
        self.subject
    }
}

/// One authored component the evaluator may overwrite, as it stood before
/// the session began. `value` is `None` for a component the entity did not
/// have (a `Checked` marker a `Value` binding may add).
struct WriteTarget {
    entity: Entity,
    type_id: TypeId,
    value: Option<Box<dyn PartialReflect>>,
}

/// Whether a referenced type can back a real component in the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewAvailability {
    /// The editor links the Rust type: it is on the scratch entity and its
    /// fields are scrubbable.
    Native,
    /// The editor knows the type only as extracted schema, so it has no Rust
    /// type to construct. Rows render disabled.
    SchemaOnly,
    /// A resource the editor holds for its own purposes. Preview will not
    /// stand in for one: a scrub would move real editor state.
    EditorOwned,
    /// Neither the registry nor the project schema knows this path.
    Unknown,
}

/// The control one field asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewFieldKind {
    Number,
    Bool,
    Text,
    /// `Vec2`: one row of X/Y inputs.
    Vector2,
    /// `Vec3`: one row of X/Y/Z inputs.
    Vector3,
    /// `Vec4` or `Quat`: one row of X/Y/Z/W inputs.
    Vector4,
    /// A shape the panel has no scrubber for; shown read-only.
    Unsupported,
}

impl PreviewFieldKind {
    /// The axis suffixes this kind scrubs, empty for a plain scalar.
    fn axes(self) -> &'static [&'static str] {
        match self {
            Self::Vector2 => &["x", "y"],
            Self::Vector3 => &["x", "y", "z"],
            Self::Vector4 => &["x", "y", "z", "w"],
            _ => &[],
        }
    }
}

/// One scrubbable field of one previewed component.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewFieldRow {
    /// What the row's label reads, which for a tuple element is its index.
    pub name: String,
    /// The reflect path from the component to this field. The same string as
    /// `name` for a named field; a tuple element spells its leading dot.
    pub path: String,
    pub kind: PreviewFieldKind,
}

/// Whether a previewed value lives on the scratch subject or in the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewSource {
    /// `Type.field`: a component of the scratch entity.
    Component,
    /// `Res(Type).field`: a resource, which no context entity can carry, so
    /// the session stands one up in the editor world instead.
    Resource,
}

/// One type the open scene's bindings read, as the panel lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewSubject {
    /// The full reflect path where one is known, otherwise the path the
    /// binding wrote.
    pub type_path: String,
    /// The trailing segment, which is what the section header shows.
    pub short_name: String,
    pub source: PreviewSource,
    pub availability: PreviewAvailability,
    /// Why the rows are disabled; empty for a previewable type.
    pub note: String,
    pub fields: Vec<PreviewFieldRow>,
}

/// Which field of which previewed component a control writes.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct PreviewField {
    pub type_path: String,
    pub field: String,
}

impl PreviewField {
    pub fn new(type_path: impl Into<String>, field: impl Into<String>) -> Self {
        Self {
            type_path: type_path.into(),
            field: field.into(),
        }
    }
}

/// A value a scrub row commits to the scratch entity.
#[derive(Clone, Debug, PartialEq)]
pub enum PreviewValue {
    Number(f64),
    Bool(bool),
    Text(String),
}

/// Why a scratch write did not land.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewError {
    /// No preview session, so there is nothing to write to.
    NotPreviewing,
    /// The named type is not on the scratch entity (schema-only, or a stale
    /// row from before the bindings changed).
    NoSuchComponent(String),
    /// The type is there but the field is not, or holds a shape no control
    /// writes.
    NoSuchField(String),
    /// The path names a resource the editor holds for itself, which preview
    /// stands in for nothing and never writes.
    EditorOwned(String),
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotPreviewing => formatter.write_str("preview is not running"),
            Self::NoSuchComponent(path) => write!(formatter, "`{path}` is not on the subject"),
            Self::NoSuchField(field) => write!(formatter, "no writable field `{field}`"),
            Self::EditorOwned(path) => write!(formatter, "`{path}` is the editor's own resource"),
        }
    }
}

impl std::error::Error for PreviewError {}

pub struct PreviewContextPlugin;

impl Plugin for PreviewContextPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PreviewSession>()
            .init_resource::<PreviewWriteTargets>()
            .add_systems(
                PostUpdate,
                (
                    resync_preview_subject.run_if(preview_needs_resync),
                    refresh_preview_panels.run_if(preview_panels_are_stale),
                )
                    .chain()
                    .before(jackdaw_bind::BindEvaluationSystems),
            )
            .add_observer(on_preview_toggle)
            .add_observer(on_scrub_commit)
            .add_observer(on_checkbox_commit)
            .add_observer(on_text_commit);
    }
}

/// Run condition for `jackdaw_bind`'s evaluator in the editor: only while a
/// session actually has a subject to read.
pub fn preview_is_evaluating(session: Res<PreviewSession>) -> bool {
    session.on && session.subject.is_some()
}

/// Whether the toggle is on, for callers holding a world rather than a
/// resource.
pub fn preview_is_running(world: &World) -> bool {
    world
        .get_resource::<PreviewSession>()
        .is_some_and(PreviewSession::is_on)
}

// ---------------------------------------------------------------------------
// Session lifecycle
// ---------------------------------------------------------------------------

/// Start or stop preview.
///
/// Never panics on user state: a document with no UI scene leaves the toggle
/// on with nothing attached, and the panel says so.
pub fn set_preview(world: &mut World, on: bool) {
    if !on {
        stop_preview(world);
        return;
    }
    world.resource_mut::<PreviewSession>().on = true;
    sync_subject(world);
}

fn stop_preview(world: &mut World) {
    let (subject, root, restore) = {
        let mut session = world.resource_mut::<PreviewSession>();
        session.on = false;
        session.subjects.clear();
        (
            session.subject.take(),
            session.root.take(),
            std::mem::take(&mut session.restore),
        )
    };
    world.resource_mut::<PreviewWriteTargets>().0.clear();
    release_stand_in_resources(world);
    detach_context(world, root);
    if let Some(subject) = subject
        && let Ok(entity) = world.get_entity_mut(subject)
    {
        entity.despawn();
    }
    restore_write_targets(world, restore);
    clear_resolved_bindings(world);
}

/// Drop the lookups the evaluator built while the session ran.
///
/// `ResolvedBindings` holds entity ids, among them the scratch subject this
/// session is about to despawn, and the editor stops running the evaluator
/// with the session, so nothing would revisit them. Clearing them here leaves
/// an authored widget with no derived state on it between sessions, and the
/// next session resolves from scratch.
fn clear_resolved_bindings(world: &mut World) {
    let bound: Vec<Entity> = world
        .query_filtered::<Entity, With<ResolvedBindings>>()
        .iter(world)
        .collect();
    for entity in bound {
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.remove::<ResolvedBindings>();
        }
    }
}

/// Take the editor's `BindContext` off a root it may or may not still have.
/// A root that was despawned with its scene needs nothing; one that outlived
/// the session must not be left pointing at a dead subject.
fn detach_context(world: &mut World, root: Option<Entity>) {
    if let Some(root) = root
        && let Ok(mut entity) = world.get_entity_mut(root)
    {
        entity.remove::<BindContext>();
    }
}

/// Give up the subject when there is no scene to point at (the document was
/// closed, or its scene despawned mid-session).
///
/// The widgets can outlive the root that stranded them (a scene parented under
/// something else is still in the world) and they are still holding what the
/// evaluator put there. So this restores before it disarms: dropping
/// [`PreviewWriteTargets`] is what stops refusing authored edits to those
/// properties, and an edit must never land on a value the session wrote.
fn drop_subject(world: &mut World) {
    {
        let session = world.resource::<PreviewSession>();
        if session.subject.is_none()
            && session.root.is_none()
            && session.subjects.is_empty()
            && session.restore.is_empty()
            && session.resources.is_empty()
        {
            return;
        }
    }
    let (orphan, root, restore) = {
        let mut session = world.resource_mut::<PreviewSession>();
        session.subjects.clear();
        (
            session.subject.take(),
            session.root.take(),
            std::mem::take(&mut session.restore),
        )
    };
    world.resource_mut::<PreviewWriteTargets>().0.clear();
    release_stand_in_resources(world);
    detach_context(world, root);
    if let Some(orphan) = orphan
        && let Ok(entity) = world.get_entity_mut(orphan)
    {
        entity.despawn();
    }
    restore_write_targets(world, restore);
    clear_resolved_bindings(world);
}

/// Bring the session in line with the open scene: the scene's current root,
/// and the referenced types on the scratch entity.
///
/// All three halves can go stale on their own. The scene's root changes on a
/// tab switch, a structural undo, or a prefab reload, leaving the recorded
/// root despawned or not the one being edited while the current root carries
/// no context; the referenced types change whenever a binding is authored. The
/// scratch entity itself survives both: types are attached and detached in
/// place, so a value the user scrubbed is still there after they add a binding.
///
/// The third is the set of properties the evaluator owns, and it moves without
/// either of the others: a binding added to a widget whose reads the panel
/// already lists, or a write repointed at another component, leaves the
/// read-derived list identical. That set has to be compared in its own right,
/// or a write target goes unsnapshotted and unguarded and the evaluator
/// overwrites an authored value with nothing recorded to put back.
fn sync_subject(world: &mut World) {
    let Some(root) = crate::ui_palette::ui_scene_root(world) else {
        drop_subject(world);
        return;
    };
    let previous_root = world.resource::<PreviewSession>().root;
    let root_moved = previous_root != Some(root);
    let subjects = referenced_subjects(world, root);
    let subject_alive = world
        .resource::<PreviewSession>()
        .subject
        .is_some_and(|entity| world.get_entity(entity).is_ok());
    let written: bevy::platform::collections::HashSet<(Entity, TypeId)> =
        write_target_ids(world, root).into_iter().collect();
    let targets_moved = world.resource::<PreviewWriteTargets>().0 != written;
    if !root_moved
        && subject_alive
        && !targets_moved
        && world.resource::<PreviewSession>().subjects == subjects
    {
        return;
    }

    if root_moved {
        // A root that outlived the switch would otherwise keep a `BindContext`
        // nothing maintains.
        detach_context(world, previous_root);
    }

    let subject = match world
        .resource::<PreviewSession>()
        .subject
        .filter(|_| subject_alive)
    {
        Some(subject) => subject,
        None => world
            .spawn((EditorEntity, Name::new(PREVIEW_SUBJECT_NAME)))
            .id(),
    };
    attach_referenced_types(world, subject, &subjects);
    attach_referenced_resources(world, &subjects);

    // A property the bindings stopped naming is handed back here, not at the
    // end of the session. The guard swap below stops refusing authored edits
    // to it, so from this moment it is the user's again; a snapshot held until
    // the session ended would be written over whatever they did with it,
    // leaving the document holding one value and the component another.
    let departed: Vec<WriteTarget> = {
        let mut session = world.resource_mut::<PreviewSession>();
        let (kept, departed): (Vec<WriteTarget>, Vec<WriteTarget>) =
            std::mem::take(&mut session.restore)
                .into_iter()
                .partition(|held| written.contains(&(held.entity, held.type_id)));
        session.restore = kept;
        departed
    };
    restore_write_targets(world, departed);

    // Snapshot before the first evaluation, so the restore holds what the user
    // authored rather than what a previous frame's binding wrote. A widget
    // bound later in the session adds its own entry; one already recorded
    // keeps the value it had when the session began.
    let unseen: Vec<(Entity, TypeId)> = {
        let session = world.resource::<PreviewSession>();
        written
            .iter()
            .copied()
            .filter(|(entity, type_id)| {
                !session
                    .restore
                    .iter()
                    .any(|held| held.entity == *entity && held.type_id == *type_id)
            })
            .collect()
    };
    let fresh = snapshot_targets(world, &unseen);
    let mut session = world.resource_mut::<PreviewSession>();
    session.subject = Some(subject);
    session.root = Some(root);
    session.subjects = subjects;
    session.restore.extend(fresh);
    world.resource_mut::<PreviewWriteTargets>().0 = written;
    world.entity_mut(root).insert(BindContext(subject));
}

/// Attach every natively-known referenced type the scratch entity is missing,
/// and drop the ones nothing reads any more. Types already attached keep the
/// values the user scrubbed into them.
fn attach_referenced_types(world: &mut World, subject: Entity, subjects: &[PreviewSubject]) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let wanted: Vec<TypeId> = {
        let registry = registry.read();
        subjects
            .iter()
            .filter(|entry| {
                entry.availability == PreviewAvailability::Native
                    && entry.source == PreviewSource::Component
            })
            .filter_map(|entry| {
                registry
                    .get_with_type_path(&entry.type_path)
                    .map(TypeRegistration::type_id)
            })
            .collect()
    };

    let held: Vec<TypeId> = world
        .get_entity(subject)
        .map(|entity| {
            entity
                .archetype()
                .components()
                .iter()
                .filter_map(|id| world.components().get_info(*id))
                .filter_map(bevy::ecs::component::ComponentInfo::type_id)
                .collect()
        })
        .unwrap_or_default();

    let registry = registry.read();
    for type_id in &wanted {
        if held.contains(type_id) {
            continue;
        }
        let Some(reflect_component) = registry
            .get(*type_id)
            .and_then(|registration| registration.data::<ReflectComponent>())
        else {
            continue;
        };
        let Some(value) = crate::reflect_default::build_reflective_default(*type_id, &registry)
        else {
            warn!("preview cannot default-construct {type_id:?}");
            continue;
        };
        reflect_component.insert(
            &mut world.entity_mut(subject),
            value.as_partial_reflect(),
            &registry,
        );
    }
    for type_id in held {
        // `Name` and the editor marker are the scratch entity's own; only
        // previewed types come and go.
        if wanted.contains(&type_id)
            || type_id == TypeId::of::<Name>()
            || type_id == TypeId::of::<EditorEntity>()
        {
            continue;
        }
        if let Some(reflect_component) = registry
            .get(type_id)
            .and_then(|registration| registration.data::<ReflectComponent>())
            && let Ok(mut entity) = world.get_entity_mut(subject)
        {
            reflect_component.remove(&mut entity);
        }
    }
}

/// Stand up a resource for every natively-known resource read the scene makes,
/// and take back down the ones nothing reads any more.
///
/// Only ever a type the editor was not already holding: `mark_editor_owned`
/// has demoted the rest, so nothing here can overwrite editor state.
fn attach_referenced_resources(world: &mut World, subjects: &[PreviewSubject]) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let wanted: Vec<(TypeId, String)> = {
        let registry = registry.read();
        subjects
            .iter()
            .filter(|entry| {
                entry.availability == PreviewAvailability::Native
                    && entry.source == PreviewSource::Resource
            })
            .filter_map(|entry| {
                registry
                    .get_with_type_path(&entry.type_path)
                    .map(|registration| (registration.type_id(), entry.type_path.clone()))
            })
            .collect()
    };
    let held = world.resource::<PreviewSession>().resources.clone();
    for (type_id, entity) in held {
        if wanted.iter().any(|(id, _)| *id == type_id) {
            continue;
        }
        remove_stand_in_resource(world, type_id, entity);
        world
            .resource_mut::<PreviewSession>()
            .resources
            .retain(|(id, _)| *id != type_id);
    }
    for (type_id, type_path) in wanted {
        if world
            .resource::<PreviewSession>()
            .resources
            .iter()
            .any(|(id, _)| *id == type_id)
        {
            continue;
        }
        let built = {
            let registry = registry.read();
            let reflect_component = registry
                .get(type_id)
                .and_then(|registration| registration.data::<ReflectComponent>())
                .cloned();
            crate::reflect_default::build_reflective_default(type_id, &registry)
                .zip(reflect_component)
        };
        let Some((value, reflect_component)) = built else {
            warn!("preview cannot default-construct resource `{type_path}`");
            continue;
        };
        // Resources are entity-backed here, and `IsResource` is a required
        // component of every one of them, so putting the component on a fresh
        // entity is what makes the world hold the resource. The marker keeps
        // that entity out of the outliner and out of the document.
        let entity = world.spawn(EditorEntity).id();
        {
            let registry = registry.read();
            reflect_component.insert(
                &mut world.entity_mut(entity),
                value.as_partial_reflect(),
                &registry,
            );
        }
        world
            .resource_mut::<PreviewSession>()
            .resources
            .push((type_id, entity));
    }
}

/// The entity a resource lives on, or `None` when the world does not hold one.
fn resource_entity(world: &World, type_id: TypeId) -> Option<Entity> {
    let component_id = world.components().get_id(type_id)?;
    world.resource_entities().get(component_id)
}

/// Take one stand-in resource back out of the editor world.
///
/// `entity` is the one the session put the resource on, and the only entity
/// this touches. If the world's resource entity is a different one, something
/// else stood the resource up after the session did and that value is not the
/// session's to drop; the stand-in entity still goes.
///
/// When `entity` is the resource entity, `IsResource` comes off first: its
/// discard hook clears the world's resource cache and queues the value
/// component's own removal, and a cache still naming this entity would make the
/// next session's insert land on top of a resource bevy thinks is already
/// there. Only once that has flushed is the entity an ordinary one that can be
/// despawned; a despawn while it is still the resource entity queues a removal
/// against an entity that is gone by the time the command runs.
fn remove_stand_in_resource(world: &mut World, type_id: TypeId, entity: Entity) {
    if resource_entity(world, type_id) == Some(entity) {
        if let Ok(mut held) = world.get_entity_mut(entity) {
            held.remove::<bevy::ecs::resource::IsResource>();
        }
        world.flush();
    }
    if let Ok(held) = world.get_entity_mut(entity) {
        held.despawn();
    }
}

/// Drop every resource the session stood up, so the editor is left holding
/// exactly what it held before preview started.
fn release_stand_in_resources(world: &mut World) {
    let held = std::mem::take(&mut world.resource_mut::<PreviewSession>().resources);
    for (type_id, entity) in held {
        remove_stand_in_resource(world, type_id, entity);
    }
}

/// Re-point the session at the scene as it currently stands.
///
/// This has to happen before the evaluator runs, not merely earlier in the
/// frame: the evaluator reads the subject and the write targets this
/// establishes, so a frame that evaluated first would drive the scene from a
/// stale subject and would write properties the session has no baseline for,
/// which is what puts a previewed value somewhere it can be saved from. It
/// runs in `PostUpdate` ahead of [`jackdaw_bind::BindEvaluationSystems`] so
/// that ordering is a constraint the schedule enforces rather than a
/// consequence of which schedule each half happens to sit in.
fn resync_preview_subject(world: &mut World) {
    if !world.resource::<PreviewSession>().on {
        return;
    }
    sync_subject(world);
}

/// Whether [`resync_preview_subject`] has anything to do this frame.
///
/// The walk itself is the expensive part, so nothing gets walked on a quiet
/// frame: only a session whose subject died, whose scene root moved, or whose
/// scene had a `Bindings` added, changed, or removed is worth re-reading.
fn preview_needs_resync(
    session: Res<PreviewSession>,
    subjects: Query<(), With<EditorEntity>>,
    changed: Query<(), Changed<Bindings>>,
    removed: RemovedComponents<Bindings>,
    roots: Query<Entity, crate::prefab::AuthoredUiSceneRoot>,
) -> bool {
    if !session.on {
        return false;
    }
    match session.subject {
        None => return true,
        Some(subject) if !subjects.contains(subject) => return true,
        Some(_) => {}
    }
    if session.root != roots.iter().min() {
        return true;
    }
    !changed.is_empty() || !removed.is_empty()
}

// ---------------------------------------------------------------------------
// Reading the scene's bindings
// ---------------------------------------------------------------------------

/// Every entity of the UI scene, root included.
fn scene_subtree(world: &mut World, root: Entity) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut children = world.query::<&Children>();
    while let Some(entity) = stack.pop() {
        out.push(entity);
        if let Ok(kids) = children.get(world, entity) {
            stack.extend(kids.iter());
        }
    }
    out
}

/// Every read path a binding names, in authored order.
fn read_paths(binding: &Binding) -> Vec<&BindPath> {
    match binding {
        Binding::Field { read, .. } => read.iter().collect(),
        Binding::Text { args, .. } => args.iter().collect(),
        Binding::Visible { read, .. } => vec![read],
        Binding::Value { with, .. } => vec![with],
        Binding::Action { fields, .. } => fields.iter().map(|(_, path)| path).collect(),
    }
}

/// The types the scene's bindings read, resolved and described.
///
/// A resource read is listed beside a component one: the context entity cannot
/// carry a resource, so the session stands one up in the world instead, and a
/// scene that reads only resources still has to list something or the panel
/// says the scene reads nothing at all.
fn referenced_subjects(world: &mut World, root: Entity) -> Vec<PreviewSubject> {
    let mut wanted: Vec<(String, PreviewSource)> = Vec::new();
    let entities = scene_subtree(world, root);
    for entity in entities {
        let Some(bindings) = world.get::<Bindings>(entity) else {
            continue;
        };
        for binding in &bindings.0 {
            for path in read_paths(binding) {
                let named = match path.parse() {
                    Ok(ParsedPath::Component { type_path, .. }) => {
                        (type_path, PreviewSource::Component)
                    }
                    Ok(ParsedPath::Resource { type_path, .. }) => {
                        (type_path, PreviewSource::Resource)
                    }
                    Err(_) => continue,
                };
                if !wanted.contains(&named) {
                    wanted.push(named);
                }
            }
        }
    }
    wanted.sort_by(|left, right| left.0.cmp(&right.0));

    let mut subjects: Vec<PreviewSubject> = {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let project = world.get_resource::<ProjectTypes>();
        wanted
            .into_iter()
            .map(|(path, source)| describe(&path, source, &registry, project))
            .collect()
    };
    mark_editor_owned(world, &mut subjects);
    subjects
}

/// Demote a resource the editor already holds. Whatever is in the world is
/// either the editor's own or this session's stand-in, and only the second is
/// a value the panel may hand the user.
fn mark_editor_owned(world: &mut World, subjects: &mut [PreviewSubject]) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let ours = world.resource::<PreviewSession>().resources.clone();
    for subject in subjects {
        if subject.source != PreviewSource::Resource
            || subject.availability != PreviewAvailability::Native
        {
            continue;
        }
        let Some(registration) = registry.get_with_type_path(&subject.type_path) else {
            continue;
        };
        let type_id = registration.type_id();
        if ours.iter().any(|(id, _)| *id == type_id) {
            continue;
        }
        if resource_entity(world, type_id).is_some() {
            subject.availability = PreviewAvailability::EditorOwned;
            subject.note = EDITOR_OWNED_NOTE.to_string();
        }
    }
}

/// Resolve one referenced type path: the project schema first, then the
/// editor's own registry.
fn describe(
    path: &str,
    source: PreviewSource,
    registry: &TypeRegistry,
    project: Option<&ProjectTypes>,
) -> PreviewSubject {
    let short = |path: &str| path.rsplit("::").next().unwrap_or(path).to_string();
    if let Some(schema) = project.and_then(|project| project_schema(project, path, source)) {
        return PreviewSubject {
            type_path: schema.type_path.clone(),
            short_name: short(&schema.type_path),
            source,
            availability: PreviewAvailability::SchemaOnly,
            note: SCHEMA_ONLY_NOTE.to_string(),
            fields: schema
                .fields
                .iter()
                .map(|field| schema_field_row(&field.name, &field.type_path))
                .collect(),
        };
    }
    let registration = registry
        .get_with_type_path(path)
        .or_else(|| registry.get_with_short_type_path(path));
    let usable = registration.filter(|reg| match source {
        PreviewSource::Component => reg.data::<ReflectComponent>().is_some(),
        PreviewSource::Resource => reg.data::<bevy::ecs::reflect::ReflectResource>().is_some(),
    });
    match usable {
        Some(registration) => {
            let type_path = registration.type_info().type_path().to_string();
            let built =
                crate::reflect_default::build_reflective_default(registration.type_id(), registry);
            // Both ways of ending up with no rows carry their own reason, so a
            // header is never left bare.
            let note = match &built {
                None => UNCONSTRUCTIBLE_NOTE,
                Some(_) => NO_FIELDS_NOTE,
            };
            let fields = built
                .map(|value| field_rows(value.as_partial_reflect()))
                .unwrap_or_default();
            PreviewSubject {
                short_name: short(&type_path),
                type_path,
                source,
                availability: PreviewAvailability::Native,
                note: if fields.is_empty() {
                    note.to_string()
                } else {
                    String::new()
                },
                fields,
            }
        }
        None => PreviewSubject {
            type_path: path.to_string(),
            short_name: short(path),
            source,
            availability: PreviewAvailability::Unknown,
            note: UNKNOWN_NOTE.to_string(),
            fields: Vec::new(),
        },
    }
}

/// A schema'd field, which names a tuple element by its bare index.
///
/// Only an all-digit name is an index: a field called `x2` is a named field
/// and reaches its value without a leading dot.
fn schema_field_row(name: &str, type_path: &str) -> PreviewFieldRow {
    let indexed = !name.is_empty() && name.chars().all(|c| c.is_ascii_digit());
    let path = if indexed {
        format!(".{name}")
    } else {
        name.to_string()
    };
    PreviewFieldRow {
        name: path.clone(),
        path,
        kind: kind_of_type_path(type_path),
    }
}

/// A project schema for a path a binding wrote, matched on the full path or on
/// the short name the binding may have used instead.
///
/// Keyed on how the binding named the type: a project's resources live in
/// their own map, so a `Res(T)` read looked up among the components resolves
/// to a type nothing knows.
fn project_schema<'a>(
    project: &'a ProjectTypes,
    path: &str,
    source: PreviewSource,
) -> Option<&'a jackdaw_schema::TypeSchema> {
    let named = |schema: &&jackdaw_schema::TypeSchema| schema.short_name == path;
    match source {
        PreviewSource::Component => project
            .component(path)
            .or_else(|| project.components().find(named)),
        PreviewSource::Resource => project
            .resources()
            .find(|schema| schema.type_path == path)
            .or_else(|| project.resources().find(named)),
    }
}

/// The fields of a struct or a tuple struct, each tagged with the control it
/// takes. A tuple element is named by its index, which is also the reflect
/// path that reaches it.
fn field_rows(value: &dyn PartialReflect) -> Vec<PreviewFieldRow> {
    match value.reflect_ref() {
        ReflectRef::Struct(structure) => (0..structure.field_len())
            .filter_map(|index| {
                let name = structure.name_at(index)?;
                let field = structure.field_at(index)?;
                Some(PreviewFieldRow {
                    name: name.to_string(),
                    path: name.to_string(),
                    kind: kind_of(field),
                })
            })
            .collect(),
        ReflectRef::TupleStruct(tuple) => (0..tuple.field_len())
            .filter_map(|index| {
                let field = tuple.field(index)?;
                let path = format!(".{index}");
                Some(PreviewFieldRow {
                    name: path.clone(),
                    path,
                    kind: kind_of(field),
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn kind_of(value: &dyn PartialReflect) -> PreviewFieldKind {
    let path = value
        .get_represented_type_info()
        .map(TypeInfo::type_path)
        .unwrap_or_default();
    kind_of_type_path(path)
}

/// Every numeric width the preview panel offers a control for.
///
/// The one list behind three things that must not drift: the names
/// `kind_of_type_path` advertises, the widths `apply_scalar` writes, and the
/// widths `read_scalar` reads back. The suite generates its fixture from it
/// too, so a width added here is advertised, written, read, and tested by that
/// edit alone.
///
/// Float-to-integer `as` casts saturate, so a scrub past a narrow type's range
/// stops at its bound rather than wrapping into a value nobody asked for.
///
/// Braces, not parentheses: the list is handed to macros that expand to items
/// as well as to ones that expand to expressions, and only a braced call works
/// in both positions.
#[macro_export]
macro_rules! numeric_widths {
    ($macro:ident) => {
        $macro! { f32, f64, i8, i16, i32, i64, isize, u8, u16, u32, u64, usize }
    };
}

/// The type names [`numeric_widths`] lists, which is what the panel offers a
/// number control for.
macro_rules! width_names {
    ($($width:ty),*) => {
        &[$(stringify!($width)),*]
    };
}

/// What the panel offers for a type, by name.
///
/// The numeric names are generated from the one list `apply_scalar` and
/// `read_scalar` are generated from, so a width cannot be advertised without
/// also being writable and readable. `Vec3A` is absent: the inspector calls it
/// uneditable, and preview matches it rather than advertising a control over a
/// value the rest of the editor will not author.
fn kind_of_type_path(path: &str) -> PreviewFieldKind {
    const NUMBERS: &[&str] = numeric_widths!(width_names);
    let name = path.rsplit("::").next().unwrap_or(path);
    if NUMBERS.contains(&name) {
        return PreviewFieldKind::Number;
    }
    match name {
        "bool" => PreviewFieldKind::Bool,
        "String" => PreviewFieldKind::Text,
        "Vec2" => PreviewFieldKind::Vector2,
        "Vec3" => PreviewFieldKind::Vector3,
        "Vec4" | "Quat" => PreviewFieldKind::Vector4,
        _ => PreviewFieldKind::Unsupported,
    }
}

/// What the panel draws: the referenced types of the open scene, or an empty
/// list when nothing is being previewed.
pub fn preview_layout(world: &mut World) -> Vec<PreviewSubject> {
    world.resource::<PreviewSession>().subjects.clone()
}

// ---------------------------------------------------------------------------
// Restoring what the evaluator overwrote
// ---------------------------------------------------------------------------

/// The components each binding kind writes on its own widget entity, named by
/// `TypeId` rather than by path so a rename in bevy is a compile error rather
/// than a silently missed restore. A `Field` binding's target comes from its
/// own write path instead, and an `Action` writes nothing on the widget.
fn implicit_write_targets(binding: &Binding) -> Vec<TypeId> {
    match binding {
        Binding::Text { .. } => vec![TypeId::of::<Text>()],
        Binding::Visible { .. } => vec![TypeId::of::<Visibility>()],
        Binding::Value { .. } => vec![
            TypeId::of::<bevy::ui_widgets::SliderValue>(),
            TypeId::of::<Checked>(),
        ],
        Binding::Field { .. } | Binding::Action { .. } => Vec::new(),
    }
}

/// Every component the scene's bindings would have the evaluator write, as
/// (entity, type). Split from the snapshot so the session can ask what it owns
/// without cloning what it holds: the answer is compared every resync, and it
/// moving is itself a reason to resync.
fn write_target_ids(world: &mut World, root: Entity) -> Vec<(Entity, TypeId)> {
    let mut by_path: Vec<(Entity, String)> = Vec::new();
    let mut by_id: Vec<(Entity, TypeId)> = Vec::new();
    for entity in scene_subtree(world, root) {
        let Some(bindings) = world.get::<Bindings>(entity) else {
            continue;
        };
        for binding in &bindings.0 {
            if let Binding::Field { write, .. } = binding
                && let Ok(ParsedPath::Component { type_path, .. }) = write.parse()
            {
                by_path.push((entity, type_path));
            }
            for type_id in implicit_write_targets(binding) {
                by_id.push((entity, type_id));
            }
        }
    }

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let resolved = by_path.into_iter().filter_map(|(entity, type_path)| {
        registry
            .get_with_type_path(&type_path)
            .or_else(|| registry.get_with_short_type_path(&type_path))
            .map(|registration| (entity, registration.type_id()))
    });
    let mut out: Vec<(Entity, TypeId)> = Vec::new();
    for (entity, type_id) in resolved.chain(by_id) {
        // A type with no `ReflectComponent` cannot be snapshotted or put back,
        // so it is not something the session can claim to own either.
        if registry
            .get(type_id)
            .is_none_or(|registration| registration.data::<ReflectComponent>().is_none())
        {
            continue;
        }
        if !out.contains(&(entity, type_id)) {
            out.push((entity, type_id));
        }
    }
    out
}

/// Read what each named target holds right now, so the session can put it back.
fn snapshot_targets(world: &mut World, targets: &[(Entity, TypeId)]) -> Vec<WriteTarget> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let mut out: Vec<WriteTarget> = Vec::new();
    for (entity, type_id) in targets.iter().copied() {
        let Some(reflect_component) = registry
            .get(type_id)
            .and_then(|registration| registration.data::<ReflectComponent>())
        else {
            continue;
        };
        let Ok(entity_ref) = world.get_entity(entity) else {
            continue;
        };
        let value = reflect_component
            .reflect(entity_ref)
            .map(PartialReflect::to_dynamic);
        out.push(WriteTarget {
            entity,
            type_id,
            value,
        });
    }
    out
}

fn restore_write_targets(world: &mut World, targets: Vec<WriteTarget>) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    for target in targets {
        let Some(reflect_component) = registry
            .get(target.type_id)
            .and_then(|registration| registration.data::<ReflectComponent>())
        else {
            continue;
        };
        let Ok(mut entity) = world.get_entity_mut(target.entity) else {
            continue;
        };
        match target.value {
            Some(value) => reflect_component.insert(&mut entity, value.as_ref(), &registry),
            None => reflect_component.remove(&mut entity),
        }
    }
}

/// What a running preview had written, held while something else reads the
/// live world. Returned by [`suspend_preview_writes`], and only useful as the
/// argument to [`resume_preview_writes`].
pub struct SuspendedPreview(Vec<WriteTarget>);

/// Put the authored values back over whatever the running preview wrote, and
/// hand back what the preview had there.
///
/// A previewed value must never reach the user's file. An authored edit to a
/// previewed property is refused outright, but any part of the emitter that
/// re-reads live ECS (asset discovery and the handle-bearing patch
/// re-derivation both do) would otherwise read the preview's value rather than
/// the author's.
///
/// The guarantee is enforced at the one place every emission passes through
/// rather than at each live-ECS read inside it: the world is put back to what
/// the author wrote for the length of the emission, and the preview is
/// reinstated afterwards. A live-ECS read added to the emitter later inherits
/// the guarantee instead of escaping it.
///
/// `None` means there was nothing to suspend (no session, or a session that
/// has not written anything yet); pass it straight back to
/// [`resume_preview_writes`].
pub fn suspend_preview_writes(world: &mut World) -> Option<SuspendedPreview> {
    let session = world.get_resource::<PreviewSession>()?;
    if !session.on || session.restore.is_empty() {
        return None;
    }
    // Cloned rather than taken: the session still owes these values to its own
    // teardown, and an emission is not the end of the session.
    let authored: Vec<WriteTarget> = session
        .restore
        .iter()
        .map(|target| WriteTarget {
            entity: target.entity,
            type_id: target.type_id,
            value: target
                .value
                .as_ref()
                .map(|value| PartialReflect::to_dynamic(value.as_ref())),
        })
        .collect();
    let targets: Vec<(Entity, TypeId)> = authored
        .iter()
        .map(|target| (target.entity, target.type_id))
        .collect();
    let previewed = snapshot_targets(world, &targets);
    restore_write_targets(world, authored);
    Some(SuspendedPreview(previewed))
}

/// Put back what [`suspend_preview_writes`] took out, so the session carries
/// on from where the emission interrupted it.
pub fn resume_preview_writes(world: &mut World, held: Option<SuspendedPreview>) {
    if let Some(SuspendedPreview(previewed)) = held {
        restore_write_targets(world, previewed);
    }
}

// ---------------------------------------------------------------------------
// Writing the scratch entity
// ---------------------------------------------------------------------------

/// Write one field of one previewed type: a component of the scratch entity,
/// or the resource the session stood up for it.
///
/// Plain reflect writes with no undo entry and no document patch: the scratch
/// entity is editor-only state, so an edit here is not something the user can
/// lose or wants to undo.
pub fn write_scratch_field(
    world: &mut World,
    field: &PreviewField,
    value: PreviewValue,
) -> Result<(), PreviewError> {
    match listed_source(world, &field.type_path) {
        Some(PreviewSource::Resource) => write_resource_field(world, field, &value),
        _ => write_component_field(world, field, &value),
    }
}

/// How the session listed `type_path`, matched on either spelling a binding
/// may have used. `None` for a type no listed subject names.
fn listed_source(world: &World, type_path: &str) -> Option<PreviewSource> {
    world
        .resource::<PreviewSession>()
        .subjects
        .iter()
        .find(|subject| subject.type_path == type_path || subject.short_name == type_path)
        .map(|subject| subject.source)
}

fn write_component_field(
    world: &mut World,
    field: &PreviewField,
    value: &PreviewValue,
) -> Result<(), PreviewError> {
    let subject = world
        .resource::<PreviewSession>()
        .subject
        .ok_or(PreviewError::NotPreviewing)?;
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let reflect_component = registry
        .get_with_type_path(&field.type_path)
        .or_else(|| registry.get_with_short_type_path(&field.type_path))
        .and_then(|registration| registration.data::<ReflectComponent>())
        .ok_or_else(|| PreviewError::NoSuchComponent(field.type_path.clone()))?;
    let mut entity = world
        .get_entity_mut(subject)
        .map_err(|_| PreviewError::NotPreviewing)?;
    let mut component = reflect_component
        .reflect_mut(&mut entity)
        .ok_or_else(|| PreviewError::NoSuchComponent(field.type_path.clone()))?;
    let target = component
        .reflect_path_mut(field.field.as_str())
        .map_err(|_| PreviewError::NoSuchField(field.field.clone()))?;
    apply_scalar(target, value)
        .then_some(())
        .ok_or_else(|| PreviewError::NoSuchField(field.field.clone()))
}

/// The resource half. Refused unless the session stood the resource up
/// itself: anything else in the world is the editor's own state.
fn write_resource_field(
    world: &mut World,
    field: &PreviewField,
    value: &PreviewValue,
) -> Result<(), PreviewError> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let registration = registry
        .get_with_type_path(&field.type_path)
        .or_else(|| registry.get_with_short_type_path(&field.type_path))
        .ok_or_else(|| PreviewError::NoSuchComponent(field.type_path.clone()))?;
    let type_id = registration.type_id();
    if !world
        .resource::<PreviewSession>()
        .resources
        .iter()
        .any(|(id, _)| *id == type_id)
    {
        return Err(PreviewError::EditorOwned(field.type_path.clone()));
    }
    let reflect_component = registration
        .data::<ReflectComponent>()
        .ok_or_else(|| PreviewError::NoSuchComponent(field.type_path.clone()))?;
    let entity = resource_entity(world, type_id)
        .ok_or_else(|| PreviewError::NoSuchComponent(field.type_path.clone()))?;
    let mut entity = world
        .get_entity_mut(entity)
        .map_err(|_| PreviewError::NoSuchComponent(field.type_path.clone()))?;
    let mut resource = reflect_component
        .reflect_mut(&mut entity)
        .ok_or_else(|| PreviewError::NoSuchComponent(field.type_path.clone()))?;
    let target = resource
        .reflect_path_mut(field.field.as_str())
        .map_err(|_| PreviewError::NoSuchField(field.field.clone()))?;
    apply_scalar(target, value)
        .then_some(())
        .ok_or_else(|| PreviewError::NoSuchField(field.field.clone()))
}

/// Set a scalar of whatever numeric width the field actually has. Returns
/// false for a shape no control writes.
///
/// Every width answered here is one [`kind_of_type_path`] advertises, and
/// [`read_scalar`] reads back the same set: a control the panel offers has to
/// be able to land its value and show it again.
fn apply_scalar(target: &mut dyn PartialReflect, value: &PreviewValue) -> bool {
    match value {
        PreviewValue::Number(number) => {
            let number = *number;
            macro_rules! write_width {
                ($($width:ty),*) => {
                    $(
                        if let Some(slot) = target.try_downcast_mut::<$width>() {
                            *slot = number as $width;
                            return true;
                        }
                    )*
                };
            }
            numeric_widths!(write_width);
            false
        }
        PreviewValue::Bool(new) => match target.try_downcast_mut::<bool>() {
            Some(slot) => {
                *slot = *new;
                true
            }
            None => false,
        },
        PreviewValue::Text(new) => match target.try_downcast_mut::<String>() {
            Some(slot) => {
                slot.clone_from(new);
                true
            }
            None => false,
        },
    }
}

/// The other half of [`apply_scalar`]: what a control seeds itself from.
///
/// A wide integer is handed back through `f64`, which is what the scrub
/// control holds, so a value past 2^53 comes back rounded. That is the
/// control's precision, not a lost write.
fn read_scalar(value: &dyn PartialReflect) -> Option<PreviewValue> {
    macro_rules! read_width {
        ($($width:ty),*) => {
            $(
                if let Some(number) = value.try_downcast_ref::<$width>() {
                    return Some(PreviewValue::Number(*number as f64));
                }
            )*
        };
    }
    numeric_widths!(read_width);
    if let Some(flag) = value.try_downcast_ref::<bool>() {
        return Some(PreviewValue::Bool(*flag));
    }
    if let Some(text) = value.try_downcast_ref::<String>() {
        return Some(PreviewValue::Text(text.clone()));
    }
    None
}

/// The value a control for `field` seeds itself with, or `None` when the
/// session is not holding that type.
pub fn read_scratch_value(world: &World, field: &PreviewField) -> Option<PreviewValue> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let registration = registry
        .get_with_type_path(&field.type_path)
        .or_else(|| registry.get_with_short_type_path(&field.type_path))?;
    let holder = match listed_source(world, &field.type_path) {
        Some(PreviewSource::Resource) => resource_entity(world, registration.type_id())?,
        _ => world.resource::<PreviewSession>().subject?,
    };
    let held = registration
        .data::<ReflectComponent>()?
        .reflect(world.get_entity(holder).ok()?)?;
    read_scalar(held.reflect_path(field.field.as_str()).ok()?)
}

// ---------------------------------------------------------------------------
// The panel
// ---------------------------------------------------------------------------

/// Root of one preview window's content.
#[derive(Component, Default)]
pub struct PreviewContextPanel;

/// The toggle that starts and stops a session.
#[derive(Component, Default)]
pub struct PreviewToggle;

/// A field row the editor cannot drive, because its type exists only as
/// project schema here. Carries what the row would have written.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct PreviewDisabledField(pub PreviewField);

/// The state a panel's rows were built from. Only structure lives here:
/// scrubbing a value must not rebuild the rows out from under the drag.
/// `None` is a panel that has never been built, which is not the same as one
/// built while preview was off.
#[derive(Component, Default)]
struct PreviewRevision(Option<PreviewShape>);

#[derive(Clone, PartialEq)]
struct PreviewShape {
    on: bool,
    has_scene: bool,
    subjects: Vec<PreviewSubject>,
}

/// Build a preview window's content under `window`.
pub fn build_preview_context_panel(window: &mut ChildSpawner) {
    window.spawn((
        PreviewContextPanel,
        PreviewRevision::default(),
        EditorEntity,
        Node {
            width: percent(100),
            height: percent(100),
            min_height: px(0),
            flex_direction: FlexDirection::Column,
            overflow: Overflow::scroll_y(),
            padding: UiRect::all(px(tokens::SPACING_SM)),
            row_gap: px(tokens::SPACING_XS),
            ..default()
        },
        ScrollPosition::default(),
        BackgroundColor(tokens::PANEL_BG),
    ));
}

/// Whether any panel's rows could be out of date: the session changed, or a
/// panel opened that has never been built. Values do not count: a scrub must
/// not rebuild the control under the pointer.
fn preview_panels_are_stale(
    session: Res<PreviewSession>,
    unbuilt: Query<&PreviewRevision>,
) -> bool {
    session.is_changed() || unbuilt.iter().any(|revision| revision.0.is_none())
}

fn refresh_preview_panels(world: &mut World) {
    let session = world.resource::<PreviewSession>();
    let revision = PreviewShape {
        on: session.on,
        has_scene: session.subject.is_some(),
        subjects: session.subjects.clone(),
    };
    let stale = world
        .query::<(Entity, &PreviewRevision)>()
        .iter(world)
        .filter(|(_, current)| current.0.as_ref() != Some(&revision))
        .map(|(entity, _)| entity)
        .collect::<Vec<_>>();

    for panel in stale {
        let children = world
            .get::<Children>(panel)
            .map(|children| children.iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for child in children {
            if let Ok(child) = world.get_entity_mut(child) {
                child.despawn();
            }
        }
        spawn_panel_rows(world, panel, &revision);
        world
            .entity_mut(panel)
            .insert(PreviewRevision(Some(revision.clone())));
    }
}

fn spawn_panel_rows(world: &mut World, panel: Entity, revision: &PreviewShape) {
    let mut queue = world.commands();
    let mut toggle = queue.spawn_scene(bsn! {
        @FeathersCheckbox { @caption: bsn! { Text("Preview") ThemedText } }
    });
    toggle.insert((PreviewToggle, EditorEntity, ChildOf(panel)));
    if revision.on {
        toggle.insert(Checked);
    }
    world.flush();

    if !revision.on {
        spawn_note(
            world,
            panel,
            "Preview runs this scene's bindings against a stand-in subject.",
        );
        return;
    }
    if !revision.has_scene {
        spawn_note(world, panel, "Open a UI scene to preview its bindings.");
        return;
    }
    if revision.subjects.is_empty() {
        spawn_note(world, panel, "No binding in this scene reads a component.");
        return;
    }

    for entry in &revision.subjects {
        spawn_section_header(world, panel, entry);
        for field in &entry.fields {
            if entry.availability == PreviewAvailability::Native {
                spawn_scrub_row(world, panel, entry, field);
            } else {
                // The user authored a binding against these fields, so the
                // panel lists them even though nothing here can drive them.
                spawn_disabled_row(world, panel, entry, field);
            }
        }
        // Including a linked type that yielded no rows: a header alone says
        // nothing about why there is nothing under it.
        if !entry.note.is_empty() {
            spawn_note(world, panel, &entry.note);
        }
    }
}

fn spawn_note(world: &mut World, panel: Entity, text: &str) {
    world.spawn((
        EditorEntity,
        Text::new(text.to_string()),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(panel),
    ));
}

/// A section header, badged when the type under it is one the editor cannot
/// drive: the label takes the colour of the state and carries the reason as a
/// tooltip.
fn spawn_section_header(world: &mut World, panel: Entity, entry: &PreviewSubject) {
    let color = match entry.availability {
        PreviewAvailability::Native if entry.note.is_empty() => tokens::TEXT_SECONDARY,
        PreviewAvailability::Native
        | PreviewAvailability::SchemaOnly
        | PreviewAvailability::EditorOwned => tokens::TEXT_WARNING,
        PreviewAvailability::Unknown => tokens::TEXT_ERROR,
    };
    let mut header = world.spawn((
        EditorEntity,
        Text::new(entry.short_name.clone()),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(color),
        Node {
            margin: UiRect::top(px(tokens::SPACING_SM)),
            ..default()
        },
        ChildOf(panel),
    ));
    if !entry.note.is_empty() {
        // `Hovered` is opt-in: the tooltip renderer only looks at entities
        // carrying both, so a `Tooltip` on its own never surfaces.
        header.insert((Hovered::default(), Tooltip::title(entry.note.clone())));
    }
}

/// How a read-only row prints the value behind it.
fn display_value(value: &PreviewValue) -> String {
    match value {
        PreviewValue::Number(number) => format!("{number}"),
        PreviewValue::Bool(flag) => flag.to_string(),
        PreviewValue::Text(text) => text.clone(),
    }
}

/// One field of a type the panel cannot drive: the same row shape, with no
/// control to touch.
///
/// An editor-owned resource is real state, so the row shows what it holds. A
/// schema-only type has no value to show, the editor having never constructed
/// one, and keeps the placeholder.
fn spawn_disabled_row(
    world: &mut World,
    panel: Entity,
    entry: &PreviewSubject,
    field: &PreviewFieldRow,
) {
    let binding = PreviewField::new(entry.type_path.clone(), field.path.clone());
    let shown = match entry.availability {
        PreviewAvailability::EditorOwned => read_scratch_value(world, &binding)
            .as_ref()
            .map(display_value),
        _ => None,
    };
    let mut queue = world.commands();
    let row = spawn_field_row(&mut queue, panel, FieldRowProps::new(field.name.clone()));
    queue.entity(row.row).insert(InteractionDisabled);
    queue.spawn((
        PreviewDisabledField(binding),
        EditorEntity,
        InteractionDisabled,
        Text::new(shown.unwrap_or_else(|| "--".to_string())),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(row.control),
    ));
    world.flush();
}

/// A math vector as the inspector draws one: the field's name above a row of
/// coloured axis inputs, each writing its own `.x`/`.y`/`.z`/`.w` sub-path.
fn spawn_axes_row(
    world: &mut World,
    panel: Entity,
    entry: &PreviewSubject,
    field: &PreviewFieldRow,
) {
    let axes: Vec<(String, PreviewField, f64)> = field
        .kind
        .axes()
        .iter()
        .map(|axis| {
            let binding =
                PreviewField::new(entry.type_path.clone(), format!("{}.{axis}", field.path));
            let value = match read_scratch_value(world, &binding) {
                Some(PreviewValue::Number(number)) => number,
                _ => 0.0,
            };
            ((*axis).to_string(), binding, value)
        })
        .collect();

    let mut queue = world.commands();
    let column = queue
        .spawn((
            EditorEntity,
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(tokens::SPACING_XS),
                width: percent(100),
                ..default()
            },
            ChildOf(panel),
        ))
        .id();
    queue.spawn((
        EditorEntity,
        Text::new(field.name.clone()),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..default()
        },
        TextColor(tokens::TEXT_TERTIARY),
        ChildOf(column),
    ));
    let axes_row = queue
        .spawn((
            EditorEntity,
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: px(tokens::SPACING_XS),
                width: percent(100),
                ..default()
            },
            ChildOf(column),
        ))
        .id();
    for (axis, binding, value) in axes {
        let (sigil, label) = match axis.as_str() {
            "x" => (feathers_tokens::TEXT_INPUT_X_AXIS, "X"),
            "y" => (feathers_tokens::TEXT_INPUT_Y_AXIS, "Y"),
            "z" => (feathers_tokens::TEXT_INPUT_Z_AXIS, "Z"),
            // No token for a fourth component, the same gap the inspector has.
            _ => (feathers_tokens::TEXT_INPUT_BG, "W"),
        };
        queue
            .spawn_scene(bsn! {
                @ScrubNumberInput { @sigil_color: {sigil}, @label_text: {label} }
                Node { flex_grow: 1.0, height: px(22.0) }
            })
            .insert((
                ScrubNumberInputValue::F64(value),
                NumberInputPrecision(2),
                binding,
                EditorEntity,
                ChildOf(axes_row),
            ));
    }
    world.flush();
}

fn spawn_scrub_row(
    world: &mut World,
    panel: Entity,
    entry: &PreviewSubject,
    field: &PreviewFieldRow,
) {
    if !field.kind.axes().is_empty() {
        spawn_axes_row(world, panel, entry, field);
        return;
    }
    let binding = PreviewField::new(entry.type_path.clone(), field.path.clone());
    let value = read_scratch_value(world, &binding);
    let mut queue = world.commands();
    let row = spawn_field_row(&mut queue, panel, FieldRowProps::new(field.name.clone()));
    match field.kind {
        PreviewFieldKind::Number => {
            let number = match value {
                Some(PreviewValue::Number(number)) => number,
                _ => 0.0,
            };
            queue
                .spawn_scene(bsn! {
                    @ScrubNumberInput
                    Node { width: percent(100.0), height: px(22.0) }
                })
                .insert((
                    ScrubNumberInputValue::F64(number),
                    NumberInputPrecision(2),
                    binding,
                    EditorEntity,
                    ChildOf(row.control),
                ));
        }
        PreviewFieldKind::Bool => {
            let mut checkbox = queue.spawn_scene(bsn! { @FeathersCheckbox });
            checkbox.insert((binding, EditorEntity, ChildOf(row.control)));
            if matches!(value, Some(PreviewValue::Bool(true))) {
                checkbox.insert(Checked);
            }
        }
        PreviewFieldKind::Text => {
            let text = match value {
                Some(PreviewValue::Text(text)) => text,
                _ => String::new(),
            };
            queue.spawn((
                text_edit(TextEditProps::default().with_default_value(text).grow()),
                binding,
                EditorEntity,
                ChildOf(row.control),
            ));
        }
        // A vector took the axis row above, so anything reaching here is a
        // shape the panel has no control for.
        PreviewFieldKind::Vector2
        | PreviewFieldKind::Vector3
        | PreviewFieldKind::Vector4
        | PreviewFieldKind::Unsupported => {
            queue.spawn((
                EditorEntity,
                Text::new("not scrubbable".to_string()),
                TextFont {
                    font_size: tokens::TEXT_SIZE_XS,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                ChildOf(row.control),
            ));
        }
    }
    world.flush();
}

// ---------------------------------------------------------------------------
// Commits
// ---------------------------------------------------------------------------

fn on_preview_toggle(
    event: On<ValueChange<bool>>,
    toggles: Query<(), With<PreviewToggle>>,
    mut commands: Commands,
) {
    let target = event.event_target();
    if !toggles.contains(target) {
        return;
    }
    let on = event.value;
    // `FeathersCheckbox` does not self-manage `Checked`.
    if on {
        commands.entity(target).insert(Checked);
    } else {
        commands.entity(target).remove::<Checked>();
    }
    commands.queue(move |world: &mut World| set_preview(world, on));
}

fn on_scrub_commit(
    event: On<ValueChange<f32>>,
    fields: Query<&PreviewField>,
    mut commands: Commands,
) {
    let Ok(field) = fields.get(event.event_target()) else {
        return;
    };
    let field = field.clone();
    let value = f64::from(event.value);
    commands.queue(move |world: &mut World| {
        report(
            write_scratch_field(world, &field, PreviewValue::Number(value)),
            &field,
        );
    });
}

fn on_checkbox_commit(
    event: On<ValueChange<bool>>,
    fields: Query<&PreviewField>,
    mut commands: Commands,
) {
    let target = event.event_target();
    let Ok(field) = fields.get(target) else {
        return;
    };
    let field = field.clone();
    let value = event.value;
    if value {
        commands.entity(target).insert(Checked);
    } else {
        commands.entity(target).remove::<Checked>();
    }
    commands.queue(move |world: &mut World| {
        report(
            write_scratch_field(world, &field, PreviewValue::Bool(value)),
            &field,
        );
    });
}

fn on_text_commit(
    event: On<TextEditCommitEvent>,
    fields: Query<&PreviewField>,
    mut commands: Commands,
) {
    let Ok(field) = fields.get(event.entity) else {
        return;
    };
    let field = field.clone();
    let text = event.text.clone();
    commands.queue(move |world: &mut World| {
        report(
            write_scratch_field(world, &field, PreviewValue::Text(text)),
            &field,
        );
    });
}

fn report(result: Result<(), PreviewError>, field: &PreviewField) {
    if let Err(error) = result {
        warn!(
            "preview edit of `{}.{}`: {error}",
            field.type_path, field.field
        );
    }
}
