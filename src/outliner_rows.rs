//! Rows built only where they can be seen.
//!
//! A scrolling Outliner keeps a row out of view as a frame one row high, and
//! builds what the row shows once it scrolls near the view. A scene of
//! thousands of top-level nodes then costs a frame per node rather than a
//! label, toggles and drop gaps per node.
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
    TreeRowSelected,
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

/// Build the rows near each scrolling Outliner's view, and take far ones back
/// to frames.
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
    let row_height = tree_row_height();
    let may_unbuild = !world.resource::<RowDragActive>().0
        && world
            .query_filtered::<(), With<crate::hierarchy::InlineRenameInput>>()
            .iter(world)
            .next()
            .is_none();

    for container in containers {
        let Some((view_top, view_bottom)) = span(world, container) else {
            continue;
        };
        let rows: Vec<Entity> = world
            .resource::<TreeIndex>()
            .rows_in(container)
            .map(|(_source, row)| row)
            .collect();
        let mut to_build = Vec::new();
        let mut to_unbuild = Vec::new();
        for row in rows {
            let held = is_held(world, row, focused);
            let built = is_built(world, row);
            let Some((top, bottom)) = span(world, row) else {
                if held && !built {
                    to_build.push(row);
                }
                continue;
            };
            let near = |margin: f32| {
                bottom >= view_top - margin * row_height && top <= view_bottom + margin * row_height
            };
            if !built && (held || near(BUILD_MARGIN_ROWS)) {
                to_build.push(row);
            } else if built && may_unbuild && !held && !near(KEEP_MARGIN_ROWS) {
                to_unbuild.push(row);
            }
        }
        for row in to_build {
            build_row(world, row);
        }
        for row in to_unbuild {
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
