use bevy::platform::collections::HashSet;
use bevy::prelude::*;

use crate::BindError;

/// Where a binding reads or writes: `Component.field` on the context entity,
/// or `Res(Type).field` for a resource. The type half takes either a full
/// type path or the trailing name alone, as long as only one registered type
/// answers to it; the field half is a reflect path, so it can go as deep as
/// `margin.left`.
// Held as the authored text so the document round-trips exactly what was
// written; parsed at evaluation time.
#[derive(Clone, Debug, PartialEq, Reflect, Default)]
pub struct BindPath {
    /// The path exactly as it was authored.
    pub raw: String,
}

/// A [`BindPath`] split into the type it names and the field inside it.
#[derive(Debug, PartialEq)]
pub enum ParsedPath {
    /// `Type.field`, read from the context entity.
    Component {
        /// The type the path names, full or short.
        type_path: String,
        /// The reflect path to the field inside that type.
        field: String,
    },
    /// `Res(Type).field`, read from a resource and needing no context.
    Resource {
        /// The resource type the path names, full or short.
        type_path: String,
        /// The reflect path to the field inside that resource.
        field: String,
    },
}

impl BindPath {
    /// Takes a path as authored. Nothing is checked here: a path that names
    /// no type is a path an author is still halfway through typing.
    pub fn new(raw: impl Into<String>) -> Self {
        Self { raw: raw.into() }
    }

    /// The component this path names when it names no field inside one.
    ///
    /// A write path spelled that way puts a marker component on or takes it
    /// off, driven by a bool. Only a write path can be spelled this way.
    pub fn marker_type(&self) -> Option<&str> {
        let raw = self.raw.trim();
        if raw.is_empty() || raw.contains('.') || raw.starts_with("Res(") {
            return None;
        }
        Some(raw)
    }

    /// Splits the path into its type and field halves, or says what is wrong
    /// with the spelling. Types are not looked up here; that waits until there
    /// is a world to look them up in.
    pub fn parse(&self) -> Result<ParsedPath, BindError> {
        let malformed = |reason| BindError::MalformedPath {
            raw: self.raw.clone(),
            reason,
        };
        if let Some(rest) = self.raw.strip_prefix("Res(") {
            let (type_path, tail) = rest
                .split_once(')')
                .ok_or_else(|| malformed("unclosed Res("))?;
            let field = tail
                .strip_prefix('.')
                .ok_or_else(|| malformed("missing field after Res()"))?;
            if type_path.is_empty() || field.is_empty() {
                return Err(malformed("empty segment"));
            }
            return Ok(ParsedPath::Resource {
                type_path: type_path.to_string(),
                field: field.to_string(),
            });
        }
        let (type_path, field) = self
            .raw
            .split_once('.')
            .ok_or_else(|| malformed("expected 'Type.field'"))?;
        if type_path.is_empty() || field.is_empty() {
            return Err(malformed("empty segment"));
        }
        Ok(ParsedPath::Component {
            type_path: type_path.to_string(),
            field: field.to_string(),
        })
    }
}

/// One connection a widget makes: a property it drives from game state, the
/// text it formats, whether it shows at all, the value it keeps in step, or
/// the event it fires.
#[derive(Clone, Debug, PartialEq, Reflect)]
pub enum Binding {
    /// Drives one field of one of the widget's own components from game state:
    /// a health bar's width from the player's health.
    ///
    /// A write path that names a component and no field drives the whole
    /// component instead, putting a marker on or taking it off.
    Field {
        /// Where the value comes from. More than one read needs a `via`
        /// function to combine them.
        read: Vec<BindPath>,
        /// A registered function the reads pass through on the way in.
        via: Option<String>,
        /// The field on this widget that takes the result, or a marker
        /// component the result puts on and takes off.
        write: BindPath,
        /// Treat the result as a fraction and write it as a percentage, which
        /// is how a `Val` field expresses a bar filling up.
        as_percent: bool,
    },
    /// Keeps the widget's text in step with game state. `{}` in the format
    /// string takes the next value in `args`.
    Text {
        /// The sentence, with `{}` where each value goes.
        format: String,
        /// The values, in the order the placeholders take them.
        args: Vec<BindPath>,
    },
    /// Shows the widget while the value it reads is true, hides it otherwise.
    Visible {
        /// Where the shown-or-hidden answer comes from.
        read: BindPath,
        /// A registered function the read passes through, for when the answer
        /// is not already a bool.
        via: Option<String>,
    },
    /// Ties a slider or checkbox to a value: the widget follows the value,
    /// and when `two_way` the value follows the widget.
    Value {
        /// The value the widget stays in step with.
        with: BindPath,
        /// Whether the author's own edits write back to that value.
        two_way: bool,
    },
    /// Sends an event when the widget is activated, with its fields filled
    /// from game state.
    Action {
        /// The event type to send.
        event: String,
        /// Which path fills which field of the event. Fields left out take
        /// the event's own default.
        fields: Vec<(String, BindPath)>,
    },
}

/// Declarative connections from this widget to game state and events.
// The sentence above is picker copy shown verbatim, so it stays one plain line.
// `Default` is reflected as well as derived, or the picker cannot synthesise an
// instance.
#[derive(Component, Clone, Debug, PartialEq, Reflect, Default)]
#[reflect(Component, Default)]
pub struct Bindings(pub Vec<Binding>);

/// Subject entity that component paths resolve against. Inherited by
/// descendants: resolution walks up `ChildOf` until one is found, so a panel
/// can name the subject once and every widget under it reads from there.
#[derive(Component, Clone, Copy, Debug, Reflect)]
#[reflect(Component)]
pub struct BindContext(
    /// The entity whose components the bindings below this one read.
    #[entities]
    pub Entity,
);

/// Where a string [`Binding::Value`] puts the text it reads: a widget component
/// and the field inside it, as a write path.
///
/// Bevy has no reflectable text-input value, and this crate does not depend on
/// the one that supplies it, so whoever composes the two names the target here.
/// Without it a string `Value` binding has nowhere to land and says so.
#[derive(Resource, Clone, Debug)]
pub struct ValueTextTarget(
    /// The `Type.field` write path, relative to the bound widget.
    pub BindPath,
);

/// Warn-once ledger: (entity, binding index) pairs already reported, since a
/// binding that fails does so every frame.
#[derive(Resource, Default)]
pub struct BindFailures(
    /// The (entity, binding index) pairs already warned about.
    pub HashSet<(Entity, usize)>,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_component_path_with_full_type_path() {
        let p = BindPath::new("game::hud::Health.current");
        match p.parse().unwrap() {
            ParsedPath::Component { type_path, field } => {
                assert_eq!(type_path, "game::hud::Health");
                assert_eq!(field, "current");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn parses_short_component_path() {
        let p = BindPath::new("Health.current");
        match p.parse().unwrap() {
            ParsedPath::Component { type_path, field } => {
                assert_eq!(type_path, "Health");
                assert_eq!(field, "current");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn parses_resource_path() {
        let p = BindPath::new("Res(game::AudioSettings).master");
        match p.parse().unwrap() {
            ParsedPath::Resource { type_path, field } => {
                assert_eq!(type_path, "game::AudioSettings");
                assert_eq!(field, "master");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn nested_field_paths_survive() {
        let p = BindPath::new("Node.margin.left");
        match p.parse().unwrap() {
            ParsedPath::Component { type_path, field } => {
                assert_eq!(type_path, "Node");
                assert_eq!(field, "margin.left");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(BindPath::new("no-dot-here").parse().is_err());
        assert!(BindPath::new("Res(unclosed.field").parse().is_err());
    }

    #[test]
    fn a_path_with_no_field_names_a_whole_component() {
        assert_eq!(
            BindPath::new("bevy_ui::InteractionDisabled").marker_type(),
            Some("bevy_ui::InteractionDisabled"),
        );
        assert_eq!(
            BindPath::new("InteractionDisabled").marker_type(),
            Some("InteractionDisabled")
        );
    }

    #[test]
    fn a_path_that_does_name_a_field_is_not_a_marker() {
        assert_eq!(BindPath::new("Health.current").marker_type(), None);
        assert_eq!(
            BindPath::new("Res(AudioSettings).master").marker_type(),
            None
        );
        assert_eq!(BindPath::new("").marker_type(), None);
    }
}
