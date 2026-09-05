//! The `Bindings` inspector card: a row per binding, with pickers for the
//! places it reads and writes.
//!
//! Every control builds the whole new `Bindings` value and commits it through
//! `crate::commands::field_edit_commit` with an empty field path, so one
//! gesture is one document patch and one undo entry. Pickers always write
//! full type paths; only the display shortens.

use bevy::ecs::component::ComponentInfo;
use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent, ReflectResource};
use bevy::feathers::controls::FeathersCheckbox;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::reflect::{TypeInfo, serde::TypedReflectSerializer};
use bevy::ui::{Checked, InteractionDisabled};
use bevy::ui_widgets::ValueChange;
use jackdaw_api::prelude::*;
use jackdaw_bind::{BindError, BindPath, Binding, Bindings, ParsedPath};
use jackdaw_feathers::{
    button::{ButtonClickEvent, ButtonProps, ButtonSize, ButtonVariant, button},
    combobox::{
        ComboBoxChangeEvent, ComboBoxOptionData, ComboBoxSelectedIndex, combobox_with_label,
        combobox_with_selected,
    },
    field_row::{FieldRowProps, spawn_field_row},
    icons::Icon,
    text_edit::{TextEditCommitEvent, TextEditProps, text_edit},
    tokens,
    tooltip::Tooltip,
    variant_edit::{VariantComboBox, VariantDefinition, VariantEditProps, variant_edit},
};
use jackdaw_scene_types::PropertyValue;

use super::material_card_routing::RefreshInspectorCardBody;

/// The reflect path of the component this card stands in for, and the card's
/// `ComponentDisplayTypePath`.
pub fn bindings_type_path() -> &'static str {
    <Bindings as bevy::reflect::TypePath>::type_path()
}

/// Undo label when a binding edit lands on more than one selected entity.
const GROUP_LABEL: &str = "Edit bindings on multiple entities";

/// Room a type or field picker asks for on a path row.
const PICKER_WIDTH: f32 = 96.0;
/// How far a picker gives way before the row wraps it onto its own line.
const PICKER_MIN_WIDTH: f32 = 72.0;

/// Node for a path row's type and field pickers, which share a line and give
/// way together. The floor comes from the basis and the row's wrap rather than
/// a `min_width`, which would clip the picker on a very narrow panel.
fn picker_node() -> Node {
    Node {
        flex_grow: 1.0,
        flex_shrink: 1.0,
        flex_basis: Val::Px(PICKER_WIDTH),
        min_width: Val::Px(0.0),
        ..Default::default()
    }
}

/// Marker on the card body, carrying the entity it was filled for.
#[derive(Component)]
pub struct BindingsCardBody(pub Entity);

/// Marker on a binding row's summary line.
#[derive(Component)]
pub struct BindingRowLabel;

/// The footer's Add Binding menu.
#[derive(Component)]
pub struct AddBindingMenu {
    source: Entity,
}

/// What a picker on this card would write, in option order.
#[derive(Component)]
pub struct BindingOptions(Vec<String>);

/// The five shapes a binding can take.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BindKind {
    Field,
    Text,
    Visible,
    Value,
    Action,
}

impl BindKind {
    pub const ALL: [BindKind; 5] = [
        BindKind::Field,
        BindKind::Text,
        BindKind::Visible,
        BindKind::Value,
        BindKind::Action,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BindKind::Field => "Field",
            BindKind::Text => "Text",
            BindKind::Visible => "Visible",
            BindKind::Value => "Value",
            BindKind::Action => "Action",
        }
    }

    fn icon(self) -> Icon {
        match self {
            BindKind::Field => Icon::SlidersHorizontal,
            BindKind::Text => Icon::Type,
            BindKind::Visible => Icon::Eye,
            BindKind::Value => Icon::Gauge,
            BindKind::Action => Icon::Zap,
        }
    }

    pub fn of(binding: &Binding) -> Self {
        match binding {
            Binding::Field { .. } => BindKind::Field,
            Binding::Text { .. } => BindKind::Text,
            Binding::Visible { .. } => BindKind::Visible,
            Binding::Value { .. } => BindKind::Value,
            Binding::Action { .. } => BindKind::Action,
        }
    }

    /// A binding of this kind with nothing chosen yet.
    fn new_binding(self, carried: Option<BindPath>) -> Binding {
        let carried = carried.unwrap_or_default();
        match self {
            BindKind::Field => Binding::Field {
                read: vec![carried],
                via: None,
                write: BindPath::default(),
                as_percent: false,
            },
            BindKind::Text => Binding::Text {
                format: "{}".to_string(),
                args: vec![carried],
            },
            BindKind::Visible => Binding::Visible {
                read: carried,
                via: None,
            },
            BindKind::Value => Binding::Value {
                with: carried,
                two_way: false,
            },
            BindKind::Action => Binding::Action {
                event: String::new(),
                fields: Vec::new(),
            },
        }
    }
}

/// Where in a binding one `BindPath` lives. `Read(0)` covers the single path
/// of a `Visible` or `Value` as well as the first read of a `Field` or `Text`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PathSlot {
    Read(usize),
    Write,
    /// An `Action`'s mapping for one of its event's fields. The field name is
    /// carried by the control rather than the slot, since mapping is by name.
    EventField,
}

/// One control on the card, named by what it edits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BindControl {
    /// The kind picker; changing it rebuilds the binding.
    Kind,
    /// Component or resource, for a path that could be either.
    PathSource(PathSlot),
    /// The type half of a path, written as a full type path.
    PathType(PathSlot),
    /// The field half of a path.
    PathField(PathSlot),
    /// The transform function, picked from the schema.
    Via,
    /// The transform function as free text, when the schema names none.
    ViaText,
    Format,
    AsPercent,
    TwoWay,
    Event,
    AddRead,
    RemoveRead(usize),
    MoveUp,
    MoveDown,
    Remove,
}

/// The card's handle on one control: which binding it belongs to and what it
/// edits.
#[derive(Component, Clone)]
pub struct BindingControl {
    source: Entity,
    binding: usize,
    control: BindControl,
    /// The event field name for a `PathSlot::EventField` control; empty otherwise.
    event_field: String,
}

impl BindingControl {
    pub fn binding(&self) -> usize {
        self.binding
    }

    pub fn control(&self) -> BindControl {
        self.control
    }

    pub fn source(&self) -> Entity {
        self.source
    }

    fn new(source: Entity, binding: usize, control: BindControl) -> Self {
        Self {
            source,
            binding,
            control,
            event_field: String::new(),
        }
    }

    fn for_event_field(source: Entity, binding: usize, control: BindControl, name: &str) -> Self {
        Self {
            source,
            binding,
            control,
            event_field: name.to_string(),
        }
    }
}

/// Every value a card picker would write, in option order.
pub fn combo_option_values(world: &World, combo: Entity) -> Vec<String> {
    world
        .get::<BindingOptions>(combo)
        .map(|options| options.0.clone())
        .unwrap_or_default()
}

/// The path at `slot`, or `None` when the binding has no such slot.
fn path_at<'a>(binding: &'a Binding, slot: PathSlot, event_field: &str) -> Option<&'a BindPath> {
    match (binding, slot) {
        (Binding::Field { read, .. }, PathSlot::Read(i)) => read.get(i),
        (Binding::Field { write, .. }, PathSlot::Write) => Some(write),
        (Binding::Text { args, .. }, PathSlot::Read(i)) => args.get(i),
        (Binding::Visible { read, .. }, PathSlot::Read(0)) => Some(read),
        (Binding::Value { with, .. }, PathSlot::Read(0)) => Some(with),
        (Binding::Action { fields, .. }, PathSlot::EventField) => fields
            .iter()
            .find(|(name, _)| name == event_field)
            .map(|(_, path)| path),
        _ => None,
    }
}

/// Put `raw` at `slot`. An empty raw on an event-field mapping removes the
/// mapping rather than authoring a path that parses to nothing.
fn set_path(binding: &mut Binding, slot: PathSlot, event_field: &str, raw: String) {
    match (binding, slot) {
        (Binding::Field { read, .. }, PathSlot::Read(i)) => {
            if let Some(path) = read.get_mut(i) {
                path.raw = raw;
            }
        }
        (Binding::Field { write, .. }, PathSlot::Write) => write.raw = raw,
        (Binding::Text { args, .. }, PathSlot::Read(i)) => {
            if let Some(path) = args.get_mut(i) {
                path.raw = raw;
            }
        }
        (Binding::Visible { read, .. }, PathSlot::Read(0)) => read.raw = raw,
        (Binding::Value { with, .. }, PathSlot::Read(0)) => with.raw = raw,
        (Binding::Action { fields, .. }, PathSlot::EventField) => {
            if raw.is_empty() {
                fields.retain(|(name, _)| name != event_field);
            } else if let Some((_, path)) = fields.iter_mut().find(|(name, _)| name == event_field)
            {
                path.raw = raw;
            } else {
                fields.push((event_field.to_string(), BindPath::new(raw)));
            }
        }
        _ => {}
    }
}

/// Every path a binding holds, with the slot it sits in and (for an event
/// mapping) the field it fills.
fn paths_of(binding: &Binding) -> Vec<(PathSlot, String, String)> {
    let read_slots = |paths: &Vec<BindPath>| -> Vec<(PathSlot, String, String)> {
        paths
            .iter()
            .enumerate()
            .map(|(i, path)| (PathSlot::Read(i), String::new(), path.raw.clone()))
            .collect()
    };
    match binding {
        Binding::Field { read, write, .. } => {
            let mut out = read_slots(read);
            out.push((PathSlot::Write, String::new(), write.raw.clone()));
            out
        }
        Binding::Text { args, .. } => read_slots(args),
        Binding::Visible { read, .. } => {
            vec![(PathSlot::Read(0), String::new(), read.raw.clone())]
        }
        Binding::Value { with, .. } => vec![(PathSlot::Read(0), String::new(), with.raw.clone())],
        Binding::Action { fields, .. } => fields
            .iter()
            .map(|(name, path)| (PathSlot::EventField, name.clone(), path.raw.clone()))
            .collect(),
    }
}

/// The `via` a binding runs its reads through, when its kind has one.
fn via_of(binding: &Binding) -> Option<&String> {
    match binding {
        Binding::Field { via, .. } | Binding::Visible { via, .. } => via.as_ref(),
        _ => None,
    }
}

/// How many arguments this binding's `via` would be called with.
fn via_arity(binding: &Binding) -> usize {
    match binding {
        Binding::Field { read, .. } => read.len(),
        Binding::Visible { .. } => 1,
        _ => 0,
    }
}

/// Split a raw path into its resource flag, its type path and its field. A
/// marker path names a type and no field.
fn decompose(raw: &str) -> (bool, String, String) {
    if let Some(marker) = BindPath::new(raw).marker_type() {
        return (false, marker.to_string(), String::new());
    }
    match BindPath::new(raw).parse() {
        Ok(ParsedPath::Component { type_path, field }) => (false, type_path, field),
        Ok(ParsedPath::Resource { type_path, field }) => (true, type_path, field),
        Err(_) => (false, String::new(), String::new()),
    }
}

/// Put a path back together from its three halves. Half a path composes to an
/// empty raw rather than a `Type.` that only parses to an error.
fn compose(is_resource: bool, type_path: &str, field: &str) -> String {
    if type_path.is_empty() || field.is_empty() {
        return String::new();
    }
    if is_resource {
        format!("Res({type_path}).{field}")
    } else {
        format!("{type_path}.{field}")
    }
}

/// The same, for a type the picker has an entry for. A marker's type path is
/// already the finished path.
fn compose_option(
    option: Option<&TypeOption>,
    is_resource: bool,
    type_path: &str,
    field: &str,
) -> String {
    if option.is_some_and(|option| option.marker) {
        return type_path.to_string();
    }
    compose(is_resource, type_path, field)
}

/// One type a path may name. Everything in the list can be validated against;
/// only the `pickable` entries belong in a dropdown.
struct TypeOption {
    type_path: String,
    short_name: String,
    fields: Vec<String>,
    pickable: bool,
    /// A component with no fields, which a `Field` binding writes by putting it
    /// on and taking it off.
    marker: bool,
    /// This entity already carries the component, or its document node does.
    /// Write targets are ordered by it; the read lists ignore it.
    on_widget: bool,
}

struct EventOption {
    type_path: String,
    short_name: String,
    /// The fields a binding has to fill, excluding the dispatcher's own `entity`.
    fields: Vec<String>,
    fills_gaps: bool,
}

struct FunctionOption {
    name: String,
    short_name: String,
    arity: usize,
}

/// What the card knows about the project and the world when building pickers.
struct SchemaCtx {
    components: Vec<TypeOption>,
    resources: Vec<TypeOption>,
    write_targets: Vec<TypeOption>,
    events: Vec<EventOption>,
    functions: Vec<FunctionOption>,
    /// True once the editor has a project schema; before that no game type can
    /// be called unknown.
    schema_known: bool,
}

fn short_of(type_path: &str) -> String {
    type_path
        .rsplit("::")
        .next()
        .unwrap_or(type_path)
        .to_string()
}

/// The field names of a reflected type, as a reflect path addresses them.
fn fields_of(info: &TypeInfo) -> Vec<String> {
    match info {
        TypeInfo::Struct(s) => s.iter().map(|f| f.name().to_string()).collect(),
        TypeInfo::TupleStruct(s) => (0..s.field_len()).map(|i| i.to_string()).collect(),
        _ => Vec::new(),
    }
}

impl SchemaCtx {
    fn gather(world: &World, source: Entity) -> Self {
        let mut ctx = Self {
            components: Vec::new(),
            resources: Vec::new(),
            write_targets: Vec::new(),
            events: Vec::new(),
            functions: Vec::new(),
            schema_known: false,
        };
        let Some(registry) = world.get_resource::<AppTypeRegistry>().cloned() else {
            warn!("the bindings picker has no type registry, so it offers no types");
            return ctx;
        };
        let registry = registry.read();

        ctx.gather_project(world);
        ctx.gather_native(world, source, &registry);
        ctx.gather_authored_project(world, source);

        let by_name = |a: &TypeOption, b: &TypeOption| {
            b.pickable
                .cmp(&a.pickable)
                .then_with(|| a.short_name.cmp(&b.short_name))
        };
        ctx.components.sort_by(by_name);
        ctx.resources.sort_by(by_name);
        // What the widget carries comes first; a marker binding exists to put
        // one on, so markers it does not carry still follow.
        ctx.write_targets.sort_by(|a, b| {
            b.on_widget
                .cmp(&a.on_widget)
                .then_with(|| a.short_name.cmp(&b.short_name))
        });
        ctx.events.sort_by(|a, b| a.short_name.cmp(&b.short_name));
        ctx.functions
            .sort_by(|a, b| a.short_name.cmp(&b.short_name));
        ctx
    }

    /// The game's own types, as the out-of-process extractor reported them.
    fn gather_project(&mut self, world: &World) {
        let Some(project) = world.get_resource::<crate::project_types::ProjectTypes>() else {
            return;
        };
        for schema in project.components() {
            self.schema_known = true;
            if schema.hidden {
                continue;
            }
            self.components.push(TypeOption {
                type_path: schema.type_path.clone(),
                short_name: schema.short_name.clone(),
                fields: schema.fields.iter().map(|f| f.name.clone()).collect(),
                pickable: true,
                marker: false,
                on_widget: false,
            });
        }
        for schema in project.resources() {
            self.schema_known = true;
            self.resources.push(TypeOption {
                type_path: schema.type_path.clone(),
                short_name: schema.short_name.clone(),
                fields: schema.fields.iter().map(|f| f.name.clone()).collect(),
                pickable: true,
                marker: false,
                on_widget: false,
            });
        }
        for schema in project.events() {
            self.schema_known = true;
            // The dispatcher builds its value field by field and cannot choose
            // a variant, so an enum event would never fire.
            if schema.kind != jackdaw_schema::TypeKind::Struct {
                continue;
            }
            self.events.push(EventOption {
                type_path: schema.type_path.clone(),
                short_name: schema.short_name.clone(),
                // The dispatcher fills entity fields from the widget's context,
                // and a bind value cannot carry an entity anyway.
                fields: schema
                    .fields
                    .iter()
                    .filter(|f| !schema.entity_fields.contains(&f.name))
                    .map(|f| f.name.clone())
                    .collect(),
                fills_gaps: schema.fills_gaps,
            });
        }
        for schema in project.functions() {
            self.schema_known = true;
            // The evaluator builds its arguments owned and takes only an owned
            // return, so anything else is unusable.
            if !schema.callable_by_value() {
                continue;
            }
            self.functions.push(FunctionOption {
                short_name: short_of(&schema.name),
                name: schema.name.clone(),
                arity: schema.arg_type_paths.len(),
            });
        }
    }

    /// The editor's own registrations. Everything registered can be validated
    /// against, but only what the bound entity carries reaches a dropdown.
    fn gather_native(
        &mut self,
        world: &World,
        source: Entity,
        registry: &bevy::reflect::TypeRegistry,
    ) {
        let mut on_hand: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut subjects = vec![source];
        if let Some(context) = jackdaw_bind::resolve_context(world, source) {
            subjects.push(context);
        }
        for (index, entity) in subjects.iter().enumerate() {
            let Ok(entity_ref) = world.get_entity(*entity) else {
                continue;
            };
            for component_id in entity_ref.archetype().iter_components() {
                let Some(type_id) = world
                    .components()
                    .get_info(component_id)
                    .and_then(ComponentInfo::type_id)
                else {
                    continue;
                };
                let Some(registration) = registry.get(type_id) else {
                    continue;
                };
                let type_path = registration.type_info().type_path_table().path();
                if type_path == bindings_type_path()
                    || crate::scene_io::should_skip_component(type_path)
                {
                    continue;
                }
                let fields = fields_of(registration.type_info());
                if fields.is_empty() {
                    continue;
                }
                on_hand.insert(type_path.to_string());
                // A `Field` binding writes to the widget it sits on, so the
                // write picker is that entity's own archetype.
                if index == 0 {
                    self.write_targets.push(TypeOption {
                        type_path: type_path.to_string(),
                        short_name: short_of(type_path),
                        fields,
                        pickable: true,
                        marker: false,
                        on_widget: true,
                    });
                }
            }
        }

        for registration in registry.iter() {
            let type_path = registration.type_info().type_path_table().path();
            if type_path == bindings_type_path() {
                continue;
            }
            let fields = fields_of(registration.type_info());
            if fields.is_empty() {
                self.gather_marker(registration, type_path);
                continue;
            }
            if registration.data::<ReflectComponent>().is_some()
                && !self.components.iter().any(|o| o.type_path == type_path)
            {
                self.components.push(TypeOption {
                    type_path: type_path.to_string(),
                    short_name: short_of(type_path),
                    fields: fields.clone(),
                    pickable: on_hand.contains(type_path),
                    marker: false,
                    on_widget: false,
                });
            }
            if registration.data::<ReflectResource>().is_some()
                && !self.resources.iter().any(|o| o.type_path == type_path)
            {
                self.resources.push(TypeOption {
                    type_path: type_path.to_string(),
                    short_name: short_of(type_path),
                    fields,
                    pickable: false,
                    marker: false,
                    on_widget: false,
                });
            }
        }
    }

    /// Offer a fieldless component as a marker write target, whether or not the
    /// widget carries one. Reflection has to be able to build it and the save
    /// policy has to keep it, or the binding would author cleanly and fail later.
    fn gather_marker(&mut self, registration: &bevy::reflect::TypeRegistration, type_path: &str) {
        let empty_struct = matches!(
            registration.type_info(),
            TypeInfo::Struct(info) if info.field_len() == 0
        );
        if !empty_struct
            || registration.data::<ReflectComponent>().is_none()
            || registration
                .data::<bevy::reflect::prelude::ReflectDefault>()
                .is_none()
            || crate::scene_io::should_skip_component(type_path)
            || self
                .write_targets
                .iter()
                .any(|option| option.type_path == type_path)
        {
            return;
        }
        self.write_targets.push(TypeOption {
            type_path: type_path.to_string(),
            short_name: short_of(type_path),
            fields: Vec::new(),
            pickable: true,
            marker: true,
            on_widget: false,
        });
    }

    /// The project components the document authored on this entity. A project
    /// component is never a real ECS component in the editor, so the archetype
    /// walk cannot see it and it has to be collected off the node.
    fn gather_authored_project(&mut self, world: &World, source: Entity) {
        let (Some(ast), Some(project)) = (
            world.get_resource::<jackdaw_bsn::SceneBsnAst>(),
            world.get_resource::<crate::project_types::ProjectTypes>(),
        ) else {
            return;
        };
        let Some(node) = ast.ast_for(source) else {
            return;
        };
        for type_path in ast.component_type_paths(node) {
            let Some(schema) = project.component(&type_path) else {
                continue;
            };
            if self
                .write_targets
                .iter()
                .any(|option| option.type_path == type_path)
            {
                continue;
            }
            self.write_targets.push(TypeOption {
                type_path: schema.type_path.clone(),
                short_name: schema.short_name.clone(),
                fields: schema.fields.iter().map(|f| f.name.clone()).collect(),
                pickable: true,
                on_widget: true,
                // `fills_gaps` is the schema's word for what the native side
                // asks of `ReflectDefault`: without it the game cannot build
                // the component either.
                marker: schema.fields.is_empty()
                    && schema.kind == jackdaw_schema::TypeKind::Struct
                    && schema.fills_gaps,
            });
        }
    }

    /// The list a path in `slot` is named from.
    fn types(&self, slot: PathSlot) -> &[TypeOption] {
        match slot {
            PathSlot::Write => &self.write_targets,
            _ => &self.components,
        }
    }

    /// The type a path names, matched on the full path first and the short name
    /// second, the two spellings a `BindPath` accepts.
    fn lookup<'a>(list: &'a [TypeOption], type_path: &str) -> Option<&'a TypeOption> {
        list.iter()
            .find(|option| option.type_path == type_path)
            .or_else(|| list.iter().find(|option| option.short_name == type_path))
    }

    /// Every list a path in `slot` may legitimately name. Wider than what the
    /// picker offers: a component may be added by game code or an inherited
    /// template long before the binding runs.
    fn judging_lists(&self, slot: PathSlot, is_resource: bool) -> Vec<&[TypeOption]> {
        if is_resource {
            return vec![&self.resources];
        }
        match slot {
            PathSlot::Write => vec![&self.write_targets, &self.components],
            _ => vec![&self.components],
        }
    }

    /// The type `type_path` names across several lists, first match wins.
    fn lookup_across<'a>(lists: &[&'a [TypeOption]], type_path: &str) -> Option<&'a TypeOption> {
        lists.iter().find_map(|list| Self::lookup(list, type_path))
    }

    fn event(&self, type_path: &str) -> Option<&EventOption> {
        self.events
            .iter()
            .find(|option| option.type_path == type_path)
            .or_else(|| {
                self.events
                    .iter()
                    .find(|option| option.short_name == type_path)
            })
    }

    /// The type list and field list behind one raw path.
    fn resolve(&self, raw: &str, slot: PathSlot) -> (&[TypeOption], Vec<String>) {
        let (is_resource, type_path, _) = decompose(raw);
        let types: &[TypeOption] = if is_resource {
            &self.resources
        } else {
            self.types(slot)
        };
        let fields = Self::lookup(types, &type_path)
            .map(|option| option.fields.clone())
            .unwrap_or_default();
        (types, fields)
    }
}

impl SchemaCtx {
    /// Why a path would not resolve. An empty path is unfinished, not wrong.
    fn path_error(&self, raw: &str, slot: PathSlot) -> Option<BindError> {
        if raw.is_empty() {
            return None;
        }
        if let Some(marker) = BindPath::new(raw).marker_type() {
            return self.marker_error(marker, slot);
        }
        let (type_path, field, lists) = match BindPath::new(raw).parse() {
            Ok(ParsedPath::Component { type_path, field }) => {
                let lists = self.judging_lists(slot, false);
                (type_path, field, lists)
            }
            Ok(ParsedPath::Resource { type_path, field }) => {
                let lists = self.judging_lists(slot, true);
                (type_path, field, lists)
            }
            Err(error) => return Some(error),
        };

        // A short name shared by several types is a spelling the resolver
        // refuses, not an unknown type. The same type can sit in two lists, so
        // candidates are counted by distinct path.
        if !type_path.contains("::") {
            let mut candidates: Vec<String> = lists
                .iter()
                .flat_map(|list| list.iter())
                .filter(|option| option.short_name == type_path)
                .map(|option| option.type_path.clone())
                .collect();
            candidates.sort();
            candidates.dedup();
            if candidates.len() > 1 {
                return Some(BindError::AmbiguousType {
                    type_path,
                    candidates,
                });
            }
        }

        let Some(option) = Self::lookup_across(&lists, &type_path) else {
            // With no project schema, nothing can be called missing.
            if !self.schema_known {
                return None;
            }
            return Some(BindError::UnknownType {
                noun: "type",
                type_path,
            });
        };
        // Only the leading segment is checked; a nested path resolves the rest
        // at runtime.
        let head = field.split('.').next().unwrap_or(&field);
        if !option.fields.is_empty() && !option.fields.iter().any(|name| name == head) {
            return Some(BindError::ReflectPath {
                field: head.to_string(),
                type_path,
                message: "no field".to_string(),
            });
        }
        None
    }

    /// Why a path naming a whole component would not work. Only a write can be
    /// spelled that way.
    fn marker_error(&self, type_path: &str, slot: PathSlot) -> Option<BindError> {
        if slot != PathSlot::Write {
            return Some(BindError::MalformedPath {
                raw: type_path.to_string(),
                reason: "expected 'Type.field'",
            });
        }
        match SchemaCtx::lookup(&self.write_targets, type_path) {
            Some(option) if option.marker => None,
            // A known type that is not a marker was named without its field.
            Some(_) => Some(BindError::MalformedPath {
                raw: type_path.to_string(),
                reason: "expected 'Type.field'",
            }),
            // With no project schema, nothing can be called missing.
            None if !self.schema_known => None,
            None => Some(BindError::UnknownType {
                noun: "type",
                type_path: type_path.to_string(),
            }),
        }
    }

    /// The first thing wrong with a binding, in reading order.
    fn binding_error(&self, binding: &Binding) -> Option<BindError> {
        if let Binding::Action { event, fields } = binding {
            if !event.is_empty() && self.schema_known && self.event(event).is_none() {
                return Some(BindError::UnknownType {
                    noun: "event type",
                    type_path: event.clone(),
                });
            }
            if let Some(schema) = self.event(event)
                && let Some((name, _)) = fields
                    .iter()
                    .find(|(name, _)| !schema.fields.iter().any(|field| field == name))
            {
                return Some(BindError::UnknownEventField {
                    field: name.clone(),
                });
            }
        }
        for (slot, _, raw) in paths_of(binding) {
            if let Some(error) = self.path_error(&raw, slot) {
                return Some(error);
            }
        }
        if let Some(via) = via_of(binding)
            && !via.is_empty()
            && !self.functions.is_empty()
            && !self
                .functions
                .iter()
                .any(|function| function.name == *via || function.short_name == *via)
        {
            return Some(BindError::UnknownFunction { name: via.clone() });
        }
        shape_error(binding)
    }

    /// The complaint for one unmapped field of an event that cannot fill its own
    /// gaps, the one unfinished binding known to fail at dispatch.
    fn gap_warning(&self, binding: &Binding, field: &str) -> Option<BindError> {
        let Binding::Action { event, fields } = binding else {
            return None;
        };
        let schema = self.event(event)?;
        if schema.fills_gaps || fields.iter().any(|(mapped, _)| mapped == field) {
            return None;
        }
        Some(BindError::UnfillableEvent {
            event_path: schema.short_name.clone(),
            field: field.to_string(),
        })
    }
}

/// What is wrong with a binding's own shape, whatever the schema says. Needs no
/// registry, so it catches mistakes before a project is built.
fn shape_error(binding: &Binding) -> Option<BindError> {
    match binding {
        Binding::Field { read, via, .. } => {
            if read.is_empty() {
                return Some(BindError::NoReads);
            }
            let has_via = via.as_ref().is_some_and(|via| !via.is_empty());
            if read.len() > 1 && !has_via {
                return Some(BindError::MultipleReadsNoVia { count: read.len() });
            }
            None
        }
        Binding::Text { format, args } => {
            (format.matches("{}").count() > args.len()).then_some(BindError::TooManyPlaceholders)
        }
        _ => None,
    }
}

/// A path as the card shows it: short type name, full field path.
fn display_path(raw: &str) -> String {
    if raw.is_empty() {
        return "...".to_string();
    }
    if let Some(marker) = BindPath::new(raw).marker_type() {
        return short_of(marker);
    }
    match BindPath::new(raw).parse() {
        Ok(ParsedPath::Component { type_path, field }) => {
            format!("{}.{field}", short_of(&type_path))
        }
        Ok(ParsedPath::Resource { type_path, field }) => {
            format!("Res({}).{field}", short_of(&type_path))
        }
        Err(_) => raw.to_string(),
    }
}

/// The one-line summary of what a binding does, in the card's own shorthand.
fn summary(binding: &Binding) -> String {
    match binding {
        Binding::Field {
            read,
            via,
            write,
            as_percent,
        } => {
            let reads: Vec<String> = read.iter().map(|path| display_path(&path.raw)).collect();
            let source = match via {
                Some(via) if !via.is_empty() => format!("{}({})", short_of(via), reads.join(", ")),
                _ => reads.join(", "),
            };
            // A marker is put on and taken off, so there is no field to point at.
            if write.marker_type().is_some() {
                return format!("{source} sets {}", display_path(&write.raw));
            }
            let percent = if *as_percent { " as %" } else { "" };
            format!("{source} -> {}{percent}", display_path(&write.raw))
        }
        Binding::Text { format, args } => {
            let args: Vec<String> = args.iter().map(|path| display_path(&path.raw)).collect();
            format!("\"{format}\" <- {}", args.join(", "))
        }
        Binding::Visible { read, via } => {
            let source = match via {
                Some(via) if !via.is_empty() => {
                    format!("{}({})", short_of(via), display_path(&read.raw))
                }
                _ => display_path(&read.raw),
            };
            format!("show when {source}")
        }
        Binding::Value { with, two_way } => {
            let arrow = if *two_way { "<->" } else { "<-" };
            format!("value {arrow} {}", display_path(&with.raw))
        }
        Binding::Action { event, fields } => {
            let event = if event.is_empty() {
                "...".to_string()
            } else {
                short_of(event)
            };
            if fields.is_empty() {
                format!("fires {event}")
            } else {
                format!("fires {event} ({} mapped)", fields.len())
            }
        }
    }
}

/// Fill the `Bindings` card body for `source`, off the live component and the
/// project schema. World-exclusive and deferred by the caller.
pub(crate) fn fill_bindings_card_body(world: &mut World, source: Entity, body: Entity) {
    if world.get_entity(body).is_err() {
        return;
    }
    let Some(bindings) = world.get::<Bindings>(source).cloned() else {
        return;
    };
    let ctx = SchemaCtx::gather(world, source);

    let mut commands = world.commands();
    commands.entity(body).insert(BindingsCardBody(source));
    let count = bindings.0.len();
    for (index, binding) in bindings.0.iter().enumerate() {
        spawn_binding_row(&mut commands, body, source, index, binding, count, &ctx);
    }
    spawn_footer(&mut commands, body, source);
    world.flush();
}

fn spawn_binding_row(
    commands: &mut Commands,
    parent: Entity,
    source: Entity,
    index: usize,
    binding: &Binding,
    count: usize,
    ctx: &SchemaCtx,
) {
    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(tokens::SPACING_XS),
                padding: UiRect::all(Val::Px(tokens::SPACING_SM)),
                margin: UiRect::bottom(Val::Px(tokens::SPACING_SM)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(parent),
        ))
        .id();

    spawn_row_header(commands, row, source, index, binding, count);
    spawn_row_label(commands, row, binding, ctx);

    match binding {
        Binding::Field {
            read,
            write,
            as_percent,
            ..
        } => {
            for i in 0..read.len() {
                spawn_path_row(
                    commands,
                    PathRowProps {
                        parent: row,
                        source,
                        index,
                        slot: PathSlot::Read(i),
                        event_field: String::new(),
                        label: numbered("read", i, read.len()),
                        removable: read.len() > 1,
                    },
                    binding,
                    ctx,
                );
            }
            spawn_add_read(commands, row, source, index, "add read");
            spawn_via(commands, row, source, index, binding, ctx);
            spawn_path_row(
                commands,
                PathRowProps {
                    parent: row,
                    source,
                    index,
                    slot: PathSlot::Write,
                    event_field: String::new(),
                    label: "write".to_string(),
                    removable: false,
                },
                binding,
                ctx,
            );
            // A marker write has no number to scale.
            if write.marker_type().is_none() {
                spawn_checkbox(
                    commands,
                    row,
                    source,
                    index,
                    BindControl::AsPercent,
                    "as percent",
                    *as_percent,
                );
            }
        }
        Binding::Text { format, args } => {
            spawn_text_input(
                commands,
                row,
                source,
                index,
                BindControl::Format,
                "format",
                format,
                "{} / {}",
            );
            for i in 0..args.len() {
                spawn_path_row(
                    commands,
                    PathRowProps {
                        parent: row,
                        source,
                        index,
                        slot: PathSlot::Read(i),
                        event_field: String::new(),
                        label: numbered("arg", i, args.len()),
                        removable: args.len() > 1,
                    },
                    binding,
                    ctx,
                );
            }
            spawn_add_read(commands, row, source, index, "add arg");
        }
        Binding::Visible { .. } => {
            spawn_path_row(
                commands,
                PathRowProps {
                    parent: row,
                    source,
                    index,
                    slot: PathSlot::Read(0),
                    event_field: String::new(),
                    label: "read".to_string(),
                    removable: false,
                },
                binding,
                ctx,
            );
            spawn_via(commands, row, source, index, binding, ctx);
        }
        Binding::Value { two_way, .. } => {
            spawn_path_row(
                commands,
                PathRowProps {
                    parent: row,
                    source,
                    index,
                    slot: PathSlot::Read(0),
                    event_field: String::new(),
                    label: "with".to_string(),
                    removable: false,
                },
                binding,
                ctx,
            );
            spawn_checkbox(
                commands,
                row,
                source,
                index,
                BindControl::TwoWay,
                "two way",
                *two_way,
            );
        }
        Binding::Action { event, fields } => {
            spawn_event_picker(commands, row, source, index, event, ctx);
            // Without a schema the rows fall back to whatever is already
            // mapped: a mapping the card does not draw cannot be taken back.
            let names: Vec<String> = match ctx.event(event) {
                Some(schema) => schema.fields.clone(),
                None => fields.iter().map(|(name, _)| name.clone()).collect(),
            };
            for name in names {
                spawn_path_row(
                    commands,
                    PathRowProps {
                        parent: row,
                        source,
                        index,
                        slot: PathSlot::EventField,
                        label: name.clone(),
                        event_field: name,
                        removable: false,
                    },
                    binding,
                    ctx,
                );
            }
        }
    }
}

fn numbered(noun: &str, index: usize, count: usize) -> String {
    if count > 1 {
        format!("{noun} {}", index + 1)
    } else {
        noun.to_string()
    }
}

/// The row's top line: the kind picker and the three structural buttons.
fn spawn_row_header(
    commands: &mut Commands,
    row: Entity,
    source: Entity,
    index: usize,
    binding: &Binding,
    count: usize,
) {
    let header = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::End,
                column_gap: Val::Px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(row),
        ))
        .id();

    let kind = BindKind::of(binding);
    let selected = BindKind::ALL
        .iter()
        .position(|candidate| *candidate == kind)
        .unwrap_or(0);
    commands.spawn((
        variant_edit(
            VariantEditProps::new("kind")
                .with_label("kind")
                .with_popover_title("Binding kind")
                .with_variants(
                    BindKind::ALL
                        .iter()
                        .map(|kind| VariantDefinition::new(kind.label()).with_icon(kind.icon()))
                        .collect(),
                )
                .with_selected(selected),
        ),
        BindingControl::new(source, index, BindControl::Kind),
        ChildOf(header),
    ));

    for (control, icon, tip, enabled) in [
        (BindControl::MoveUp, Icon::ArrowUp, "Move up", index > 0),
        (
            BindControl::MoveDown,
            Icon::ArrowDown,
            "Move down",
            index + 1 < count,
        ),
        (BindControl::Remove, Icon::Trash2, "Remove binding", true),
    ] {
        let variant = if control == BindControl::Remove {
            ButtonVariant::Destructive
        } else {
            ButtonVariant::Ghost
        };
        let mut entity = commands.spawn((
            button(
                ButtonProps::new("")
                    .with_variant(variant)
                    .with_size(ButtonSize::IconSM)
                    .with_left_icon(icon),
            ),
            Tooltip::title(tip),
            ChildOf(header),
        ));
        // An arrow at the end of the list keeps its place so the header does not
        // reflow, but carries no control.
        if enabled {
            entity.insert(BindingControl::new(source, index, control));
        } else {
            entity.insert(InteractionDisabled);
        }
    }
}

/// The summary line, and the badge when something in the row names nothing.
fn spawn_row_label(commands: &mut Commands, parent: Entity, binding: &Binding, ctx: &SchemaCtx) {
    let error = ctx.binding_error(binding);
    let color = if error.is_some() {
        tokens::TEXT_ERROR
    } else {
        tokens::TEXT_SECONDARY
    };
    let mut entity = commands.spawn((
        BindingRowLabel,
        Text::new(summary(binding)),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(color),
        ChildOf(parent),
    ));
    if let Some(error) = error {
        // A badge, never a gate: the edit still commits. `Hovered` is opt-in and
        // the tooltip renderer reads the pair.
        entity.insert((Hovered::default(), Tooltip::title(error.to_string())));
    }
}

/// Where one path row goes and what it stands for.
struct PathRowProps {
    parent: Entity,
    source: Entity,
    index: usize,
    slot: PathSlot,
    event_field: String,
    label: String,
    removable: bool,
}

/// One path row: where it reads from, which type, which field, and (for a
/// list of reads) a button to drop this one.
fn spawn_path_row(
    commands: &mut Commands,
    props: PathRowProps,
    binding: &Binding,
    ctx: &SchemaCtx,
) {
    let PathRowProps {
        parent,
        source,
        index,
        slot,
        event_field,
        label,
        removable,
    } = props;

    let raw = path_at(binding, slot, &event_field)
        .map(|path| path.raw.clone())
        .unwrap_or_default();
    let (is_resource, type_path, field) = decompose(&raw);
    let (types, fields) = ctx.resolve(&raw, slot);

    // The row asks for a picker's width, so a panel that cannot give it that
    // drops the whole control onto its own line and the pickers wrap within it.
    let row = spawn_field_row(
        commands,
        parent,
        FieldRowProps::new(label).with_control_min_width(PICKER_MIN_WIDTH),
    );
    commands
        .entity(row.control)
        .entry::<Node>()
        .and_modify(|mut node| {
            node.flex_wrap = FlexWrap::Wrap;
            node.row_gap = Val::Px(tokens::SPACING_XS);
        });

    // A write always targets a component; only a read can come from a resource.
    if slot != PathSlot::Write {
        let selected = usize::from(is_resource);
        commands
            .spawn((
                combobox_with_selected(
                    vec![
                        ComboBoxOptionData::new("Component"),
                        ComboBoxOptionData::new("Resource"),
                    ],
                    selected,
                ),
                ChildOf(row.control),
            ))
            // The widget carries its own `Node`, so the narrower width goes on
            // afterwards rather than into the bundle beside it.
            .insert((
                ComboBoxSelectedIndex(selected),
                BindingOptions(vec!["Component".to_string(), "Resource".to_string()]),
                BindingControl::for_event_field(
                    source,
                    index,
                    BindControl::PathSource(slot),
                    &event_field,
                ),
                Node {
                    width: Val::Px(PICKER_MIN_WIDTH),
                    // Gives way with the pickers beside it rather than pushing
                    // them off a narrow row.
                    flex_shrink: 1.0,
                    min_width: Val::Px(0.0),
                    ..Default::default()
                },
            ));
    }

    let offered = offered_with_current(
        types
            .iter()
            .filter(|option| option.pickable)
            .map(|option| option.type_path.clone())
            .collect(),
        &type_path,
    );
    let selected = offered
        .iter()
        .position(|option| *option == type_path)
        .unwrap_or(0);
    commands
        .spawn((
            combobox_with_selected(
                offered
                    .iter()
                    .map(|path| ComboBoxOptionData::new(short_of(path)).with_value(path))
                    .collect::<Vec<_>>(),
                selected,
            ),
            ComboBoxSelectedIndex(selected),
            BindingOptions(offered),
            BindingControl::for_event_field(
                source,
                index,
                BindControl::PathType(slot),
                &event_field,
            ),
            ChildOf(row.control),
        ))
        .insert(picker_node());

    // A marker names no field, so the row stops at the type picker.
    if !SchemaCtx::lookup(types, &type_path).is_some_and(|option| option.marker) {
        let fields = offered_with_current(fields, &field);
        let selected = fields.iter().position(|name| *name == field).unwrap_or(0);
        commands
            .spawn((
                combobox_with_selected(
                    fields
                        .iter()
                        .map(|name| ComboBoxOptionData::new(name).with_value(name))
                        .collect::<Vec<_>>(),
                    selected,
                ),
                ComboBoxSelectedIndex(selected),
                BindingOptions(fields),
                BindingControl::for_event_field(
                    source,
                    index,
                    BindControl::PathField(slot),
                    &event_field,
                ),
                ChildOf(row.control),
            ))
            .insert(picker_node());
    }

    if removable && let PathSlot::Read(i) = slot {
        commands.spawn((
            button(
                ButtonProps::new("")
                    .with_variant(ButtonVariant::Ghost)
                    .with_size(ButtonSize::IconSM)
                    .with_left_icon(Icon::X),
            ),
            Tooltip::title("Remove this read"),
            BindingControl::new(source, index, BindControl::RemoveRead(i)),
            ChildOf(row.control),
        ));
    }

    if let Some(gap) = ctx.gap_warning(binding, &event_field) {
        commands.spawn((
            Text::new("unmapped"),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_ERROR),
            Hovered::default(),
            Tooltip::title(gap.to_string()),
            ChildOf(row.control),
        ));
    }
}

/// The options a picker shows, with whatever is already authored kept in the
/// list so opening a dropdown cannot lose the current value.
fn offered_with_current(mut offered: Vec<String>, current: &str) -> Vec<String> {
    if !current.is_empty() && !offered.iter().any(|option| option == current) {
        offered.insert(0, current.to_string());
    }
    offered
}

/// The transform picker: a dropdown of callable functions when the schema names
/// any, otherwise a free text field.
fn spawn_via(
    commands: &mut Commands,
    parent: Entity,
    source: Entity,
    index: usize,
    binding: &Binding,
    ctx: &SchemaCtx,
) {
    let current = via_of(binding).cloned().unwrap_or_default();

    if ctx.functions.is_empty() {
        spawn_text_input(
            commands,
            parent,
            source,
            index,
            BindControl::ViaText,
            "via",
            &current,
            "function name",
        );
        return;
    }

    let arity = via_arity(binding);
    let offered = offered_with_current(
        ctx.functions
            .iter()
            .filter(|function| function.arity == arity)
            .map(|function| function.name.clone())
            .collect(),
        &current,
    );
    spawn_optional_picker(
        commands,
        parent,
        source,
        index,
        BindControl::Via,
        "via",
        offered,
        &current,
    );
}

fn spawn_event_picker(
    commands: &mut Commands,
    parent: Entity,
    source: Entity,
    index: usize,
    current: &str,
    ctx: &SchemaCtx,
) {
    let offered = offered_with_current(
        ctx.events
            .iter()
            .map(|event| event.type_path.clone())
            .collect(),
        current,
    );
    spawn_optional_picker(
        commands,
        parent,
        source,
        index,
        BindControl::Event,
        "event",
        offered,
        current,
    );
}

/// A dropdown whose first option is "none", for a binding's optional transform
/// function or event.
#[expect(
    clippy::too_many_arguments,
    reason = "one row: where it goes, what it edits, what it offers, and what it holds"
)]
fn spawn_optional_picker(
    commands: &mut Commands,
    parent: Entity,
    source: Entity,
    index: usize,
    control: BindControl,
    label: &str,
    offered: Vec<String>,
    current: &str,
) {
    // Index 0 is "none", so offered indices shift by one and the recorded values
    // carry an empty string at the front.
    let selected = offered
        .iter()
        .position(|name| name == current)
        .map_or(0, |index| index + 1);
    let mut options = vec![ComboBoxOptionData::new("none")];
    options.extend(
        offered
            .iter()
            .map(|name| ComboBoxOptionData::new(short_of(name)).with_value(name)),
    );
    let mut values = vec![String::new()];
    values.extend(offered);

    let row = spawn_field_row(commands, parent, FieldRowProps::new(label));
    commands.spawn((
        combobox_with_selected(options, selected),
        ComboBoxSelectedIndex(selected),
        BindingOptions(values),
        BindingControl::new(source, index, control),
        ChildOf(row.control),
    ));
}

fn spawn_checkbox(
    commands: &mut Commands,
    parent: Entity,
    source: Entity,
    index: usize,
    control: BindControl,
    label: &str,
    value: bool,
) {
    let row = spawn_field_row(commands, parent, FieldRowProps::new(label));
    let mut entity = commands.spawn_scene(bsn! { @FeathersCheckbox });
    entity.insert((
        BindingControl::new(source, index, control),
        ChildOf(row.control),
    ));
    // The checkbox does not self-manage `Checked`; seed the initial state.
    if value {
        entity.insert(Checked);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "a labelled text row needs its control identity, its value and its placeholder"
)]
fn spawn_text_input(
    commands: &mut Commands,
    parent: Entity,
    source: Entity,
    index: usize,
    control: BindControl,
    label: &str,
    value: &str,
    placeholder: &str,
) {
    let row = spawn_field_row(commands, parent, FieldRowProps::new(label));
    commands.spawn((
        text_edit(
            TextEditProps::default()
                .with_placeholder(placeholder)
                .with_default_value(value)
                .allow_empty(),
        ),
        BindingControl::new(source, index, control),
        ChildOf(row.control),
    ));
}

fn spawn_add_read(
    commands: &mut Commands,
    parent: Entity,
    source: Entity,
    index: usize,
    label: &str,
) {
    commands.spawn((
        button(
            ButtonProps::new(label)
                .with_variant(ButtonVariant::Ghost)
                .with_size(ButtonSize::MD)
                .with_left_icon(Icon::Plus),
        ),
        BindingControl::new(source, index, BindControl::AddRead),
        ChildOf(parent),
    ));
}

/// The footer: one menu that adds a binding of any of the five kinds.
fn spawn_footer(commands: &mut Commands, parent: Entity, source: Entity) {
    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                width: Val::Percent(100.0),
                margin: UiRect::top(Val::Px(tokens::SPACING_SM)),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();
    commands.spawn((
        combobox_with_label(
            BindKind::ALL
                .iter()
                .map(|kind| ComboBoxOptionData::new(kind.label()).with_icon(kind.icon()))
                .collect::<Vec<_>>(),
            "Add Binding",
        ),
        BindingOptions(
            BindKind::ALL
                .iter()
                .map(|kind| kind.label().to_string())
                .collect(),
        ),
        AddBindingMenu { source },
        ChildOf(row),
    ));
}

/// Build the new `Bindings` value and commit it whole. Every mutation on this
/// card comes through here, so one gesture is one patch and one undo entry.
fn apply(world: &mut World, source: Entity, mutate: impl FnOnce(&mut Vec<Binding>) -> bool) {
    let Some(mut list) = world
        .get::<Bindings>(source)
        .map(|bindings| bindings.0.clone())
    else {
        warn!("a binding edit was dropped: {source} carries no Bindings to edit");
        return;
    };
    if !mutate(&mut list) {
        return;
    }
    let bindings = Bindings(list);
    let json = {
        let Some(registry) = world.get_resource::<AppTypeRegistry>().cloned() else {
            warn!("a binding edit was dropped: no type registry to serialize it with");
            return;
        };
        let registry = registry.read();
        let serializer = TypedReflectSerializer::new(&bindings, &registry);
        match serde_json::to_value(&serializer) {
            Ok(json) => json,
            Err(error) => {
                warn!("a binding edit could not be serialized, so it was dropped: {error}");
                return;
            }
        }
    };

    if !super::reflect_fields::try_route_pie_live_field_edit(
        world,
        source,
        bindings_type_path(),
        "",
        json.clone(),
    ) {
        crate::commands::field_edit_commit(world, bindings_type_path(), "", &json, GROUP_LABEL);
    }
    // Remember the tick this write landed on, so the change-detection pass does
    // not rebuild the card for the edit it just made.
    let echo = world.get_entity(source).ok().and_then(|entity| {
        entity
            .get_ref::<Bindings>()
            .map(|bindings| bindings.last_changed())
    });
    if let Some(mut card_echo) = world.get_resource_mut::<BindingsCardEcho>() {
        card_echo.0 = echo.map(|tick| (source, tick));
    }
    world.trigger(RefreshInspectorCardBody {
        source,
        type_path: bindings_type_path().to_string(),
    });
    world.flush();
}

/// The optional halves of a binding a clause may spell: enough to author each of
/// the five shapes, and nothing that needs a schema to make sense of.
struct BindingEdit {
    read: Option<Vec<BindPath>>,
    write: Option<BindPath>,
    via: Option<Option<String>>,
    as_percent: Option<bool>,
    format: Option<String>,
    two_way: Option<bool>,
    event: Option<String>,
    map: Option<Vec<(String, BindPath)>>,
}

impl BindingEdit {
    /// Read one clause's optional keys. `read` and `with` are the same key under
    /// the two names the card's rows use for it.
    fn from_params(params: &OperatorParameters) -> Self {
        let paths = |raw: &str| {
            raw.split(',')
                .filter(|part| !part.is_empty())
                .map(BindPath::new)
                .collect::<Vec<_>>()
        };
        let read = params.as_str("read");
        if read.is_some() && params.as_str("with").is_some() {
            warn!("a binding clause gave both `read` and `with`; `read` is the one that landed");
        }
        Self {
            read: read.or_else(|| params.as_str("with")).map(paths),
            write: params.as_str("write").map(BindPath::new),
            // An empty `via=` clears it.
            via: params
                .as_str("via")
                .map(|name| (!name.is_empty()).then(|| name.to_string())),
            as_percent: params.as_bool("as_percent"),
            format: params.as_str("format").map(str::to_string),
            two_way: params.as_bool("two_way"),
            event: params.as_str("event").map(str::to_string),
            map: params.as_str("map").map(|raw| {
                raw.split(',')
                    .filter(|part| !part.is_empty())
                    .filter_map(|pair| pair.split_once(':'))
                    .map(|(field, path)| (field.to_string(), BindPath::new(path)))
                    .collect()
            }),
        }
    }

    /// Apply what applies to `binding` and return the keys this shape has no
    /// place for.
    fn apply_to(self, binding: &mut Binding) -> Vec<&'static str> {
        let mut ignored = Vec::new();
        let Self {
            read,
            write,
            via,
            as_percent,
            format,
            two_way,
            event,
            map,
        } = self;
        match binding {
            Binding::Field {
                read: slot,
                via: via_slot,
                write: write_slot,
                as_percent: percent_slot,
            } => {
                if let Some(read) = read {
                    *slot = read;
                }
                if let Some(via) = via {
                    *via_slot = via;
                }
                if let Some(write) = write {
                    *write_slot = write;
                }
                if let Some(as_percent) = as_percent {
                    *percent_slot = as_percent;
                }
                ignored.extend(named_keys(&[
                    ("format", format.is_some()),
                    ("two_way", two_way.is_some()),
                    ("event", event.is_some()),
                    ("map", map.is_some()),
                ]));
            }
            Binding::Text {
                format: format_slot,
                args,
            } => {
                if let Some(read) = read {
                    *args = read;
                }
                if let Some(format) = format {
                    *format_slot = format;
                }
                ignored.extend(named_keys(&[
                    ("write", write.is_some()),
                    ("via", via.is_some()),
                    ("as_percent", as_percent.is_some()),
                    ("two_way", two_way.is_some()),
                    ("event", event.is_some()),
                    ("map", map.is_some()),
                ]));
            }
            Binding::Visible {
                read: slot,
                via: via_slot,
            } => {
                if let Some(read) = read {
                    *slot = single_path(read, "Visible");
                }
                if let Some(via) = via {
                    *via_slot = via;
                }
                ignored.extend(named_keys(&[
                    ("write", write.is_some()),
                    ("as_percent", as_percent.is_some()),
                    ("format", format.is_some()),
                    ("two_way", two_way.is_some()),
                    ("event", event.is_some()),
                    ("map", map.is_some()),
                ]));
            }
            Binding::Value {
                with,
                two_way: two_way_slot,
            } => {
                if let Some(read) = read {
                    *with = single_path(read, "Value");
                }
                if let Some(two_way) = two_way {
                    *two_way_slot = two_way;
                }
                ignored.extend(named_keys(&[
                    ("write", write.is_some()),
                    ("via", via.is_some()),
                    ("as_percent", as_percent.is_some()),
                    ("format", format.is_some()),
                    ("event", event.is_some()),
                    ("map", map.is_some()),
                ]));
            }
            Binding::Action {
                event: event_slot,
                fields,
            } => {
                // A mapping belongs to the event it was made against, so a new
                // event clears it before the clause's own map lands.
                if let Some(event) = event
                    && *event_slot != event
                {
                    *event_slot = event;
                    fields.clear();
                }
                if let Some(map) = map {
                    *fields = map;
                }
                ignored.extend(named_keys(&[
                    ("read", read.is_some()),
                    ("write", write.is_some()),
                    ("via", via.is_some()),
                    ("as_percent", as_percent.is_some()),
                    ("format", format.is_some()),
                    ("two_way", two_way.is_some()),
                ]));
            }
        }
        ignored
    }
}

/// The one path a `Visible` or `Value` binding keeps, warning if given more.
fn single_path(read: Vec<BindPath>, shape: &str) -> BindPath {
    if read.len() > 1 {
        warn!(
            "a {shape} binding reads one path; the {} after `{}` were dropped",
            read.len() - 1,
            read[0].raw
        );
    }
    read.into_iter().next().unwrap_or_default()
}

fn named_keys(present: &[(&'static str, bool)]) -> Vec<&'static str> {
    present
        .iter()
        .filter(|(_, given)| *given)
        .map(|(key, _)| *key)
        .collect()
}

/// Add a binding to an entity's list. The entity has to carry `Bindings`
/// already; putting it there is `component.add`'s job.
#[operator(
    id = "binding.add",
    label = "Add Binding",
    description = "Add a binding to the selected entity's Bindings list.",
    allows_undo = false,
    is_available = super::ops::has_primary_selection,
    params(
        entity(Entity, doc = "Entity whose Bindings list gains the binding."),
        kind(String, doc = "One of field, text, visible, value, action."),
        read(String, doc = "Comma-separated read paths, e.g. \"Health.current\"."),
        with(String, doc = "The path a Value binding stays in step with."),
        write(String, doc = "Where a Field binding writes; a bare type path drives a marker."),
        via(String, doc = "Registered function the reads pass through. Empty clears it."),
        as_percent(bool, doc = "Write a Field binding's result as a percentage."),
        format(
            String,
            doc = "A Text binding's format string, with {} per value. A boot clause has no \
                   quoting, so this one cannot contain a space."
        ),
        two_way(bool, doc = "Let a Value binding write the author's edits back."),
        event(String, doc = "An Action binding's event type path."),
        map(String, doc = "Action field mapping, as comma-separated field:path pairs."),
    ),
)]
pub(crate) fn binding_add(
    params: In<OperatorParameters>,
    mut commands: Commands,
    bound: Query<(), With<Bindings>>,
) -> OperatorResult {
    let entity = params.as_entity("entity")?;
    let Some(kind) = params.as_str("kind").and_then(bind_kind_named) else {
        warn!(
            "binding.add: `kind` names none of field, text, visible, value, action (got {:?})",
            params.as_str("kind")
        );
        return OperatorResult::Cancelled;
    };
    if !bound.contains(entity) {
        warn!(
            "binding.add: {entity} carries no Bindings; add it first with \
             `component.add type_path=jackdaw_bind::types::Bindings`"
        );
        return OperatorResult::Cancelled;
    }
    let edit = BindingEdit::from_params(&params);
    commands.queue(move |world: &mut World| {
        crate::selection::select_for_edit(world, entity);
        apply(world, entity, |list| {
            let mut binding = kind.new_binding(None);
            report_ignored("binding.add", kind, edit.apply_to(&mut binding));
            list.push(binding);
            true
        });
    });
    OperatorResult::Finished
}

/// Edit one binding already on the list, addressed by index, top row first.
#[operator(
    id = "binding.set",
    label = "Set Binding",
    description = "Change one binding on the selected entity's Bindings list.",
    allows_undo = false,
    is_available = super::ops::has_primary_selection,
    params(
        entity(Entity, doc = "Entity whose binding is edited."),
        index(
            i64,
            doc = "Which binding, counting from zero, top row first. Left out, the top row."
        ),
        read(String, doc = "Comma-separated read paths, e.g. \"Health.current\"."),
        with(String, doc = "The path a Value binding stays in step with."),
        write(String, doc = "Where a Field binding writes; a bare type path drives a marker."),
        via(String, doc = "Registered function the reads pass through. Empty clears it."),
        as_percent(bool, doc = "Write a Field binding's result as a percentage."),
        format(
            String,
            doc = "A Text binding's format string, with {} per value. A boot clause has no \
                   quoting, so this one cannot contain a space."
        ),
        two_way(bool, doc = "Let a Value binding write the author's edits back."),
        event(String, doc = "An Action binding's event type path."),
        map(String, doc = "Action field mapping, as comma-separated field:path pairs."),
    ),
)]
pub(crate) fn binding_set(
    params: In<OperatorParameters>,
    mut commands: Commands,
    bound: Query<&Bindings>,
) -> OperatorResult {
    let entity = params.as_entity("entity")?;
    let index = match params.get("index") {
        None => 0,
        Some(PropertyValue::Int(index)) if *index >= 0 => *index as usize,
        // Spelled but no position: better to edit nothing than binding zero.
        Some(other) => {
            warn!("binding.set: `index` is {other}, which is no binding position");
            return OperatorResult::Cancelled;
        }
    };
    let Ok(bindings) = bound.get(entity) else {
        warn!(
            "binding.set: {entity} carries no Bindings; add it first with \
             `component.add type_path=jackdaw_bind::types::Bindings`"
        );
        return OperatorResult::Cancelled;
    };
    let Some(kind) = bindings.0.get(index).map(BindKind::of) else {
        warn!(
            "binding.set: {entity} has no binding {index}; it holds {}",
            bindings.0.len()
        );
        return OperatorResult::Cancelled;
    };
    let edit = BindingEdit::from_params(&params);
    commands.queue(move |world: &mut World| {
        crate::selection::select_for_edit(world, entity);
        apply(world, entity, |list| {
            let Some(binding) = list.get_mut(index) else {
                return false;
            };
            report_ignored("binding.set", kind, edit.apply_to(binding));
            true
        });
    });
    OperatorResult::Finished
}

fn bind_kind_named(name: &str) -> Option<BindKind> {
    BindKind::ALL
        .into_iter()
        .find(|kind| kind.label().eq_ignore_ascii_case(name))
}

fn report_ignored(op: &str, kind: BindKind, ignored: Vec<&'static str>) {
    if !ignored.is_empty() {
        warn!(
            "{op}: a {} binding has no {}; those were ignored",
            kind.label().to_lowercase(),
            ignored.join(", ")
        );
    }
}

/// The `Bindings` write this card made itself, as the tick it landed on, so the
/// refresh pass can tell its own commits from outside changes.
#[derive(Resource, Default)]
pub struct BindingsCardEcho(Option<(Entity, bevy::ecs::change_detection::Tick)>);

/// Rebuild the card when `Bindings` changes underneath it. The card addresses
/// every widget by index, so a value that moved without a rebuild leaves rows
/// pointing at bindings that are not theirs.
pub fn refresh_bindings_card_on_change(
    cards: Query<&BindingsCardBody>,
    bindings: Query<Ref<Bindings>>,
    echo: Res<BindingsCardEcho>,
    mut commands: Commands,
) {
    let mut done: Vec<Entity> = Vec::new();
    for card in &cards {
        let source = card.0;
        if done.contains(&source) {
            continue;
        }
        let Ok(bindings) = bindings.get(source) else {
            continue;
        };
        if !bindings.is_changed() || echo.0 == Some((source, bindings.last_changed())) {
            continue;
        }
        done.push(source);
        commands.trigger(RefreshInspectorCardBody {
            source,
            type_path: bindings_type_path().to_string(),
        });
    }
}

/// Mutate one binding in place. An index that no longer names a binding is a
/// stale widget, not an error.
fn apply_to(
    commands: &mut Commands,
    source: Entity,
    index: usize,
    mutate: impl FnOnce(&mut Binding) + Send + 'static,
) {
    commands.queue(move |world: &mut World| {
        apply(world, source, |list| {
            let Some(binding) = list.get_mut(index) else {
                return false;
            };
            mutate(binding);
            true
        });
    });
}

/// The read-only entities every write path on the card has to check for.
type RemoteProxies<'w, 's> =
    Query<'w, 's, (), With<crate::remote::entity_browser::RemoteEntityProxy>>;

pub(crate) fn on_binding_combobox_change(
    event: On<ComboBoxChangeEvent>,
    controls: Query<&BindingControl>,
    options: Query<&BindingOptions>,
    variants: Query<&VariantComboBox>,
    menus: Query<&AddBindingMenu>,
    remote_proxies: RemoteProxies,
    mut commands: Commands,
) {
    // The footer menu adds; nothing else on the card does.
    if let Ok(menu) = menus.get(event.entity) {
        if remote_proxies.contains(menu.source) {
            return;
        }
        let Some(kind) = BindKind::ALL.get(event.selected).copied() else {
            return;
        };
        let source = menu.source;
        commands.queue(move |world: &mut World| {
            apply(world, source, |list| {
                list.push(kind.new_binding(None));
                true
            });
        });
        return;
    }

    // The kind picker's dropdown lives inside the variant-edit popover, so its
    // event arrives on the popover's combobox, not the card's own control.
    let owner = variants
        .get(event.entity)
        .map_or(event.entity, |variant| variant.0);
    let Ok(control) = controls.get(owner) else {
        return;
    };
    if remote_proxies.contains(control.source) {
        return;
    }
    let (source, index, event_field) =
        (control.source, control.binding, control.event_field.clone());
    // The widget's own value wins; the recorded option list is the fallback for
    // an index-only change.
    let picked = event.value.clone().or_else(|| {
        options
            .get(event.entity)
            .ok()
            .and_then(|options| options.0.get(event.selected).cloned())
    });

    match control.control {
        BindControl::Kind => {
            let Some(kind) = BindKind::ALL.get(event.selected).copied() else {
                return;
            };
            apply_to(&mut commands, source, index, move |binding| {
                if BindKind::of(binding) == kind {
                    return;
                }
                // Carry the first path across, so a misclick on the kind menu
                // does not throw away the source already picked.
                let carried = path_at(binding, PathSlot::Read(0), "").cloned();
                *binding = kind.new_binding(carried);
            });
        }
        BindControl::PathSource(slot) => {
            // The type list changes with the source, so the row lands on the
            // first entry of the new list.
            let to_resource = event.selected == 1;
            commands.queue(move |world: &mut World| {
                let ctx = SchemaCtx::gather(world, source);
                let types: &[TypeOption] = if to_resource {
                    &ctx.resources
                } else {
                    ctx.types(slot)
                };
                let raw = types
                    .iter()
                    .find(|option| option.pickable)
                    .map(|option| {
                        compose(
                            to_resource,
                            &option.type_path,
                            option
                                .fields
                                .first()
                                .map(String::as_str)
                                .unwrap_or_default(),
                        )
                    })
                    .unwrap_or_default();
                apply(world, source, |list| {
                    let Some(binding) = list.get_mut(index) else {
                        return false;
                    };
                    set_path(binding, slot, &event_field, raw);
                    true
                });
            });
        }
        BindControl::PathType(slot) => {
            let Some(type_path) = picked else {
                return;
            };
            commands.queue(move |world: &mut World| {
                let ctx = SchemaCtx::gather(world, source);
                let raw = world
                    .get::<Bindings>(source)
                    .and_then(|bindings| bindings.0.get(index))
                    .and_then(|binding| path_at(binding, slot, &event_field))
                    .map(|path| path.raw.clone())
                    .unwrap_or_default();
                let (is_resource, _, field) = decompose(&raw);
                let types: &[TypeOption] = if is_resource {
                    &ctx.resources
                } else {
                    ctx.types(slot)
                };
                let option = SchemaCtx::lookup(types, &type_path);
                let fields = option
                    .map(|option| option.fields.clone())
                    .unwrap_or_default();
                // Keep the field name when the new type has one by that name,
                // otherwise land on its first. A type with no schema reports no
                // fields, which is not the same as having none, so its authored
                // field is kept.
                let field = if fields.contains(&field) || option.is_none() {
                    field
                } else {
                    fields.first().cloned().unwrap_or_default()
                };
                let raw = compose_option(option, is_resource, &type_path, &field);
                apply(world, source, |list| {
                    let Some(binding) = list.get_mut(index) else {
                        return false;
                    };
                    set_path(binding, slot, &event_field, raw);
                    true
                });
            });
        }
        BindControl::PathField(slot) => {
            let Some(field) = picked else {
                return;
            };
            apply_to(&mut commands, source, index, move |binding| {
                let raw = path_at(binding, slot, &event_field)
                    .map(|path| path.raw.clone())
                    .unwrap_or_default();
                let (is_resource, type_path, _) = decompose(&raw);
                set_path(
                    binding,
                    slot,
                    &event_field,
                    compose(is_resource, &type_path, &field),
                );
            });
        }
        BindControl::Via => {
            let via = picked.filter(|name| !name.is_empty());
            apply_to(&mut commands, source, index, move |binding| match binding {
                Binding::Field { via: slot, .. } | Binding::Visible { via: slot, .. } => {
                    *slot = via;
                }
                _ => {}
            });
        }
        BindControl::Event => {
            let picked = picked.unwrap_or_default();
            apply_to(&mut commands, source, index, move |binding| {
                if let Binding::Action { event, fields } = binding
                    && *event != picked
                {
                    // A mapping belongs to the event it was made against.
                    *event = picked;
                    fields.clear();
                }
            });
        }
        _ => {}
    }
}

pub(crate) fn on_binding_button_click(
    event: On<ButtonClickEvent>,
    controls: Query<&BindingControl>,
    remote_proxies: RemoteProxies,
    mut commands: Commands,
) {
    let Ok(control) = controls.get(event.entity) else {
        return;
    };
    if remote_proxies.contains(control.source) {
        return;
    }
    let (source, index) = (control.source, control.binding);
    match control.control {
        BindControl::Remove => {
            commands.queue(move |world: &mut World| {
                apply(world, source, |list| {
                    if index >= list.len() {
                        return false;
                    }
                    list.remove(index);
                    true
                });
            });
        }
        BindControl::MoveUp | BindControl::MoveDown => {
            let up = control.control == BindControl::MoveUp;
            commands.queue(move |world: &mut World| {
                apply(world, source, |list| {
                    let other = if up {
                        index.checked_sub(1)
                    } else {
                        index.checked_add(1)
                    };
                    let Some(other) = other.filter(|other| *other < list.len()) else {
                        return false;
                    };
                    list.swap(index, other);
                    true
                });
            });
        }
        BindControl::AddRead => {
            apply_to(&mut commands, source, index, move |binding| match binding {
                Binding::Field { read, .. } => read.push(BindPath::default()),
                Binding::Text { args, .. } => args.push(BindPath::default()),
                _ => {}
            });
        }
        BindControl::RemoveRead(slot) => {
            apply_to(&mut commands, source, index, move |binding| match binding {
                // The last read stays, and a slot the binding has since lost
                // names no read to drop.
                Binding::Field { read, .. } if read.len() > 1 && slot < read.len() => {
                    read.remove(slot);
                }
                Binding::Text { args, .. } if args.len() > 1 && slot < args.len() => {
                    args.remove(slot);
                }
                _ => {}
            });
        }
        _ => {}
    }
}

pub(crate) fn on_binding_checkbox_change(
    event: On<ValueChange<bool>>,
    controls: Query<&BindingControl>,
    remote_proxies: RemoteProxies,
    mut commands: Commands,
) {
    let target = event.source;
    let Ok(control) = controls.get(target) else {
        return;
    };
    if remote_proxies.contains(control.source) {
        return;
    }
    let value = event.value;
    // The checkbox does not self-manage `Checked`.
    jackdaw_feathers::utils::set_marker_if_alive::<Checked>(&mut commands, target, value);
    let control_kind = control.control;
    apply_to(
        &mut commands,
        control.source,
        control.binding,
        move |binding| match (binding, control_kind) {
            (Binding::Field { as_percent, .. }, BindControl::AsPercent) => *as_percent = value,
            (Binding::Value { two_way, .. }, BindControl::TwoWay) => *two_way = value,
            _ => {}
        },
    );
}

pub(crate) fn on_binding_text_commit(
    event: On<TextEditCommitEvent>,
    controls: Query<&BindingControl>,
    child_of: Query<&ChildOf>,
    remote_proxies: RemoteProxies,
    mut commands: Commands,
) {
    // The commit fires on the inner text entry, so walk up to the row the card
    // spawned to find the control.
    let mut current = event.entity;
    let mut found = controls.get(current).ok().cloned();
    for _ in 0..4 {
        if found.is_some() {
            break;
        }
        let Ok(parent) = child_of.get(current) else {
            break;
        };
        found = controls.get(parent.parent()).ok().cloned();
        current = parent.parent();
    }
    let Some(control) = found else {
        return;
    };
    if remote_proxies.contains(control.source) {
        return;
    }
    let text = event.text.clone();
    let control_kind = control.control;
    apply_to(
        &mut commands,
        control.source,
        control.binding,
        move |binding| match (binding, control_kind) {
            (Binding::Text { format, .. }, BindControl::Format) => *format = text,
            (Binding::Field { via, .. }, BindControl::ViaText)
            | (Binding::Visible { via, .. }, BindControl::ViaText) => {
                *via = (!text.is_empty()).then_some(text);
            }
            _ => {}
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind constructs, and constructs as itself.
    #[test]
    fn every_kind_builds_a_binding_of_that_kind() {
        for kind in BindKind::ALL {
            assert_eq!(
                BindKind::of(&kind.new_binding(None)),
                kind,
                "{kind:?} has to build itself",
            );
        }
    }

    /// The one path every kind but `Action` has survives a kind change.
    #[test]
    fn a_kind_change_carries_the_first_path() {
        let carried = Some(BindPath::new("demo::Health.current"));
        for kind in [
            BindKind::Field,
            BindKind::Text,
            BindKind::Visible,
            BindKind::Value,
        ] {
            let binding = kind.new_binding(carried.clone());
            assert_eq!(
                path_at(&binding, PathSlot::Read(0), "").map(|path| path.raw.as_str()),
                Some("demo::Health.current"),
                "{kind:?} keeps the carried path",
            );
        }
    }

    /// A path survives the compose/decompose round trip every picker does.
    #[test]
    fn a_path_survives_being_taken_apart_and_put_back() {
        for raw in [
            "demo::Health.current",
            "Res(demo::Audio).master",
            "bevy_ui::ui_node::Node.margin.left",
        ] {
            let (is_resource, type_path, field) = decompose(raw);
            assert_eq!(compose(is_resource, &type_path, &field), raw);
        }
        assert_eq!(compose(false, "demo::Health", ""), "");
        assert_eq!(compose(false, "", "current"), "");
    }

    fn marker_option(type_path: &str) -> TypeOption {
        TypeOption {
            type_path: type_path.to_string(),
            short_name: short_of(type_path),
            fields: Vec::new(),
            pickable: true,
            marker: true,
            on_widget: false,
        }
    }

    /// A marker path names a type and no field, and survives the round trip.
    #[test]
    fn a_marker_path_survives_being_taken_apart_and_put_back() {
        let raw = "bevy_ui::interaction_states::InteractionDisabled";
        let option = marker_option(raw);
        let (is_resource, type_path, field) = decompose(raw);
        assert_eq!(type_path, raw);
        assert!(field.is_empty());
        assert_eq!(
            compose_option(Some(&option), is_resource, &type_path, &field),
            raw,
        );
        assert_eq!(
            compose(is_resource, &type_path, &field),
            "",
            "without the picker's answer a fieldless path is still half a path",
        );
    }

    /// A marker write reads as "sets" rather than an arrow at a missing field.
    #[test]
    fn a_marker_write_reads_as_setting_the_component() {
        let binding = Binding::Field {
            read: vec![BindPath::new("demo::Form.incomplete")],
            via: None,
            write: BindPath::new("bevy_ui::interaction_states::InteractionDisabled"),
            as_percent: false,
        };
        assert_eq!(
            summary(&binding),
            "Form.incomplete sets InteractionDisabled"
        );
    }

    /// A marker is a legitimate write path and a nonsense read path.
    #[test]
    fn a_marker_is_judged_only_as_a_write() {
        let ctx = SchemaCtx {
            components: Vec::new(),
            resources: Vec::new(),
            write_targets: vec![marker_option(
                "bevy_ui::interaction_states::InteractionDisabled",
            )],
            events: Vec::new(),
            functions: Vec::new(),
            schema_known: true,
        };
        let raw = "bevy_ui::interaction_states::InteractionDisabled";
        assert_eq!(ctx.path_error(raw, PathSlot::Write), None);
        assert!(matches!(
            ctx.path_error(raw, PathSlot::Read(0)),
            Some(BindError::MalformedPath { .. }),
        ));
        assert!(matches!(
            ctx.path_error("demo::NotAMarker", PathSlot::Write),
            Some(BindError::UnknownType { .. }),
        ));
    }

    #[test]
    fn display_shortens_the_type_and_keeps_the_field() {
        assert_eq!(display_path("demo::game::Health.current"), "Health.current");
        assert_eq!(
            display_path("Res(demo::game::Audio).master"),
            "Res(Audio).master",
        );
        assert_eq!(display_path(""), "...");
    }

    /// The summary names both ends of the binding.
    #[test]
    fn a_summary_names_both_ends() {
        let binding = Binding::Field {
            read: vec![
                BindPath::new("demo::Health.current"),
                BindPath::new("demo::Health.max"),
            ],
            via: Some("demo::ratio".to_string()),
            write: BindPath::new("bevy_ui::ui_node::Node.width"),
            as_percent: true,
        };
        assert_eq!(
            summary(&binding),
            "ratio(Health.current, Health.max) -> Node.width as %",
        );
    }

    /// The two shape mistakes the card's own buttons make easy.
    #[test]
    fn a_binding_that_cannot_evaluate_says_so_without_a_schema() {
        let field = |reads: usize, via: Option<&str>| Binding::Field {
            read: vec![BindPath::new("demo::Health.current"); reads],
            via: via.map(str::to_string),
            write: BindPath::new("bevy_ui::ui_node::Node.width"),
            as_percent: false,
        };
        assert_eq!(shape_error(&field(1, None)), None);
        assert_eq!(
            shape_error(&field(2, None)),
            Some(BindError::MultipleReadsNoVia { count: 2 }),
        );
        assert_eq!(
            shape_error(&field(2, Some("demo::ratio"))),
            None,
            "two reads with a via is the whole point of a via",
        );
        assert_eq!(shape_error(&field(0, None)), Some(BindError::NoReads));

        let text = |format: &str, args: usize| Binding::Text {
            format: format.to_string(),
            args: vec![BindPath::new("demo::Health.current"); args],
        };
        assert_eq!(shape_error(&text("{}", 1)), None);
        assert_eq!(
            shape_error(&text("{} / {}", 1)),
            Some(BindError::TooManyPlaceholders),
        );
        assert_eq!(
            shape_error(&text("hp", 1)),
            None,
            "an unused arg is wasteful, not broken",
        );
    }

    /// Setting an event mapping is by name; clearing one removes the entry.
    #[test]
    fn an_event_mapping_is_set_and_cleared_by_name() {
        let mut binding = Binding::Action {
            event: "demo::Fired".to_string(),
            fields: Vec::new(),
        };
        set_path(
            &mut binding,
            PathSlot::EventField,
            "amount",
            "demo::Health.current".to_string(),
        );
        assert_eq!(
            path_at(&binding, PathSlot::EventField, "amount").map(|path| path.raw.as_str()),
            Some("demo::Health.current"),
        );
        set_path(&mut binding, PathSlot::EventField, "amount", String::new());
        let Binding::Action { fields, .. } = &binding else {
            panic!("still an action");
        };
        assert!(fields.is_empty(), "clearing a mapping removes the entry");
    }
}
