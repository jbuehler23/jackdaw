//! The inspector takes the width the dock gives it.
//!
//! A panel's width belongs to the dock: the user drags a split, the leaf
//! resizes, and every card inside follows. Flexbox's automatic minimum
//! size works against that. Without a `min_width` on the column holding
//! the cards its floor is its own min-content width, so one unbreakable
//! component title (`Inherited<ComputedUiRenderTargetInfo>` measures about
//! 250 px in the shipped font) lifts that floor above the sidebar's width.
//! Every card, row and control below the column then lays out at the wider
//! figure and the surplus is cut off at the panel's edge, down to a `Val`
//! row's unit dropdown rendering as a single clipped glyph.
//!
//! The `inspector_val` matrix cannot see this. It parents a row to an
//! absolutely-positioned `width: Px(212)` column, definite by
//! construction, so where the width comes from is answered there by the
//! harness. The wide unshrinkable child is the load-bearing case: the
//! headless stand-in for a title a real font has measured, since headless
//! text measures nothing.

use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use jackdaw::inspector::node_card::NodeCardBody;
use jackdaw::selection::Selection;

mod util;

/// The shipped right sidebar, as a dock leaf hands it over: a definite
/// width, clipping, laid out as a column.
const PANEL_WIDTH: f32 = 231.0;

/// What a long generic type name measures at once a real font has seen it.
const LONG_TITLE_WIDTH: f32 = 250.0;

/// The width a unit dropdown stops reading at: "vmin" plus the chevron.
/// The floor `inspector_val` pins; read here to prove the row reached it.
const UNIT_DROPDOWN_FLOOR: f32 = 34.0;

/// The inspector panel mounted in a dock leaf of a fixed width, with a
/// `Node`-carrying entity selected so the card dispatch has something to
/// build. Returns the leaf and the card body.
fn panel_with_a_node_card() -> (App, Entity, Entity) {
    let mut app = util::editor_test_app();
    let leaf = app
        .world_mut()
        .spawn(Node {
            position_type: PositionType::Absolute,
            width: Val::Px(PANEL_WIDTH),
            height: Val::Px(900.0),
            flex_direction: FlexDirection::Column,
            overflow: Overflow::clip(),
            ..default()
        })
        .id();
    let content = app
        .world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()))
        .id();
    app.world_mut().entity_mut(content).insert(ChildOf(leaf));

    let entity = app
        .world_mut()
        .spawn((Name::new("ui"), Node::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    let world = app.world_mut();
    world.resource_scope(|world, mut selection: Mut<Selection>| {
        let mut commands = world.commands();
        selection.select_single(&mut commands, entity);
    });
    world.flush();
    app.update();
    app.update();
    app.update();

    let body = app
        .world_mut()
        .query_filtered::<Entity, With<NodeCardBody>>()
        .iter(app.world())
        .next()
        .expect("a Node selection builds the card");

    // A card ships collapsed in a fresh app and opens on a pointer gesture;
    // headless, the body is shown by hand so its rows lay out.
    let mut cursor = Some(body);
    while let Some(entity) = cursor {
        if let Some(mut node) = app.world_mut().get_mut::<Node>(entity)
            && node.display == Display::None
        {
            node.display = Display::Flex;
        }
        cursor = app.world().get::<ChildOf>(entity).map(ChildOf::parent);
    }
    app.update();

    (app, leaf, body)
}

/// The scrollable card list.
fn inspector(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<jackdaw::inspector::Inspector>>()
        .iter(app.world())
        .next()
        .expect("the panel mounts an inspector")
}

/// Hang a child of `width` on the card list that refuses to shrink, the
/// stand-in for an unbreakable title.
fn add_an_unshrinkable_child(app: &mut App, width: f32) {
    let list = inspector(app);
    app.world_mut().spawn((
        Node {
            width: Val::Px(width),
            height: Val::Px(4.0),
            flex_shrink: 0.0,
            ..default()
        },
        ChildOf(list),
    ));
    app.update();
}

/// Every entity under `root`, `root` excluded.
fn descendants(app: &mut App, root: Entity) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut query = app.world_mut().query::<&Children>();
    while let Some(entity) = stack.pop() {
        if entity != root {
            out.push(entity);
        }
        if let Ok(children) = query.get(app.world(), entity) {
            stack.extend(children.iter());
        }
    }
    out
}

/// How far past `leaf`'s right edge anything under `root` reaches. The scan
/// starts at the card, not at the leaf: the stand-in title is a 250 px
/// unshrinkable box hung on the card list by hand, so it overhangs by
/// construction. What must not overhang is the card it widens.
fn overhang(app: &mut App, leaf: Entity, root: Entity) -> f32 {
    let leaf_right = right_edge(app, leaf);
    descendants(app, root)
        .into_iter()
        .map(|entity| right_edge(app, entity) - leaf_right)
        .fold(0.0, f32::max)
}

fn right_edge(app: &App, entity: Entity) -> f32 {
    let (Some(computed), Some(transform)) = (
        app.world().get::<ComputedNode>(entity),
        app.world().get::<UiGlobalTransform>(entity),
    ) else {
        return f32::NEG_INFINITY;
    };
    transform.translation.x + computed.size().x / 2.0
}

/// One child that will not shrink must not widen the column it sits in.
/// Without a `min_width` floor on that column a 250 px child takes the card
/// list to 258 px inside a 231 px panel, and every card with it.
#[test]
fn a_title_too_wide_for_the_panel_does_not_widen_the_panel() {
    let (mut app, _, body) = panel_with_a_node_card();
    add_an_unshrinkable_child(&mut app, LONG_TITLE_WIDTH);

    let body_width = app.world().get::<ComputedNode>(body).unwrap().size().x;
    assert!(
        body_width <= PANEL_WIDTH,
        "a {LONG_TITLE_WIDTH} px title widened the card column: the card body \
         is {body_width} px inside a {PANEL_WIDTH} px panel, so every row \
         below it lays out at that width and is cut off at the panel's edge",
    );
}

/// Nothing under the leaf may reach past the panel's edge, whatever a title
/// asks for. This covers every card the inspector stacks in that column,
/// including the bindings card's chips and summary line.
#[test]
fn no_control_hangs_off_the_panels_edge() {
    let (mut app, leaf, body) = panel_with_a_node_card();
    let plain = overhang(&mut app, leaf, body);
    assert!(
        plain <= 0.5,
        "something on the card overhangs the panel's edge by {plain} px",
    );

    add_an_unshrinkable_child(&mut app, LONG_TITLE_WIDTH);
    let crowded = overhang(&mut app, leaf, body);
    assert!(
        crowded <= 0.5,
        "with a {LONG_TITLE_WIDTH} px title on the card list, something on \
         the card overhangs the panel's edge by {crowded} px",
    );
}

/// A length's unit dropdown keeps a width its labels read at, which it only
/// does if the row was handed the panel's width rather than a wider
/// column's.
#[test]
fn a_unit_dropdown_still_reads_with_a_wide_title_on_the_list() {
    let (mut app, _, body) = panel_with_a_node_card();
    add_an_unshrinkable_child(&mut app, LONG_TITLE_WIDTH);

    // A card's collapsed groups keep their rows in the tree with
    // `display: None`, and those lay out at nothing on both axes. Only a
    // dropdown that is on screen was handed a width, so the ones with no
    // height are skipped.
    let dropdowns: Vec<Entity> = descendants(&mut app, body)
        .into_iter()
        .filter(|entity| {
            app.world()
                .get::<jackdaw_feathers::combobox::EditorComboBox>(*entity)
                .is_some()
        })
        .filter(|entity| {
            app.world()
                .get::<ComputedNode>(*entity)
                .is_some_and(|computed| computed.size().y > 0.0)
        })
        .collect();
    assert!(!dropdowns.is_empty(), "the Node card lays out dropdowns");

    for dropdown in dropdowns {
        let width = app.world().get::<ComputedNode>(dropdown).unwrap().size().x;
        assert!(
            width >= UNIT_DROPDOWN_FLOOR,
            "a dropdown keeps {UNIT_DROPDOWN_FLOOR} px to read in; it got {width} px",
        );
    }
}
