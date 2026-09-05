use bevy::ecs::change_detection::Tick;
use bevy::ecs::component::ComponentId;
use bevy::ecs::reflect::{ReflectComponent, ReflectResource};
use bevy::prelude::*;
use bevy::reflect::ParsedPath as FieldPath;
use bevy::reflect::prelude::ReflectDefault;
use bevy::reflect::{GetPath, PartialReflect, TypePathTable, TypeRegistration, TypeRegistry};

use crate::{BindError, BindPath, ParsedPath};

/// A value on its way out of game state and into a widget. Every numeric type
/// a binding can read arrives here as an `f32`.
#[derive(Clone, Debug, PartialEq)]
pub enum BindValue {
    /// Any number the source held, widened to `f32`.
    F32(f32),
    /// A bool, which is what a `Visible` binding needs.
    Bool(bool),
    /// A string, which as it stands only a `Text` binding can use.
    Str(String),
}

/// Entities the search for a context looks at: the widget itself and the 63
/// above it. The cap stops a cycle in `ChildOf` from hanging the frame.
const MAX_CONTEXT_WALK: usize = 64;

/// The entity a widget's component paths read from: its own
/// [`BindContext`](crate::BindContext), or the nearest one above it. `None`
/// when nothing on the way up names a context, and when the search runs out
/// of entities to look at.
pub fn resolve_context(world: &World, entity: Entity) -> Option<Entity> {
    let mut current = entity;
    for _ in 0..MAX_CONTEXT_WALK {
        if let Some(ctx) = world.get::<crate::BindContext>(current) {
            return Some(ctx.0);
        }
        match world.get::<ChildOf>(current) {
            Some(parent) => current = parent.parent(),
            None => return None,
        }
    }
    warn!(
        "binding on {entity}: gave up looking for a BindContext after {MAX_CONTEXT_WALK} entities \
         up the tree -- either nothing above it names a subject, or ChildOf loops"
    );
    None
}

/// Looks a type up by full path or by trailing segment alone; a short name
/// several types answer to is reported as ambiguous. `noun` names the thing in
/// the not-found message ("type", "event type").
pub fn lookup_registration<'a>(
    registry: &'a TypeRegistry,
    type_path: &str,
    noun: &'static str,
) -> Result<&'a TypeRegistration, BindError> {
    if let Some(reg) = registry
        .get_with_type_path(type_path)
        .or_else(|| registry.get_with_short_type_path(type_path))
    {
        return Ok(reg);
    }
    if registry.is_ambiguous(type_path) {
        let mut paths: Vec<&str> = registry
            .iter()
            .map(|reg| reg.type_info().type_path_table())
            .filter(|table| table.short_path() == type_path)
            .map(TypePathTable::path)
            .collect();
        paths.sort_unstable();
        return Err(BindError::AmbiguousType {
            type_path: type_path.to_string(),
            candidates: paths.into_iter().map(str::to_string).collect(),
        });
    }
    Err(BindError::UnknownType {
        noun,
        type_path: type_path.to_string(),
    })
}

pub(crate) fn extract(value: &dyn PartialReflect, raw: &str) -> Result<BindValue, BindError> {
    if let Some(v) = value.try_downcast_ref::<f32>() {
        return Ok(BindValue::F32(*v));
    }
    if let Some(v) = value.try_downcast_ref::<f64>() {
        return Ok(BindValue::F32(*v as f32));
    }
    // Every width the write side narrows back into: a width readable here but
    // not writable there is a binding that works in one direction only.
    macro_rules! integer_source {
        ($($ty:ty),* $(,)?) => {
            $(
                if let Some(v) = value.try_downcast_ref::<$ty>() {
                    return Ok(BindValue::F32(*v as f32));
                }
            )*
        };
    }
    integer_source!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize);
    if let Some(v) = value.try_downcast_ref::<bool>() {
        return Ok(BindValue::Bool(*v));
    }
    if let Some(v) = value.try_downcast_ref::<String>() {
        return Ok(BindValue::Str(v.clone()));
    }
    Err(BindError::UnsupportedValueType {
        at: raw.to_string(),
    })
}

/// The entity backing a resource. 0.19 stores resources as components on a
/// dedicated entity, so both reads and writes go through this indirection.
fn resource_entity(
    world: &World,
    reg: &TypeRegistration,
    type_path: &str,
) -> Result<Entity, BindError> {
    let absent = || BindError::ResourceNotPresent {
        type_path: type_path.to_string(),
    };
    let component_id = world
        .components()
        .get_id(reg.type_id())
        .ok_or_else(absent)?;
    world
        .resource_entities()
        .get(component_id)
        .ok_or_else(absent)
}

fn reflect_path_error(field: &str, type_path: &str, e: impl std::fmt::Display) -> BindError {
    BindError::ReflectPath {
        field: field.to_string(),
        type_path: type_path.to_string(),
        message: e.to_string(),
    }
}

/// Reads whatever a bind path names: a field of a resource, or a field of a
/// component on the context entity. A component path with no context is an
/// error rather than an empty read.
pub fn read_path(
    world: &World,
    context: Option<Entity>,
    path: &BindPath,
) -> Result<BindValue, BindError> {
    let registry_arc = world
        .get_resource::<AppTypeRegistry>()
        .ok_or(BindError::NoTypeRegistry)?
        .clone();
    let registry = registry_arc.read();
    match path.parse()? {
        ParsedPath::Component { type_path, field } => {
            let subject = context.ok_or_else(|| BindError::NoContext {
                raw: path.raw.clone(),
            })?;
            let reg = lookup_registration(&registry, &type_path, "type")?;
            let reflect_component =
                reg.data::<ReflectComponent>()
                    .ok_or_else(|| BindError::NotAComponent {
                        type_path: type_path.clone(),
                    })?;
            let entity_ref = world
                .get_entity(subject)
                .map_err(|_| BindError::ContextEntityMissing { entity: subject })?;
            let component = reflect_component.reflect(entity_ref).ok_or_else(|| {
                BindError::ContextMissingComponent {
                    type_path: type_path.clone(),
                }
            })?;
            let value = component
                .reflect_path(field.as_str())
                .map_err(|e| reflect_path_error(&field, &type_path, e))?;
            extract(value, &path.raw)
        }
        ParsedPath::Resource { type_path, field } => {
            let not_a_resource = || BindError::NotAResource {
                type_path: type_path.clone(),
            };
            let absent = || BindError::ResourceNotPresent {
                type_path: type_path.clone(),
            };
            let reg = lookup_registration(&registry, &type_path, "type")?;
            if reg.data::<ReflectResource>().is_none() {
                return Err(not_a_resource());
            }
            // Resources are entity-backed in 0.19.
            let reflect_component = reg.data::<ReflectComponent>().ok_or_else(not_a_resource)?;
            let entity = resource_entity(world, reg, &type_path)?;
            let entity_ref = world.get_entity(entity).map_err(|_| absent())?;
            let resource = reflect_component.reflect(entity_ref).ok_or_else(absent)?;
            let value = resource
                .reflect_path(field.as_str())
                .map_err(|e| reflect_path_error(&field, &type_path, e))?;
            extract(value, &path.raw)
        }
    }
}

/// A value on its way into a widget's own field. The same shapes a binding
/// reads, plus the fraction a bar's width is written as.
#[derive(Clone, Debug)]
pub enum WriteValue {
    /// A number, written as pixels when the target is a `Val`.
    F32(f32),
    /// A fraction, written as a percentage when the target is a `Val`.
    Percent(f32),
    /// A bool, which also sets a `Visibility`.
    Bool(bool),
    /// A string.
    Str(String),
}

/// Whether a number a binding read narrows into an integer field without
/// becoming a different number, which `as` would silently allow.
pub(crate) fn fits(v: f32, min: f64, max: f64) -> bool {
    let truncated = f64::from(v).trunc();
    v.is_finite() && truncated >= min && truncated <= max
}

fn set_target(target: &mut dyn PartialReflect, value: &WriteValue) -> Result<bool, BindError> {
    let mismatch =
        |target: &'static str, needs: &'static str| BindError::WriteTypeMismatch { target, needs };
    macro_rules! integer_target {
        ($ty:ty, $name:literal) => {
            if let Some(slot) = target.try_downcast_mut::<$ty>() {
                let (WriteValue::F32(v) | WriteValue::Percent(v)) = value else {
                    return Err(mismatch($name, "numeric"));
                };
                if !fits(*v, <$ty>::MIN as f64, <$ty>::MAX as f64) {
                    return Err(BindError::WriteOutOfRange {
                        target: $name,
                        value: *v,
                    });
                }
                let new = *v as $ty;
                let changed = *slot != new;
                *slot = new;
                return Ok(changed);
            }
        };
    }
    // NaN compares unequal to itself, so the equality guard below would report
    // a change on every evaluation and a NaN `Val` would reach layout.
    let finite = |v: f32, target: &'static str| -> Result<f32, BindError> {
        if v.is_finite() {
            Ok(v)
        } else {
            Err(BindError::WriteOutOfRange { target, value: v })
        }
    };
    if let Some(slot) = target.try_downcast_mut::<Val>() {
        let new = match value {
            WriteValue::Percent(v) => Val::Percent(finite(*v, "Val")? * 100.0),
            WriteValue::F32(v) => Val::Px(finite(*v, "Val")?),
            _ => return Err(mismatch("Val", "numeric")),
        };
        let changed = *slot != new;
        *slot = new;
        return Ok(changed);
    }
    if let Some(slot) = target.try_downcast_mut::<f32>() {
        let new = match value {
            WriteValue::F32(v) | WriteValue::Percent(v) => finite(*v, "f32")?,
            _ => return Err(mismatch("f32", "numeric")),
        };
        let changed = *slot != new;
        *slot = new;
        return Ok(changed);
    }
    if let Some(slot) = target.try_downcast_mut::<f64>() {
        let new = match value {
            WriteValue::F32(v) | WriteValue::Percent(v) => f64::from(finite(*v, "f64")?),
            _ => return Err(mismatch("f64", "numeric")),
        };
        let changed = *slot != new;
        *slot = new;
        return Ok(changed);
    }
    integer_target!(u8, "u8");
    integer_target!(u16, "u16");
    integer_target!(u32, "u32");
    integer_target!(u64, "u64");
    integer_target!(usize, "usize");
    integer_target!(i8, "i8");
    integer_target!(i16, "i16");
    integer_target!(i32, "i32");
    integer_target!(i64, "i64");
    integer_target!(isize, "isize");
    if let Some(slot) = target.try_downcast_mut::<bool>() {
        let WriteValue::Bool(v) = value else {
            return Err(mismatch("bool", "bool"));
        };
        let changed = *slot != *v;
        *slot = *v;
        return Ok(changed);
    }
    if let Some(slot) = target.try_downcast_mut::<String>() {
        let WriteValue::Str(v) = value else {
            return Err(mismatch("String", "string"));
        };
        let changed = slot != v;
        slot.clone_from(v);
        return Ok(changed);
    }
    if let Some(slot) = target.try_downcast_mut::<Visibility>() {
        let WriteValue::Bool(v) = value else {
            return Err(mismatch("Visibility", "bool"));
        };
        let new = if *v {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        let changed = *slot != new;
        *slot = new;
        return Ok(changed);
    }
    Err(BindError::UnsupportedWriteTarget)
}

/// Writes through the reflect path on the bound entity itself; write paths
/// always target the widget, never the context. The write bypasses change
/// detection and flags the component only when the stored value actually
/// changed. Returns `Ok(true)` when it was flagged.
pub fn write_path(
    world: &mut World,
    entity: Entity,
    path: &BindPath,
    value: &WriteValue,
) -> Result<bool, BindError> {
    if path.marker_type().is_some() {
        let write = resolve_write(world, path)?;
        return write_resolved(world, entity, &write, value);
    }
    let ParsedPath::Component { type_path, field } = path.parse()? else {
        return Err(BindError::WritePathNotComponent {
            raw: path.raw.clone(),
        });
    };
    let registry_arc = world
        .get_resource::<AppTypeRegistry>()
        .ok_or(BindError::NoTypeRegistry)?
        .clone();
    let registry = registry_arc.read();
    let reg = lookup_registration(&registry, &type_path, "type")?;
    let reflect_component = reg
        .data::<ReflectComponent>()
        .ok_or_else(|| BindError::NotAComponent {
            type_path: type_path.clone(),
        })?
        .clone();
    drop(registry);
    write_component_field(world, entity, &reflect_component, &type_path, &field, value)
}

/// Shared write tail for a component on an entity and for the entity that backs
/// a resource.
fn write_component_field(
    world: &mut World,
    entity: Entity,
    reflect_component: &ReflectComponent,
    type_path: &str,
    field: &str,
    value: &WriteValue,
) -> Result<bool, BindError> {
    // reflect_mut panics on immutable components, so refuse them up front; the
    // type may hold no ComponentId yet, so force registration to ask at all.
    let component_id = reflect_component.register_component(world);
    if world
        .components()
        .get_info(component_id)
        .is_some_and(|info| !info.mutable())
    {
        return Err(BindError::ImmutableComponent {
            type_path: type_path.to_string(),
        });
    }
    let mut entity_mut = world
        .get_entity_mut(entity)
        .map_err(|_| BindError::EntityMissing { entity })?;
    let mut component = reflect_component
        .reflect_mut(&mut entity_mut)
        .ok_or_else(|| BindError::EntityMissingComponent {
            type_path: type_path.to_string(),
        })?;
    let changed = {
        let target = component
            .bypass_change_detection()
            .reflect_path_mut(field)
            .map_err(|e| reflect_path_error(field, type_path, e))?;
        set_target(target, value)?
    };
    if changed {
        component.set_changed();
    }
    Ok(changed)
}

/// Writes back to whatever a bind path names: a resource, or a component on the
/// context entity. Two-way value bindings feed the data side through here,
/// where `write_path` only ever targets the bound widget itself.
pub fn write_source_path(
    world: &mut World,
    context: Option<Entity>,
    path: &BindPath,
    value: &WriteValue,
) -> Result<bool, BindError> {
    let ParsedPath::Resource { type_path, field } = path.parse()? else {
        let subject = context.ok_or_else(|| BindError::NoContext {
            raw: path.raw.clone(),
        })?;
        return write_path(world, subject, path, value);
    };
    let registry_arc = world
        .get_resource::<AppTypeRegistry>()
        .ok_or(BindError::NoTypeRegistry)?
        .clone();
    let (reflect_component, entity) = {
        let not_a_resource = || BindError::NotAResource {
            type_path: type_path.clone(),
        };
        let registry = registry_arc.read();
        let reg = lookup_registration(&registry, &type_path, "type")?;
        if reg.data::<ReflectResource>().is_none() {
            return Err(not_a_resource());
        }
        let reflect_component = reg
            .data::<ReflectComponent>()
            .ok_or_else(not_a_resource)?
            .clone();
        (reflect_component, resource_entity(world, reg, &type_path)?)
    };
    write_component_field(world, entity, &reflect_component, &type_path, &field, value)
}

/// One read a binding makes, with the lookups already done, so evaluation never
/// parses a path or takes the type registry lock.
pub(crate) struct ResolvedSource {
    /// The entity holding the component, or `None` for a resource, whose
    /// backing entity can move and so is looked up by id at read time.
    pub(crate) source_entity: Option<Entity>,
    /// The component the value lives in, which for a resource path is the
    /// resource's own component. The change-tick gate asks about this id.
    pub(crate) source_component: ComponentId,
    /// The field inside that component, parsed once.
    pub(crate) source_path: FieldPath,
    /// How to reach the component through reflection.
    reflect: ReflectComponent,
    /// The type the path names, for the messages a failed read raises.
    type_path: String,
    /// The field half of the path, for the same reason.
    field: String,
    /// The path as authored, which is how a value of an unsupported type is
    /// reported back to the author.
    raw: String,
}

/// What a `Field` binding's write path landed on.
enum WriteTarget {
    /// A field inside one of the widget's components.
    Field {
        /// The field as authored, for the messages a failed write raises.
        field: String,
        /// The same field, parsed once.
        path: FieldPath,
    },
    /// A whole marker component, which the write puts on and takes off rather
    /// than sets anything inside.
    Marker {
        /// How to build the component the write puts on.
        default: ReflectDefault,
        /// A registry of one, holding this marker's registration and nothing
        /// else; see `write_marker` for why the app's is not handed over.
        registry: TypeRegistry,
    },
}

/// Whether a binding's write lands on exactly what one of its reads takes, so
/// the value it writes is the value it reads next frame.
///
/// Only the direct case is answered here; a cycle through a second binding goes
/// undetected.
pub(crate) fn is_self_cycle(
    entity: Entity,
    source: &ResolvedSource,
    write: &ResolvedWrite,
) -> bool {
    let WriteTarget::Field { path, .. } = &write.target else {
        // A marker holds no value, so no read can name one.
        return false;
    };
    source.source_entity == Some(entity)
        && source.source_component == write.component
        && &source.source_path == path
}

/// The widget field a `Field` binding writes into, looked up once.
pub(crate) struct ResolvedWrite {
    /// The component the write lands in.
    pub(crate) component: ComponentId,
    /// How to reach it on the widget.
    reflect: ReflectComponent,
    /// The type the write path names, for the messages a failed write raises.
    type_path: String,
    /// Which part of that component the write is aimed at.
    target: WriteTarget,
}

impl ResolvedSource {
    /// The entity the read goes to, looking a resource's backing entity up by
    /// id.
    pub(crate) fn entity(&self, world: &World) -> Result<Entity, BindError> {
        match self.source_entity {
            Some(entity) => Ok(entity),
            None => world
                .resource_entities()
                .get(self.source_component)
                .ok_or_else(|| BindError::ResourceNotPresent {
                    type_path: self.type_path.clone(),
                }),
        }
    }

    /// Whether the value behind this read has moved since the evaluator last
    /// ran. A source that is not there at all counts as moved, so the read
    /// still runs and still reports what is missing.
    pub(crate) fn changed(&self, world: &World, last_run: Tick, this_run: Tick) -> bool {
        let Ok(entity) = self.entity(world) else {
            return true;
        };
        let Ok(entity_ref) = world.get_entity(entity) else {
            return true;
        };
        match entity_ref.get_change_ticks_by_id(self.source_component) {
            Some(ticks) => ticks.is_changed(last_run, this_run),
            None => true,
        }
    }
}

/// Reads what a resolved source names. Raises the same errors [`read_path`]
/// raises for the same world, minus the ones the resolver already answered.
pub(crate) fn read_resolved(
    world: &World,
    source: &ResolvedSource,
) -> Result<BindValue, BindError> {
    let entity = source.entity(world)?;
    let missing_component = || {
        if source.source_entity.is_some() {
            BindError::ContextMissingComponent {
                type_path: source.type_path.clone(),
            }
        } else {
            BindError::ResourceNotPresent {
                type_path: source.type_path.clone(),
            }
        }
    };
    let entity_ref = world.get_entity(entity).map_err(|_| {
        if source.source_entity.is_some() {
            BindError::ContextEntityMissing { entity }
        } else {
            BindError::ResourceNotPresent {
                type_path: source.type_path.clone(),
            }
        }
    })?;
    let component = source
        .reflect
        .reflect(entity_ref)
        .ok_or_else(missing_component)?;
    let value = component
        .reflect_path(&source.source_path)
        .map_err(|e| reflect_path_error(&source.field, &source.type_path, e))?;
    extract(value, &source.raw)
}

/// Writes into a resolved widget field, flagging the component only when the
/// stored value actually changed.
pub(crate) fn write_resolved(
    world: &mut World,
    entity: Entity,
    write: &ResolvedWrite,
    value: &WriteValue,
) -> Result<bool, BindError> {
    let (field, path) = match &write.target {
        WriteTarget::Marker { default, registry } => {
            return write_marker(world, entity, write, default, registry, value);
        }
        WriteTarget::Field { field, path } => (field, path),
    };
    // reflect_mut panics on immutable components, so refuse them up front.
    if world
        .components()
        .get_info(write.component)
        .is_some_and(|info| !info.mutable())
    {
        return Err(BindError::ImmutableComponent {
            type_path: write.type_path.clone(),
        });
    }
    let mut entity_mut = world
        .get_entity_mut(entity)
        .map_err(|_| BindError::EntityMissing { entity })?;
    let mut component = write.reflect.reflect_mut(&mut entity_mut).ok_or_else(|| {
        BindError::EntityMissingComponent {
            type_path: write.type_path.clone(),
        }
    })?;
    let changed = {
        let target = component
            .bypass_change_detection()
            .reflect_path_mut(path)
            .map_err(|e| reflect_path_error(field, &write.type_path, e))?;
        set_target(target, value)?
    };
    if changed {
        component.set_changed();
    }
    Ok(changed)
}

/// Puts a marker component on the entity or takes it off, following a bool. A
/// marker already there is left alone, and an immutable one is set and cleared
/// like any other since nothing inside it is touched.
///
/// `ReflectComponent::insert` holds the registry it is given while the insert
/// hooks run, so it is handed a registry of one rather than the app's, leaving
/// the app's unlocked for a hook that asks for it.
fn write_marker(
    world: &mut World,
    entity: Entity,
    write: &ResolvedWrite,
    default: &ReflectDefault,
    registry: &TypeRegistry,
    value: &WriteValue,
) -> Result<bool, BindError> {
    let WriteValue::Bool(wanted) = value else {
        return Err(BindError::MarkerNeedsBool {
            type_path: write.type_path.clone(),
        });
    };
    let present = world
        .get_entity(entity)
        .map_err(|_| BindError::EntityMissing { entity })?
        .contains_id(write.component);
    if *wanted == present {
        return Ok(false);
    }
    if *wanted {
        let built = default.default();
        write.reflect.insert(
            &mut world.entity_mut(entity),
            built.as_partial_reflect(),
            registry,
        );
    } else {
        write.reflect.remove(&mut world.entity_mut(entity));
    }
    Ok(true)
}

/// What a lookup found in the registry, before the world is asked for the
/// component id, so the registry lock is released between the two halves.
struct Found {
    reflect: ReflectComponent,
    type_path: String,
    field: String,
}

fn find_source(registry: &TypeRegistry, path: &BindPath) -> Result<(Found, bool), BindError> {
    match path.parse()? {
        ParsedPath::Component { type_path, field } => {
            let reg = lookup_registration(registry, &type_path, "type")?;
            let reflect = reg
                .data::<ReflectComponent>()
                .ok_or_else(|| BindError::NotAComponent {
                    type_path: type_path.clone(),
                })?
                .clone();
            Ok((
                Found {
                    reflect,
                    type_path,
                    field,
                },
                false,
            ))
        }
        ParsedPath::Resource { type_path, field } => {
            let not_a_resource = || BindError::NotAResource {
                type_path: type_path.clone(),
            };
            let reg = lookup_registration(registry, &type_path, "type")?;
            if reg.data::<ReflectResource>().is_none() {
                return Err(not_a_resource());
            }
            // Resources are entity-backed in 0.19: reach them through the
            // resource entity and the type's ReflectComponent.
            let reflect = reg
                .data::<ReflectComponent>()
                .ok_or_else(not_a_resource)?
                .clone();
            Ok((
                Found {
                    reflect,
                    type_path,
                    field,
                },
                true,
            ))
        }
    }
}

/// Looks up everything a read needs; a component path with no context fails
/// here.
pub(crate) fn resolve_source(
    world: &mut World,
    context: Option<Entity>,
    path: &BindPath,
) -> Result<ResolvedSource, BindError> {
    let registry_arc = world
        .get_resource::<AppTypeRegistry>()
        .ok_or(BindError::NoTypeRegistry)?
        .clone();
    let (found, is_resource) = {
        let registry = registry_arc.read();
        find_source(&registry, path)?
    };
    let source_entity = if is_resource {
        None
    } else {
        Some(context.ok_or_else(|| BindError::NoContext {
            raw: path.raw.clone(),
        })?)
    };
    let source_path = FieldPath::parse(&found.field)
        .map_err(|e| reflect_path_error(&found.field, &found.type_path, e))?;
    Ok(ResolvedSource {
        source_entity,
        source_component: found.reflect.register_component(world),
        source_path,
        reflect: found.reflect,
        type_path: found.type_path,
        field: found.field,
        raw: path.raw.clone(),
    })
}

/// Looks up everything a write needs. Write paths always name a component on
/// the widget itself.
pub(crate) fn resolve_write(
    world: &mut World,
    path: &BindPath,
) -> Result<ResolvedWrite, BindError> {
    if let Some(type_path) = path.marker_type() {
        let type_path = type_path.to_string();
        return resolve_marker_write(world, type_path);
    }
    let ParsedPath::Component { type_path, field } = path.parse()? else {
        return Err(BindError::WritePathNotComponent {
            raw: path.raw.clone(),
        });
    };
    let registry_arc = world
        .get_resource::<AppTypeRegistry>()
        .ok_or(BindError::NoTypeRegistry)?
        .clone();
    let reflect = {
        let registry = registry_arc.read();
        let reg = lookup_registration(&registry, &type_path, "type")?;
        reg.data::<ReflectComponent>()
            .ok_or_else(|| BindError::NotAComponent {
                type_path: type_path.clone(),
            })?
            .clone()
    };
    let parsed = FieldPath::parse(&field).map_err(|e| reflect_path_error(&field, &type_path, e))?;
    Ok(ResolvedWrite {
        component: reflect.register_component(world),
        reflect,
        type_path,
        target: WriteTarget::Field {
            field,
            path: parsed,
        },
    })
}

/// Looks up a write aimed at a whole marker component.
///
/// A path with no field is only a marker write when the type has no fields
/// either: `Node` without `.width` is a path missing its field half, not an
/// instruction to strip the layout off a live widget. Only the registry can
/// answer that, so the check is here rather than at parse time. Reflection must
/// also be able to build one from nothing, since that is what `true` means.
fn resolve_marker_write(world: &mut World, type_path: String) -> Result<ResolvedWrite, BindError> {
    let registry_arc = world
        .get_resource::<AppTypeRegistry>()
        .ok_or(BindError::NoTypeRegistry)?
        .clone();
    let (reflect, default, alone) = {
        let registry = registry_arc.read();
        let reg = lookup_registration(&registry, &type_path, "type")?;
        let fieldless = matches!(
            reg.type_info(),
            bevy::reflect::TypeInfo::Struct(info) if info.field_len() == 0
        );
        if !fieldless {
            return Err(BindError::MalformedPath {
                raw: type_path,
                reason: "expected 'Type.field'",
            });
        }
        let reflect = reg
            .data::<ReflectComponent>()
            .ok_or_else(|| BindError::NotAComponent {
                type_path: type_path.clone(),
            })?
            .clone();
        let default = reg
            .data::<ReflectDefault>()
            .ok_or_else(|| BindError::MarkerNotDefaultable {
                type_path: type_path.clone(),
            })?
            .clone();
        let mut alone = TypeRegistry::empty();
        alone.add_registration(reg.clone());
        (reflect, default, alone)
    };
    Ok(ResolvedWrite {
        component: reflect.register_component(world),
        reflect,
        type_path,
        target: WriteTarget::Marker {
            default,
            registry: alone,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BindContext;

    #[derive(Component, Reflect, Default)]
    #[reflect(Component)]
    struct Health {
        current: f32,
        max: f32,
    }

    #[derive(Resource, Reflect, Default)]
    #[reflect(Resource)]
    struct AudioSettings {
        master: f32,
    }

    fn world_with_types() -> World {
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        {
            let mut r = registry.write();
            r.register::<Health>();
            r.register::<AudioSettings>();
        }
        world.insert_resource(registry);
        world
    }

    #[test]
    fn context_walks_up_hierarchy() {
        let mut world = world_with_types();
        let subject = world.spawn_empty().id();
        let root = world.spawn(BindContext(subject)).id();
        let mid = world.spawn(ChildOf(root)).id();
        let leaf = world.spawn(ChildOf(mid)).id();
        assert_eq!(resolve_context(&world, leaf), Some(subject));
        assert_eq!(resolve_context(&world, root), Some(subject));
        let orphan = world.spawn_empty().id();
        assert_eq!(resolve_context(&world, orphan), None);
    }

    /// A context on the root, `below` entities under it, and the deepest one
    /// returned.
    fn chain_under_a_context(world: &mut World, below: usize) -> Entity {
        let subject = world.spawn_empty().id();
        let mut current = world.spawn(BindContext(subject)).id();
        for _ in 0..below {
            current = world.spawn(ChildOf(current)).id();
        }
        current
    }

    #[test]
    fn a_context_at_the_edge_of_the_walk_still_resolves() {
        let mut world = world_with_types();
        let leaf = chain_under_a_context(&mut world, MAX_CONTEXT_WALK - 1);
        assert!(
            resolve_context(&world, leaf).is_some(),
            "a context on the last entity the search looks at is still found",
        );
    }

    #[test]
    fn a_context_past_the_edge_of_the_walk_does_not() {
        let mut world = world_with_types();
        let leaf = chain_under_a_context(&mut world, MAX_CONTEXT_WALK);
        assert_eq!(
            resolve_context(&world, leaf),
            None,
            "one entity further up is out of reach",
        );
    }

    #[test]
    fn reads_component_field_by_short_path() {
        let mut world = world_with_types();
        let subject = world
            .spawn(Health {
                current: 40.0,
                max: 100.0,
            })
            .id();
        let v = read_path(&world, Some(subject), &BindPath::new("Health.current")).unwrap();
        assert_eq!(v, BindValue::F32(40.0));
    }

    #[test]
    fn reads_resource_field_without_context() {
        let mut world = world_with_types();
        world.insert_resource(AudioSettings { master: 0.7 });
        let v = read_path(&world, None, &BindPath::new("Res(AudioSettings).master")).unwrap();
        assert_eq!(v, BindValue::F32(0.7));
    }

    #[test]
    fn missing_component_is_an_error_not_a_panic() {
        let mut world = world_with_types();
        let subject = world.spawn_empty().id();
        let err = read_path(&world, Some(subject), &BindPath::new("Health.current")).unwrap_err();
        assert!(
            matches!(err, BindError::ContextMissingComponent { .. }),
            "wrong branch: {err}"
        );
        assert!(err.to_string().contains("lacks"), "{err}");
    }

    #[test]
    fn missing_context_is_an_error_not_a_panic() {
        let world = world_with_types();
        let err = read_path(&world, None, &BindPath::new("Health.current")).unwrap_err();
        assert!(
            matches!(err, BindError::NoContext { .. }),
            "wrong branch: {err}"
        );
        assert!(err.to_string().contains("no context"), "{err}");
    }

    #[test]
    fn despawned_context_entity_is_an_error_not_a_panic() {
        let mut world = world_with_types();
        let subject = world
            .spawn(Health {
                current: 40.0,
                max: 100.0,
            })
            .id();
        assert!(world.despawn(subject));
        let err = read_path(&world, Some(subject), &BindPath::new("Health.current")).unwrap_err();
        assert!(
            matches!(err, BindError::ContextEntityMissing { entity } if entity == subject),
            "wrong branch: {err}"
        );
        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    mod audio {
        use bevy::prelude::*;

        #[derive(Component, Reflect, Default)]
        #[reflect(Component)]
        pub struct Volume(pub f32);
    }

    mod render {
        use bevy::prelude::*;

        #[derive(Component, Reflect, Default)]
        #[reflect(Component)]
        pub struct Volume(pub f32);
    }

    #[test]
    fn a_short_type_path_two_types_share_reads_as_ambiguous() {
        let mut registry = TypeRegistry::empty();
        registry.register::<audio::Volume>();
        registry.register::<render::Volume>();
        let err = lookup_registration(&registry, "Volume", "type").unwrap_err();
        assert!(
            matches!(err, BindError::AmbiguousType { .. }),
            "wrong branch: {err}"
        );
        let message = err.to_string();
        assert!(
            message.starts_with("ambiguous short type path 'Volume'"),
            "{message}"
        );
        assert!(message.contains("audio::Volume"), "{message}");
        assert!(message.contains("render::Volume"), "{message}");
    }

    #[test]
    fn an_unregistered_type_still_reads_as_unknown() {
        let registry = TypeRegistry::empty();
        let err = lookup_registration(&registry, "Volume", "type").unwrap_err();
        assert!(
            matches!(err, BindError::UnknownType { .. }),
            "wrong branch: {err}"
        );
        assert_eq!(err.to_string(), "unknown type 'Volume'");
    }

    #[test]
    fn missing_type_registry_is_an_error_not_a_panic() {
        let world = World::new();
        let err = read_path(&world, None, &BindPath::new("Health.current")).unwrap_err();
        assert_eq!(err, BindError::NoTypeRegistry);
        assert_eq!(err.to_string(), "no type registry");
    }
}
