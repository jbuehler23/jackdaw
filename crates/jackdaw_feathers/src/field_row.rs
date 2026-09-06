//! The one row shape for a labeled control: label left in a fixed column, control
//! right, uniform height.
//!
//! Every panel that stacks fields down a column uses this. Sub-values indent under
//! their feature via [`FieldRowProps::indented`], which takes the indent out of
//! the label column rather than pushing the control right.

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
    /// Room this row's control needs before the row should wrap instead,
    /// for a widget that needs more than [`tokens::FIELD_CONTROL_MIN_WIDTH`]
    /// to stay readable. `None` takes the shared floor.
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

    /// Raise this row's control floor. Only ever raises it: a control
    /// asking for less than the shared floor still gets the shared one,
    /// so no row can undercut the column.
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

/// Marker on a row [`spawn_field_row`] built, so the gutter pass can find it.
#[derive(Component)]
pub struct FieldRowNode;

/// Marker on a mark hung off a field row: the prefab override dot, the live-edit
/// dot, the keyframe diamond. Goes on the mark's own absolutely-positioned
/// wrapper, a direct child of the row.
///
/// A mark anchors to the row's right edge, so a row showing one has to give up
/// that strip. Most rows never show one, so the strip is taken only while a mark
/// is present rather than reserved on every row.
#[derive(Component)]
pub struct FieldRowDecoration;

/// Take [`tokens::FIELD_DECORATION_GUTTER`] out of a row showing a mark and
/// give it back when the mark goes.
pub fn reserve_decoration_gutters(
    marks: Query<&ChildOf, With<FieldRowDecoration>>,
    mut rows: Query<(Entity, &mut Node), With<FieldRowNode>>,
) {
    let decorated: bevy::platform::collections::HashSet<Entity> =
        marks.iter().map(ChildOf::parent).collect();
    for (entity, mut node) in &mut rows {
        let wanted = if decorated.contains(&entity) {
            Val::Px(tokens::FIELD_DECORATION_GUTTER)
        } else {
            Val::Px(0.0)
        };
        if node.padding.right != wanted {
            node.padding.right = wanted;
        }
    }
}

/// Spawn a labeled row under `parent`.
pub fn spawn_field_row(commands: &mut Commands, parent: Entity, props: FieldRowProps) -> FieldRow {
    let inset = props.inset();
    let control_min = props.control_width();

    let row = commands
        .spawn((
            FieldRowNode,
            Node {
                flex_direction: FlexDirection::Row,
                // In a panel too narrow to hold label and control side by
                // side, the control drops onto its own line rather than
                // being squeezed to nothing or clipped off the edge.
                flex_wrap: FlexWrap::Wrap,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_SM),
                row_gap: Val::Px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                min_height: Val::Px(tokens::FIELD_ROW_HEIGHT),
                padding: UiRect {
                    left: Val::Px(inset),
                    ..Default::default()
                },
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
            // The label column is a target, not a floor: in a panel too narrow
            // to hold it the label gives way rather than pushing the control off
            // the edge.
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
                // The basis is the ask; the row's own width is the ceiling. A
                // floor here would let a control wider than the row hang past the
                // panel's edge, where the panel clips it away.
                min_width: Val::Px(0.0),
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

    /// The label column is the alignment contract: an indented row takes
    /// its inset out of the label, so every control still starts at the
    /// same x as its unindented neighbours.
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

    /// The strip is taken when a mark appears and given back when it goes.
    /// That the control really stops short of it is measured against a
    /// layout pass in the editor's `inspector_val` suite.
    #[test]
    fn the_gutter_comes_and_goes_with_the_mark() {
        let mut app = spawn(0);
        let (row, _) = app.world().resource::<Spawned>().0.unwrap();
        let gutter = Val::Px(tokens::FIELD_DECORATION_GUTTER);

        let id = app.world_mut().register_system(reserve_decoration_gutters);
        app.world_mut().run_system(id).unwrap();
        assert_eq!(
            app.world().get::<Node>(row).unwrap().padding.right,
            Val::Px(0.0),
            "an unmarked row keeps its full width",
        );

        let mark = app
            .world_mut()
            .spawn((FieldRowDecoration, ChildOf(row)))
            .id();
        app.world_mut().run_system(id).unwrap();
        assert_eq!(
            app.world().get::<Node>(row).unwrap().padding.right,
            gutter,
            "a marked row clears the strip the mark lands in",
        );

        app.world_mut().entity_mut(mark).despawn();
        app.world_mut().run_system(id).unwrap();
        assert_eq!(
            app.world().get::<Node>(row).unwrap().padding.right,
            Val::Px(0.0),
            "the strip comes back when the mark goes",
        );
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
