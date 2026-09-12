//! A field row whose control is an asset reference: square swatch, the
//! name of what is bound, then the buttons that change it.
//!
//! Built on [`crate::field_row`], so a swatch row lines up with the plain
//! rows above and below it. Nothing here knows what kind of asset it
//! shows - the caller supplies the image and fills [`SwatchRow::actions`].

use bevy::prelude::*;

use crate::field_row::{FieldRowProps, spawn_field_row};
use crate::tokens;

/// What a spawned swatch row hands back.
pub struct SwatchRow {
    pub row: Entity,
    /// The square image node, for a caller that wants to observe it.
    pub swatch: Entity,
    /// The text naming what is bound, for a caller that keeps it in step.
    pub value: Entity,
    /// Trailing slot for assign/clear buttons.
    pub actions: Entity,
}

#[derive(Clone)]
pub struct SwatchRowProps {
    pub label: String,
    /// Name of the bound asset, or the placeholder shown when none is.
    pub value: String,
    /// `None` draws an empty swatch rather than leaving a hole.
    pub image: Option<Handle<Image>>,
    pub indent: u8,
    /// Dim the value text: nothing is bound, so the text is a placeholder
    /// rather than a name.
    pub unbound: bool,
    /// Room the swatch, the name and the buttons need before the row wraps.
    pub control_min_width: Option<f32>,
}

impl SwatchRowProps {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: String::new(),
            image: None,
            indent: 0,
            unbound: true,
            control_min_width: None,
        }
    }

    /// Bind an image and the name it goes by.
    pub fn bound(mut self, image: Handle<Image>, value: impl Into<String>) -> Self {
        self.image = Some(image);
        self.value = value.into();
        self.unbound = false;
        self
    }

    pub fn placeholder(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    pub fn indented(mut self, levels: u8) -> Self {
        self.indent = levels;
        self
    }

    /// Raise the floor under the control, so a row carrying more than a name
    /// wraps rather than squeezing the name away.
    pub fn with_control_min_width(mut self, width: f32) -> Self {
        self.control_min_width = Some(width);
        self
    }
}

pub fn spawn_swatch_row(
    commands: &mut Commands,
    parent: Entity,
    props: SwatchRowProps,
) -> SwatchRow {
    let mut row_props = FieldRowProps::new(props.label).indented(props.indent);
    if let Some(width) = props.control_min_width {
        row_props = row_props.with_control_min_width(width);
    }
    let field = spawn_field_row(commands, parent, row_props);

    let mut swatch = commands.spawn((
        Node {
            width: Val::Px(tokens::SWATCH_SIZE),
            height: Val::Px(tokens::SWATCH_SIZE),
            flex_shrink: 0.0,
            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
            ..Default::default()
        },
        BackgroundColor(tokens::INPUT_BG),
        ChildOf(field.control),
    ));
    if let Some(image) = props.image {
        swatch.insert(ImageNode::new(image));
    }
    let swatch = swatch.id();

    // The text cannot clip its own glyphs, so it rides in a box that can: a
    // name too long for the row is cut at the edge rather than drawn over
    // whatever sits beside it.
    let name_box = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_width: Val::Px(0.0),
                overflow: Overflow::clip(),
                ..Default::default()
            },
            ChildOf(field.control),
        ))
        .id();
    let value = commands
        .spawn((
            Text::new(props.value),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(if props.unbound {
                tokens::TEXT_DISABLED
            } else {
                tokens::TEXT_TERTIARY
            }),
            Node {
                flex_shrink: 0.0,
                ..Default::default()
            },
            ChildOf(name_box),
        ))
        .id();

    let actions = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                flex_shrink: 0.0,
                ..Default::default()
            },
            ChildOf(field.control),
        ))
        .id();

    SwatchRow {
        row: field.row,
        swatch,
        value,
        actions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;

    #[derive(Resource, Default)]
    struct Spawned(Option<Entity>);

    fn spawn_with(props: SwatchRowProps) -> App {
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_asset::<Image>();
        app.init_resource::<Spawned>();
        let id = app.world_mut().register_system(
            move |mut commands: Commands, mut out: ResMut<Spawned>| {
                let parent = commands.spawn(Node::default()).id();
                out.0 = Some(spawn_swatch_row(&mut commands, parent, props.clone()).swatch);
            },
        );
        app.world_mut().run_system(id).unwrap();
        app.world_mut().flush();
        app
    }

    /// An unbound slot still draws its square, so a column of slots reads
    /// as a column rather than as gaps between the filled ones.
    #[test]
    fn an_unbound_row_keeps_its_swatch_and_drops_the_image() {
        let app = spawn_with(SwatchRowProps::new("Normal").placeholder("None"));
        let swatch = app.world().resource::<Spawned>().0.unwrap();
        assert_eq!(
            app.world().get::<Node>(swatch).unwrap().width,
            Val::Px(tokens::SWATCH_SIZE)
        );
        assert!(app.world().get::<ImageNode>(swatch).is_none());
    }

    #[test]
    fn a_bound_row_shows_its_image_in_the_same_square() {
        let app =
            spawn_with(SwatchRowProps::new("Normal").bound(Handle::default(), "rock_nor.png"));
        let swatch = app.world().resource::<Spawned>().0.unwrap();
        assert_eq!(
            app.world().get::<Node>(swatch).unwrap().height,
            Val::Px(tokens::SWATCH_SIZE)
        );
        assert!(app.world().get::<ImageNode>(swatch).is_some());
    }
}
