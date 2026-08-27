use bevy::prelude::Entity;

/// Every way a binding can fail to resolve, evaluate or dispatch. Display text
/// is what the warn-once log and the editor's validation badge both show, so it
/// stays phrased for an author reading their own scene.
///
/// # Matching
///
/// The list grows as bindings learn to do more, so it is `#[non_exhaustive]`:
/// a match needs a `_` arm even when it names every variant that exists today.
///
/// ```
/// # use jackdaw_bind::BindError;
/// fn hint(error: &BindError) -> &'static str {
///     match error {
///         BindError::AmbiguousType { .. } => "name the full type path",
///         BindError::NoContext { .. } => "give the widget a BindContext",
///         _ => "check the binding against the scene",
///     }
/// }
/// # assert_eq!(hint(&BindError::NoTypeRegistry), "check the binding against the scene");
/// ```
///
/// Naming variants instead does not compile, however many of them are named:
/// there is always one more the match has not seen.
///
/// ```compile_fail
/// # use jackdaw_bind::BindError;
/// # fn describe(error: BindError) -> &'static str {
/// match error {
///     BindError::NoReads => "nothing to read",
///     BindError::VisibleNotBool => "not a bool",
/// }
/// # }
/// ```
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum BindError {
    /// The path is neither `Type.field` nor `Res(Type).field`.
    #[error("{reason} in '{raw}'")]
    MalformedPath {
        /// The path as authored.
        raw: String,
        /// The syntax problem: "unclosed Res(", "empty segment", ...
        reason: &'static str,
    },

    /// The path names a type the app never registered, so nothing can be read
    /// or written through it.
    #[error("unknown {noun} '{type_path}'")]
    UnknownType {
        /// What was being looked up: "type" or "event type".
        noun: &'static str,
        /// The type path that found nothing.
        type_path: String,
    },

    /// The path gave a short type name that several registered types answer
    /// to.
    #[error("ambiguous short type path '{type_path}': {} -- use the full type path", candidates.join(", "))]
    AmbiguousType {
        /// The short name that was not enough to go on.
        type_path: String,
        /// The full paths to choose between.
        candidates: Vec<String>,
    },

    /// The app holds no `AppTypeRegistry`, so no path can name anything.
    #[error("no type registry")]
    NoTypeRegistry,

    /// The app holds no `AppFunctionRegistry`, so a `via` function cannot be
    /// looked up.
    #[error("no function registry")]
    NoFunctionRegistry,

    /// A component path needs a subject entity, and neither the widget nor any
    /// ancestor carries a [`BindContext`](crate::BindContext).
    #[error("no context for '{raw}'")]
    NoContext {
        /// The path that had nowhere to read from.
        raw: String,
    },

    /// The type resolved but is not registered as a component, so it cannot
    /// sit on the context entity.
    #[error("'{type_path}' is not a component")]
    NotAComponent {
        /// The type the path named.
        type_path: String,
    },

    /// `Res(Type)` named a type that is not registered as a resource.
    #[error("'{type_path}' is not a resource")]
    NotAResource {
        /// The type the path named.
        type_path: String,
    },

    /// The resource type is known, but the app is not holding one right now.
    #[error("resource '{type_path}' not present")]
    ResourceNotPresent {
        /// The resource the path named.
        type_path: String,
    },

    /// The context names an entity that has since been despawned.
    #[error("context entity {entity} does not exist")]
    ContextEntityMissing {
        /// The entity the context pointed at.
        entity: Entity,
    },

    /// The context entity is alive but does not carry the component the path
    /// reads from.
    #[error("context entity lacks '{type_path}'")]
    ContextMissingComponent {
        /// The component the path expected to find there.
        type_path: String,
    },

    /// The entity a write was aimed at is gone.
    #[error("entity {entity} does not exist")]
    EntityMissing {
        /// The entity the write was aimed at.
        entity: Entity,
    },

    /// The entity is alive but does not carry the component the write path
    /// names.
    #[error("entity lacks '{type_path}'")]
    EntityMissingComponent {
        /// The component the write path expected to find there.
        type_path: String,
    },

    /// Reflection's own path error, kept as text: it borrows from the value it
    /// was raised against and cannot outlive the lookup.
    #[error("path '{field}' on '{type_path}': {message}")]
    ReflectPath {
        /// The field half of the path.
        field: String,
        /// The type the field was looked for on.
        type_path: String,
        /// What reflection said about it.
        message: String,
    },

    /// The value the path landed on is not one a binding carries: bindings
    /// move numbers, bools and strings.
    #[error("unsupported value type at '{at}'")]
    UnsupportedValueType {
        /// The path that landed on it.
        at: String,
    },

    /// The value read does not suit the field being written: a bool cannot
    /// drive a width, a number cannot drive a label's visibility.
    #[error("{target} target needs a {needs} value")]
    WriteTypeMismatch {
        /// The type of the field being written.
        target: &'static str,
        /// The shape of value that field would take.
        needs: &'static str,
    },

    /// The number a binding worked out does not fit the widget field it is
    /// aimed at: a health of 400 in a `u8`, or a NaN anywhere.
    #[error("{target} target cannot take {value}")]
    WriteOutOfRange {
        /// The type of the field being written.
        target: &'static str,
        /// The number that did not fit it.
        value: f32,
    },

    /// A binding reads the very field it writes, so what it writes is what it
    /// reads next. Only this direct case is caught; see the crate doc.
    #[error("binding reads and writes the same field '{path}'")]
    SelfCycle {
        /// The write path, which is also one of the reads.
        path: String,
    },

    /// The write path landed on a field of a type bindings cannot set.
    #[error("unsupported write target type")]
    UnsupportedWriteTarget,

    /// A write path spelled `Res(Type).field`. Writes go to the widget's own
    /// components; only a two-way `Value` binding writes back to a resource.
    #[error("write path '{raw}' must target a component")]
    WritePathNotComponent {
        /// The write path as authored.
        raw: String,
    },

    /// The write path names a marker component, one with nothing in it, and the
    /// binding worked out something other than a bool. A marker is there or it
    /// is not, and only a bool answers which.
    #[error("marker '{type_path}' is set by a bool: true puts it on, false takes it off")]
    MarkerNeedsBool {
        /// The marker the write path named.
        type_path: String,
    },

    /// The write path names a marker component reflection cannot build, so
    /// nothing can put one on an entity.
    #[error("marker '{type_path}' has no #[reflect(Default)], so nothing can put it on")]
    MarkerNotDefaultable {
        /// The marker the write path named.
        type_path: String,
    },

    /// The component the write path names is declared immutable, so nothing
    /// can set a field on it.
    #[error("'{type_path}' is immutable")]
    ImmutableComponent {
        /// The component the write path named.
        type_path: String,
    },

    /// The `via` function is not in the app's function registry.
    #[error("unknown function '{name}'")]
    UnknownFunction {
        /// The name the binding gave.
        name: String,
    },

    /// The `via` name is a short name several registered functions answer to.
    #[error("ambiguous function '{name}': {}", candidates.join(", "))]
    AmbiguousFunction {
        /// The short name that was not enough to go on.
        name: String,
        /// The full paths to choose between.
        candidates: Vec<String>,
    },

    /// The `via` function refused the values handed to it: the wrong number of
    /// reads, or the wrong types.
    #[error("call '{name}': {message}")]
    FunctionCall {
        /// The function that was called.
        name: String,
        /// What the call said about it.
        message: String,
    },

    /// The `via` function returned a borrow. A binding needs a value it can
    /// keep, so the function has to return one.
    #[error("function '{name}' must return an owned value")]
    NonOwnedReturn {
        /// The function that was called.
        name: String,
    },

    /// A `Field` binding reads several places without a `via` function to
    /// combine them into the one value it writes.
    #[error("Field binding has {count} reads but no via function to combine them")]
    MultipleReadsNoVia {
        /// How many reads the binding lists.
        count: usize,
    },

    /// A `Field` binding writes somewhere but reads nowhere.
    #[error("Field binding with no reads")]
    NoReads,

    /// A `Text` binding's format string has more `{}` slots than it has paths
    /// to fill them.
    #[error("more {{}} placeholders than args")]
    TooManyPlaceholders,

    /// A `Visible` binding read something other than a bool, so there is no
    /// shown-or-hidden answer in it.
    #[error("Visible binding must produce a bool")]
    VisibleNotBool,

    /// The binding needs a widget component the entity does not have: `Text`
    /// for a text binding, `SliderValue` for a slider's value.
    #[error("entity {entity} lacks '{component}'")]
    MissingWidgetComponent {
        /// The widget the binding was authored on.
        entity: Entity,
        /// The component that binding kind needs.
        component: &'static str,
    },

    /// A `Value` binding read a string, but the widget holds no text to put it
    /// in: either it is not a text widget, or nothing named where a widget's
    /// text lives.
    #[error("Value binding read a string, but the widget holds no text field to put it in")]
    StringValueNoTarget,

    /// A `Value` binding on a text widget read something that is not text.
    #[error("Value binding on a text widget read a {kind}, and a text field takes a string")]
    ValueNotAString {
        /// What the binding read instead of a string.
        kind: &'static str,
    },

    /// An `Action` binding names a type that is registered but is not an
    /// event, so nothing can be sent.
    #[error("'{event_path}' has no ReflectEvent (missing #[reflect(Event)])")]
    NotAnEvent {
        /// The type the binding named as its event.
        event_path: String,
    },

    /// An `Action` binding names an event whose fields have no names to fill:
    /// a tuple struct, an enum, anything reflection does not describe as a
    /// named struct. A binding maps paths to field names, and there are none
    /// here to map onto.
    #[error("'{event_path}' is a {kind}, and an action binding fills fields by name")]
    EventNotNamedStruct {
        /// The type the binding named as its event.
        event_path: String,
        /// What reflection says the type is instead: "tuple struct", "enum".
        kind: String,
    },

    /// An `Action` binding fills a field the event does not declare.
    #[error("event has no field '{field}'")]
    UnknownEventField {
        /// The field the binding tried to fill.
        field: String,
    },

    /// The value read cannot go in the event field it was mapped to.
    #[error("field '{field}' is '{type_path}' and cannot take a {kind} value")]
    EventFieldTypeMismatch {
        /// The event field being filled.
        field: String,
        /// The type that field is declared as.
        type_path: String,
        /// The shape of the value read: "numeric", "bool" or "string".
        kind: &'static str,
    },

    /// The number read does not fit the event field it was mapped to: a slot
    /// of 300 in a `u8`, or a NaN out of a `via` function.
    #[error("field '{field}' is '{type_path}' and cannot take {value}")]
    EventFieldOutOfRange {
        /// The event field being filled.
        field: String,
        /// The type that field is declared as.
        type_path: String,
        /// The number that did not fit it.
        value: f32,
    },

    /// An `Action` binding leaves an event field unmapped, and the event has
    /// no default to fill the gap with.
    #[error("'{event_path}' leaves field '{field}' unmapped and has no #[reflect(Default)]")]
    UnfillableEvent {
        /// The event the binding names.
        event_path: String,
        /// The field left with nothing to put in it.
        field: String,
    },

    /// An `Action` binding names an event that targets an entity, and nothing
    /// above the widget says which one. The event's own default would supply
    /// `Entity::PLACEHOLDER`, which is not an entity, so nothing is sent.
    #[error("'{event_path}' targets an entity through '{field}', and no BindContext says which")]
    MissingContext {
        /// The event the binding names.
        event_path: String,
        /// The field that takes the target entity.
        field: String,
    },

    /// An `Action` binding leaves an entity-typed field unmapped, and it is
    /// not the one the widget's context fills. Bind values carry numbers,
    /// bools and strings, so nothing else can fill it either.
    #[error(
        "'{event_path}' declares entity field '{field}', which a binding cannot fill -- only a \
         field named 'entity' takes the widget's context"
    )]
    UnfillableEntityField {
        /// The event the binding names.
        event_path: String,
        /// The entity field with nothing to put in it.
        field: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(s: &str) -> String {
        s.to_string()
    }

    /// Every message an author can be shown, pinned to its exact text.
    #[test]
    fn display_text_is_pinned() {
        let e = Entity::PLACEHOLDER;
        let cases: Vec<(BindError, String)> = vec![
            (
                BindError::MalformedPath {
                    raw: owned("Res(unclosed.field"),
                    reason: "unclosed Res(",
                },
                owned("unclosed Res( in 'Res(unclosed.field'"),
            ),
            (
                BindError::MalformedPath {
                    raw: owned("Res(A)x"),
                    reason: "missing field after Res()",
                },
                owned("missing field after Res() in 'Res(A)x'"),
            ),
            (
                BindError::MalformedPath {
                    raw: owned("A."),
                    reason: "empty segment",
                },
                owned("empty segment in 'A.'"),
            ),
            (
                BindError::MalformedPath {
                    raw: owned("no-dot-here"),
                    reason: "expected 'Type.field'",
                },
                owned("expected 'Type.field' in 'no-dot-here'"),
            ),
            (
                BindError::UnknownType {
                    noun: "type",
                    type_path: owned("Volume"),
                },
                owned("unknown type 'Volume'"),
            ),
            (
                BindError::UnknownType {
                    noun: "event type",
                    type_path: owned("Fired"),
                },
                owned("unknown event type 'Fired'"),
            ),
            (
                BindError::AmbiguousType {
                    type_path: owned("Volume"),
                    candidates: vec![owned("audio::Volume"), owned("render::Volume")],
                },
                owned(
                    "ambiguous short type path 'Volume': audio::Volume, render::Volume -- use the \
                     full type path",
                ),
            ),
            (BindError::NoTypeRegistry, owned("no type registry")),
            (BindError::NoFunctionRegistry, owned("no function registry")),
            (
                BindError::NoContext {
                    raw: owned("Health.current"),
                },
                owned("no context for 'Health.current'"),
            ),
            (
                BindError::NotAComponent {
                    type_path: owned("Health"),
                },
                owned("'Health' is not a component"),
            ),
            (
                BindError::NotAResource {
                    type_path: owned("Audio"),
                },
                owned("'Audio' is not a resource"),
            ),
            (
                BindError::ResourceNotPresent {
                    type_path: owned("Audio"),
                },
                owned("resource 'Audio' not present"),
            ),
            (
                BindError::ContextEntityMissing { entity: e },
                format!("context entity {e} does not exist"),
            ),
            (
                BindError::ContextMissingComponent {
                    type_path: owned("Health"),
                },
                owned("context entity lacks 'Health'"),
            ),
            (
                BindError::EntityMissing { entity: e },
                format!("entity {e} does not exist"),
            ),
            (
                BindError::EntityMissingComponent {
                    type_path: owned("Node"),
                },
                owned("entity lacks 'Node'"),
            ),
            (
                BindError::ReflectPath {
                    field: owned("current"),
                    type_path: owned("Health"),
                    message: owned("no field"),
                },
                owned("path 'current' on 'Health': no field"),
            ),
            (
                BindError::UnsupportedValueType {
                    at: owned("Health.name"),
                },
                owned("unsupported value type at 'Health.name'"),
            ),
            (
                BindError::WriteTypeMismatch {
                    target: "Val",
                    needs: "numeric",
                },
                owned("Val target needs a numeric value"),
            ),
            (
                BindError::WriteTypeMismatch {
                    target: "f32",
                    needs: "numeric",
                },
                owned("f32 target needs a numeric value"),
            ),
            (
                BindError::WriteTypeMismatch {
                    target: "bool",
                    needs: "bool",
                },
                owned("bool target needs a bool value"),
            ),
            (
                BindError::WriteTypeMismatch {
                    target: "String",
                    needs: "string",
                },
                owned("String target needs a string value"),
            ),
            (
                BindError::WriteTypeMismatch {
                    target: "Visibility",
                    needs: "bool",
                },
                owned("Visibility target needs a bool value"),
            ),
            (
                BindError::WriteOutOfRange {
                    target: "u8",
                    value: 400.0,
                },
                owned("u8 target cannot take 400"),
            ),
            (
                BindError::SelfCycle {
                    path: owned("Relay.value"),
                },
                owned("binding reads and writes the same field 'Relay.value'"),
            ),
            (
                BindError::UnsupportedWriteTarget,
                owned("unsupported write target type"),
            ),
            (
                BindError::WritePathNotComponent {
                    raw: owned("Res(Audio).master"),
                },
                owned("write path 'Res(Audio).master' must target a component"),
            ),
            (
                BindError::MarkerNeedsBool {
                    type_path: owned("InteractionDisabled"),
                },
                owned(
                    "marker 'InteractionDisabled' is set by a bool: true puts it on, false takes \
                     it off",
                ),
            ),
            (
                BindError::MarkerNotDefaultable {
                    type_path: owned("Locked"),
                },
                owned("marker 'Locked' has no #[reflect(Default)], so nothing can put it on"),
            ),
            (
                BindError::ImmutableComponent {
                    type_path: owned("Locked"),
                },
                owned("'Locked' is immutable"),
            ),
            (
                BindError::UnknownFunction {
                    name: owned("scale"),
                },
                owned("unknown function 'scale'"),
            ),
            (
                BindError::AmbiguousFunction {
                    name: owned("scale"),
                    candidates: vec![owned("hotbar::scale"), owned("hud::scale")],
                },
                owned("ambiguous function 'scale': hotbar::scale, hud::scale"),
            ),
            (
                BindError::FunctionCall {
                    name: owned("scale"),
                    message: owned("ArgCountMismatch"),
                },
                owned("call 'scale': ArgCountMismatch"),
            ),
            (
                BindError::NonOwnedReturn {
                    name: owned("borrowed"),
                },
                owned("function 'borrowed' must return an owned value"),
            ),
            (
                BindError::MultipleReadsNoVia { count: 2 },
                owned("Field binding has 2 reads but no via function to combine them"),
            ),
            (BindError::NoReads, owned("Field binding with no reads")),
            (
                BindError::TooManyPlaceholders,
                owned("more {} placeholders than args"),
            ),
            (
                BindError::VisibleNotBool,
                owned("Visible binding must produce a bool"),
            ),
            (
                BindError::MissingWidgetComponent {
                    entity: e,
                    component: "Text",
                },
                format!("entity {e} lacks 'Text'"),
            ),
            (
                BindError::StringValueNoTarget,
                owned(
                    "Value binding read a string, but the widget holds no text field to put it in",
                ),
            ),
            (
                BindError::ValueNotAString { kind: "number" },
                owned(
                    "Value binding on a text widget read a number, and a text field takes a string",
                ),
            ),
            (
                BindError::NotAnEvent {
                    event_path: owned("Fired"),
                },
                owned("'Fired' has no ReflectEvent (missing #[reflect(Event)])"),
            ),
            (
                BindError::EventNotNamedStruct {
                    event_path: owned("Fired"),
                    kind: owned("tuple struct"),
                },
                owned("'Fired' is a tuple struct, and an action binding fills fields by name"),
            ),
            (
                BindError::UnknownEventField {
                    field: owned("amount"),
                },
                owned("event has no field 'amount'"),
            ),
            (
                BindError::EventFieldTypeMismatch {
                    field: owned("amount"),
                    type_path: owned("String"),
                    kind: "numeric",
                },
                owned("field 'amount' is 'String' and cannot take a numeric value"),
            ),
            (
                BindError::EventFieldOutOfRange {
                    field: owned("slot"),
                    type_path: owned("u8"),
                    value: 300.0,
                },
                owned("field 'slot' is 'u8' and cannot take 300"),
            ),
            (
                BindError::UnfillableEvent {
                    event_path: owned("Fired"),
                    field: owned("amount"),
                },
                owned("'Fired' leaves field 'amount' unmapped and has no #[reflect(Default)]"),
            ),
            (
                BindError::MissingContext {
                    event_path: owned("Fired"),
                    field: owned("entity"),
                },
                owned("'Fired' targets an entity through 'entity', and no BindContext says which"),
            ),
            (
                BindError::UnfillableEntityField {
                    event_path: owned("Aimed"),
                    field: owned("at"),
                },
                owned(
                    "'Aimed' declares entity field 'at', which a binding cannot fill -- only a \
                     field named 'entity' takes the widget's context",
                ),
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected, "{error:?}");
        }
    }
}
