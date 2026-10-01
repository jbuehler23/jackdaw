//! Rows built only where they can be seen.
//!
//! A scrolling Outliner keeps a row out of view as a frame one row high, and
//! builds what the row shows once it scrolls near the view. A scene of
//! thousands of top-level nodes then costs a frame per node rather than a
//! label, toggles and drop gaps per node.
//!
//! A frame further still is parked: it leaves UI layout altogether, and the
//! room it takes is kept as a margin on the next row still laid out, so the
//! list scrolls the same distance. Each pass works out where every row would
//! sit from the rows laid out and the height of a frame.
//!
//! A row stays built while anything is working with it: an open branch, a
//! selected or focused row, a rename, or a drag. Focus and selection also scroll
//! their row into view, which is what builds it.

use bevy::prelude::*;
use bevy::ui::{ComputedNode, ScrollPosition, UiGlobalTransform};
use jackdaw_feathers::tree_view::{
    TreeRowStyle, set_row_expand_toggle, tree_row_height, tree_row_parts,
};
use jackdaw_widgets::tree_view::{
    TreeFocused, TreeIndex, TreeNode, TreeNodeExpanded, TreeRowChildren, TreeRowContent,
    TreeRowParked, TreeRowSelected, row_is_hidden,
};

use crate::hierarchy::HierarchyTreeContainer;
use crate::selection::{Selected, Selection};

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<RowToShow>()
        .init_resource::<RowDragActive>()
        .add_observer(note_drag_start)
        .add_observer(note_drag_end)
        .add_systems(
            PostUpdate,
            (
                follow_focus_and_selection,
                show_the_row,
                build_rows_in_view.run_if(rows_may_have_moved),
            )
                .chain()
                .after(bevy::ui::UiSystems::Layout),
        );
}

/// How far beyond the view, in rows, rows are built ahead of a scroll.
const BUILD_MARGIN_ROWS: f32 = 20.0;

/// How far beyond the view, in rows, a built row has to be before it is taken
/// back to a frame. Further than [`BUILD_MARGIN_ROWS`], so a small scroll back
/// and forth does not build and take down the same rows.
const KEEP_MARGIN_ROWS: f32 = 60.0;

/// How far beyond the view, in rows, a frame has to be before it leaves
/// layout. Further than [`KEEP_MARGIN_ROWS`], so a frame is laid out again
/// before it is built.
const PARK_MARGIN_ROWS: f32 = 80.0;

/// How many frames a row to show may take to appear, while its branch opens.
const SHOW_FRAMES: u32 = 16;

/// The scene entity whose row should be scrolled into view once it exists.
#[derive(Resource, Default)]
struct RowToShow {
    source: Option<Entity>,
    frames_left: u32,
}

/// Whether a row is being dragged, so no row is taken down under the pointer.
#[derive(Resource, Default)]
struct RowDragActive(bool);

fn note_drag_start(
    start: On<Pointer<DragStart>>,
    contents: Query<(), With<TreeRowContent>>,
    mut active: ResMut<RowDragActive>,
) {
    if contents.contains(start.event_target()) {
        active.0 = true;
    }
}

fn note_drag_end(
    end: On<Pointer<DragEnd>>,
    contents: Query<(), With<TreeRowContent>>,
    mut active: ResMut<RowDragActive>,
) {
    if contents.contains(end.event_target()) {
        active.0 = false;
    }
}

/// Whether rows in `container` are built only near the view: a list that
/// scrolls. One that grows to fit its rows shows all of them.
pub(crate) fn builds_rows_in_view(world: &World, container: Entity) -> bool {
    world.get::<HierarchyTreeContainer>(container).is_some()
        && world
            .get::<Node>(container)
            .is_some_and(|node| node.overflow.y == OverflowAxis::Scroll)
}

/// Whether `row` shows its label line yet, or is still a frame.
pub(crate) fn is_built(world: &World, row: Entity) -> bool {
    world.get::<Children>(row).is_some_and(|children| {
        children
            .iter()
            .any(|child| world.get::<TreeRowContent>(child).is_some())
    })
}

/// Build what `row` shows from the entity it stands for, as a freshly spawned
/// row would show it.
pub(crate) fn build_row(world: &mut World, row: Entity) {
    if is_built(world, row) {
        return;
    }
    let Some(source) = world.get::<TreeNode>(row).map(|node| node.0) else {
        return;
    };
    if world.get_entity(source).is_err() {
        return;
    }
    let label = crate::hierarchy::row_label(world, source);
    let has_children = crate::hierarchy::has_visible_children(world, source);
    let category = crate::hierarchy::classify_entity(world, source);
    let inherited = crate::hierarchy::is_inherited_descendant(world, source);
    let icon_override = jackdaw_api_internal::entity_icons::registered_icon(world, source);
    let selected = world.get::<Selected>(source).is_some();
    let style = TreeRowStyle {
        icon_font: world
            .resource::<jackdaw_feathers::icons::IconFont>()
            .0
            .clone(),
    };
    world.entity_mut(row).insert(tree_row_parts(
        &label,
        selected,
        category,
        inherited,
        icon_override,
        &style,
    ));
    set_row_expand_toggle(world, row, has_children);
    if selected
        && let Some(content) = world.get::<Children>(row).and_then(|children| {
            children
                .iter()
                .find(|&child| world.get::<TreeRowContent>(child).is_some())
        })
    {
        world.entity_mut(content).insert(TreeRowSelected);
    }
    if world.get::<Visibility>(source) == Some(&Visibility::Hidden) {
        crate::hierarchy::refresh_row_visibility_glyph(world, source, true);
    }
    set_min_height(world, row, Val::Auto);
}

/// Give `row`, not built yet, the room a built row takes.
pub(crate) fn reserve_row_room(world: &mut World, row: Entity) {
    set_min_height(world, row, px(tree_row_height()));
}

fn set_min_height(world: &mut World, row: Entity, min_height: Val) {
    if let Some(mut node) = world.get_mut::<Node>(row)
        && node.min_height != min_height
    {
        node.min_height = min_height;
    }
}

/// Take `row` back to a frame. A row holding rows of its own keeps them.
fn unbuild_row(world: &mut World, row: Entity) {
    let Some(children) = world.get::<Children>(row).map(|children| children.to_vec()) else {
        return;
    };
    let holds_rows = children.iter().any(|&child| {
        world.get::<TreeRowChildren>(child).is_some()
            && world
                .get::<Children>(child)
                .is_some_and(|rows| !rows.is_empty())
    });
    if holds_rows {
        return;
    }
    for child in children {
        if let Ok(part) = world.get_entity_mut(child) {
            part.despawn();
        }
    }
    world
        .entity_mut(row)
        .remove::<crate::hierarchy::RowLockGlyph>();
    reserve_row_room(world, row);
}

/// Whether something is working with `row`, so it has to stay built.
fn is_held(world: &World, row: Entity, focused: Option<Entity>) -> bool {
    if focused == Some(row) {
        return true;
    }
    if world
        .get::<TreeNodeExpanded>(row)
        .is_some_and(|expanded| expanded.0)
    {
        return true;
    }
    world
        .get::<TreeNode>(row)
        .is_some_and(|node| world.get::<Selected>(node.0).is_some())
}

/// The top and bottom of `entity` on screen, in logical pixels, from the last
/// layout. `None` before it has been laid out.
fn span(world: &World, entity: Entity) -> Option<(f32, f32)> {
    let computed = world.get::<ComputedNode>(entity)?;
    if computed.size.y <= 0.0 {
        return None;
    }
    let centre = world.get::<UiGlobalTransform>(entity)?.translation.y;
    let scale = computed.inverse_scale_factor;
    let half = computed.size.y / 2.0;
    Some(((centre - half) * scale, (centre + half) * scale))
}

/// Whether a row could have come into or left the view, or started or stopped
/// being held, since the last pass.
fn rows_may_have_moved(
    moved_rows: Query<(), (With<TreeNode>, Changed<UiGlobalTransform>)>,
    moved_views: Query<
        (),
        (
            With<HierarchyTreeContainer>,
            Or<(Changed<ScrollPosition>, Changed<ComputedNode>)>,
        ),
    >,
    focused: Res<TreeFocused>,
    selection: Res<Selection>,
    drag: Res<RowDragActive>,
) -> bool {
    focused.is_changed()
        || selection.is_changed()
        || drag.is_changed()
        || !moved_views.is_empty()
        || !moved_rows.is_empty()
}

/// What one pass decides for the rows of a list.
#[derive(Default)]
struct RowPass {
    to_build: Vec<Entity>,
    to_unbuild: Vec<Entity>,
    to_park: Vec<Entity>,
    to_unpark: Vec<Entity>,
    /// The margin above each row that stays laid out.
    margins: Vec<(Entity, f32)>,
}

/// The view a pass places rows against, in logical pixels.
struct View {
    top: f32,
    bottom: f32,
    row_height: f32,
    focused: Option<Entity>,
    may_unbuild: bool,
}

impl View {
    fn near(&self, top: f32, bottom: f32, margin_rows: f32) -> bool {
        let margin = margin_rows * self.row_height;
        bottom >= self.top - margin && top <= self.bottom + margin
    }
}

/// The rows of `list`, a panel or a branch's child container, in the order
/// they are drawn.
fn rows_of(world: &World, list: Entity) -> Vec<Entity> {
    world
        .get::<Children>(list)
        .map(|children| {
            children
                .iter()
                .filter(|&child| world.get::<TreeNode>(child).is_some())
                .collect()
        })
        .unwrap_or_default()
}

/// The container under `row` its child rows go in, while the branch is open.
fn open_child_list(world: &World, row: Entity) -> Option<Entity> {
    if !world
        .get::<TreeNodeExpanded>(row)
        .is_some_and(|expanded| expanded.0)
    {
        return None;
    }
    world.get::<Children>(row)?.iter().find(|&child| {
        world.get::<TreeRowChildren>(child).is_some()
            && world
                .get::<Node>(child)
                .is_some_and(|node| node.display != Display::None)
    })
}

/// Decide, for each row of `list` and of the open branches under it, whether
/// it is built and whether it stays laid out, from where it would sit. The
/// first row the list shows always stays laid out, and the rest are placed
/// from it; so does the last, which holds the room of the rows parked above it
/// and so keeps the full length of the list scrollable.
fn plan_list(world: &World, list: Entity, view: &View, pass: &mut RowPass) {
    let rows: Vec<Entity> = rows_of(world, list)
        .into_iter()
        .filter(|&row| !row_is_hidden(world.get::<Node>(row), world.get::<TreeRowParked>(row)))
        .collect();
    let Some(&first) = rows.first() else {
        return;
    };
    if world.get::<TreeRowParked>(first).is_some() {
        pass.to_unpark.push(first);
        return;
    }
    let Some((mut top, _)) = span(world, first) else {
        return;
    };
    let last = rows.len() - 1;
    let mut room_above = 0.0;
    for (position, row) in rows.into_iter().enumerate() {
        let parked = world.get::<TreeRowParked>(row).is_some();
        let height = if parked {
            view.row_height
        } else {
            span(world, row).map_or(view.row_height, |(top, bottom)| bottom - top)
        };
        let bottom = top + height;
        let held = is_held(world, row, view.focused);
        let built = is_built(world, row);
        let keep_margin = if parked {
            KEEP_MARGIN_ROWS
        } else {
            PARK_MARGIN_ROWS
        };
        let laid_out = position == 0
            || position == last
            || held
            || built
            || view.near(top, bottom, keep_margin);
        if laid_out {
            if parked {
                pass.to_unpark.push(row);
            }
            pass.margins.push((row, room_above));
            room_above = 0.0;
            if !built && (held || view.near(top, bottom, BUILD_MARGIN_ROWS)) {
                pass.to_build.push(row);
            } else if built
                && view.may_unbuild
                && !held
                && !view.near(top, bottom, KEEP_MARGIN_ROWS)
            {
                pass.to_unbuild.push(row);
            }
            if built && let Some(children) = open_child_list(world, row) {
                plan_list(world, children, view, pass);
            }
        } else {
            if !parked {
                pass.to_park.push(row);
            }
            room_above += height;
        }
        top = bottom;
    }
}

/// Take `row` out of layout, keeping its `Node` for when it comes back.
fn park_row(world: &mut World, row: Entity) {
    let Ok(mut entity) = world.get_entity_mut(row) else {
        return;
    };
    let Some(mut node) = entity.take::<Node>() else {
        return;
    };
    node.margin.top = Val::ZERO;
    entity.insert(TreeRowParked(node));
}

/// Put a parked `row` back into layout.
fn unpark_row(world: &mut World, row: Entity) {
    let Ok(mut entity) = world.get_entity_mut(row) else {
        return;
    };
    if let Some(TreeRowParked(node)) = entity.take::<TreeRowParked>() {
        entity.insert(node);
    }
}

fn set_room_above(world: &mut World, row: Entity, room: f32) {
    let room = px(room);
    if let Some(mut node) = world.get_mut::<Node>(row)
        && node.margin.top != room
    {
        node.margin.top = room;
    }
}

/// Build the rows near each scrolling Outliner's view, take far ones back to
/// frames, and park the frames further out.
fn build_rows_in_view(world: &mut World) {
    let containers: Vec<Entity> = world
        .query_filtered::<Entity, With<HierarchyTreeContainer>>()
        .iter(world)
        .filter(|&container| builds_rows_in_view(world, container))
        .collect();
    if containers.is_empty() {
        return;
    }
    let focused = world.resource::<TreeFocused>().0;
    let may_unbuild = !world.resource::<RowDragActive>().0
        && world
            .query_filtered::<(), With<crate::hierarchy::InlineRenameInput>>()
            .iter(world)
            .next()
            .is_none();

    for container in containers {
        let Some((top, bottom)) = span(world, container) else {
            continue;
        };
        let view = View {
            top,
            bottom,
            row_height: tree_row_height(),
            focused,
            may_unbuild,
        };
        let mut pass = RowPass::default();
        plan_list(world, container, &view, &mut pass);

        let held_frames: Vec<Entity> = world
            .resource::<TreeIndex>()
            .rows_in(container)
            .map(|(_source, row)| row)
            .filter(|&row| is_held(world, row, focused) && !is_built(world, row))
            .collect();
        for row in held_frames {
            if world.get::<TreeRowParked>(row).is_some() {
                pass.to_unpark.push(row);
            }
            pass.to_build.push(row);
        }

        for row in pass.to_unpark {
            unpark_row(world, row);
        }
        for (row, room) in pass.margins {
            set_room_above(world, row, room);
        }
        for row in pass.to_park {
            park_row(world, row);
        }
        for row in pass.to_build {
            build_row(world, row);
        }
        for row in pass.to_unbuild {
            unbuild_row(world, row);
        }
    }
}

/// A new focus or a new primary selection is a row to scroll into view.
fn follow_focus_and_selection(
    focused: Res<TreeFocused>,
    selection: Res<Selection>,
    rows: Query<&TreeNode>,
    mut show: ResMut<RowToShow>,
) {
    let source = if focused.is_changed() {
        focused
            .0
            .and_then(|row| rows.get(row).ok())
            .map(|node| node.0)
    } else if selection.is_changed() {
        selection.primary()
    } else {
        return;
    };
    if source.is_some() {
        show.source = source;
        show.frames_left = SHOW_FRAMES;
    }
}

/// Scroll each scrolling Outliner so the row to show sits inside its view,
/// once the row has been laid out.
fn show_the_row(world: &mut World) {
    let Some(source) = world.resource::<RowToShow>().source else {
        return;
    };
    let rows: Vec<(Entity, Entity)> = world
        .resource::<TreeIndex>()
        .rows_for_source(source)
        .collect();
    let mut shown = false;
    for (container, row) in rows {
        if !builds_rows_in_view(world, container) {
            shown = true;
            continue;
        }
        let (Some((view_top, view_bottom)), Some((top, _))) =
            (span(world, container), span(world, row))
        else {
            continue;
        };
        let bottom = top + tree_row_height();
        let delta = if top < view_top {
            top - view_top
        } else if bottom > view_bottom {
            bottom - view_bottom
        } else {
            0.0
        };
        if delta != 0.0
            && let Some(mut scroll) = world.get_mut::<ScrollPosition>(container)
        {
            scroll.y = (scroll.y + delta).max(0.0);
        }
        shown = true;
    }
    let mut show = world.resource_mut::<RowToShow>();
    if shown || show.frames_left == 0 {
        show.source = None;
    } else {
        show.frames_left -= 1;
    }
}
