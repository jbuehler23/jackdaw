//! The row shape for a labeled control: label left in a fixed column, control
//! right, uniform height.
//!
//! Used by every panel that stacks fields down a column, so a label sits at
//! the same x whatever the control beside it is, and a row is the same height
//! whether it holds a checkbox or a text entry. Sub-values indent under their
//! feature via [`FieldRowProps::indented`], which takes the indent out of the
//! label column rather than pushing the control right, so indented rows keep
//! their controls aligned.

use bevy::prelude::*;

use crate::tokens;

/// What a spawned field row hands back.
pub struct FieldRow {
    /// The row itself: where a marker, tooltip or observer goes.
    pub row: Entity,
    /// The right-hand slot the caller fills with its widget.
    pub control: Entity,
}

#[derive(Clone, Default)]
pub struct FieldRowProps {
    pub label: String,
    /// Nesting depth; each level insets by [`tokens::FIELD_INDENT`].
    pub indent: u8,
    /// Room this row's control needs before the row wraps instead, for a
    /// widget needing more than [`tokens::FIELD_CONTROL_MIN_WIDTH`] to stay
    /// readable. `None` takes the shared floor.
    pub control_min_width: Option<f32>,
}

impl FieldRowProps {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            indent: 0,
            control_min_width: None,
        }
    }

    pub fn indented(mut self, levels: u8) -> Self {
        self.indent = levels;
        self
    }

    /// Raise this row's control floor. Raises only: a control asking for less
    /// than the shared floor gets the shared one, so no row undercuts the
    /// column.
    pub fn with_control_min_width(mut self, width: f32) -> Self {
        self.control_min_width = Some(width.max(tokens::FIELD_CONTROL_MIN_WIDTH));
        self
    }

    fn inset(&self) -> f32 {
        f32::from(self.indent) * tokens::FIELD_INDENT
    }

    fn control_width(&self) -> f32 {
        self.control_min_width
            .unwrap_or(tokens::FIELD_CONTROL_MIN_WIDTH)
    }
}

/// Spawn a labeled row under `parent`.
pub fn spawn_field_row(commands: &mut Commands, parent: Entity, props: FieldRowProps) -> FieldRow {
    let inset = props.inset();
    let control_min = props.control_width();

    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                // In a panel too narrow for label and control side by side,
                // the control drops onto its own line rather than being
                // squeezed to nothing or clipped off the edge.
                flex_wrap: FlexWrap::Wrap,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_SM),
                row_gap: Val::Px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                min_height: Val::Px(tokens::FIELD_ROW_HEIGHT),
                padding: UiRect::left(Val::Px(inset)),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    commands.spawn((
        Text::new(props.label),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            width: Val::Px((tokens::FIELD_LABEL_WIDTH - inset).max(0.0)),
            // The label column is a target, not a floor: in a panel too
            // narrow for it the label shrinks, down to a width that still
            // reads as a word, rather than pushing the control off the edge.
            min_width: Val::Px(tokens::FIELD_LABEL_MIN_WIDTH),
            flex_shrink: 1.0,
            overflow: Overflow::clip(),
            ..Default::default()
        },
        ChildOf(row),
    ));

    let control = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                flex_grow: 1.0,
                flex_shrink: 1.0,
                // The basis is what decides whether this fits beside the
                // label or wraps under it.
                flex_basis: Val::Px(control_min),
                min_width: Val::Px(control_min),
                ..Default::default()
            },
            ChildOf(row),
        ))
        .id();

    FieldRow { row, control }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct Spawned(Option<(Entity, Entity)>);

    fn spawn(indent: u8) -> App {
        let mut app = App::new();
        app.init_resource::<Spawned>();
        let id = app.world_mut().register_system(
            move |mut commands: Commands, mut out: ResMut<Spawned>| {
                let parent = commands.spawn(Node::default()).id();
                let row = spawn_field_row(
                    &mut commands,
                    parent,
                    FieldRowProps::new("Metallic").indented(indent),
                );
                out.0 = Some((row.row, row.control));
            },
        );
        app.world_mut().run_system(id).unwrap();
        app.world_mut().flush();
        app
    }

    /// An indented row takes its inset out of the label, so every control
    /// starts at the same x as its unindented neighbours.
    #[test]
    fn indenting_narrows_the_label_instead_of_moving_the_control() {
        for indent in [0u8, 1, 2] {
            let mut app = spawn(indent);
            let (row, _) = app.world().resource::<Spawned>().0.unwrap();
            let label = app.world().get::<Children>(row).unwrap()[0];
            let inset = f32::from(indent) * tokens::FIELD_INDENT;

            let row_node = app.world().get::<Node>(row).unwrap();
            assert_eq!(row_node.padding.left, Val::Px(inset));
            let label_node = app.world().get::<Node>(label).unwrap();
            assert_eq!(
                label_node.width,
                Val::Px(tokens::FIELD_LABEL_WIDTH - inset),
                "indent must come out of the label column"
            );
            app.world_mut().clear_all();
        }
    }

    #[test]
    fn every_row_is_at_least_one_row_high() {
        let app = spawn(0);
        let (row, _) = app.world().resource::<Spawned>().0.unwrap();
        assert_eq!(
            app.world().get::<Node>(row).unwrap().min_height,
            Val::Px(tokens::FIELD_ROW_HEIGHT)
        );
    }
}
