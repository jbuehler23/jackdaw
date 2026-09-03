use bevy::{
    feathers::{controls::FeathersDisclosureToggle, theme::ThemedText},
    prelude::*,
    text::{LineBreak, TextLayoutInfo},
    ui::Checked,
    ui_widgets::{ValueChange, observe},
};
use bevy_monitors::prelude::{MonitorSelf, Mutation, NotifyChanged};
use jackdaw_widgets::tree_view::{
    EntityCategory, TreeChildrenPopulated, TreeDragCancelled, TreeDropLine, TreeFocused, TreeNode,
    TreeNodeExpandToggle, TreeNodeExpanded, TreeRoot, TreeRowChildren, TreeRowClicked,
    TreeRowContent, TreeRowDot, TreeRowDropped, TreeRowDroppedOnRoot, TreeRowInlineRename,
    TreeRowInsertZone, TreeRowInserted, TreeRowLabel, TreeRowLockToggle, TreeRowLockToggled,
    TreeRowSelected,
    TreeRowStartRename, TreeRowVisibilityToggle, TreeRowVisibilityToggled, TreeSpringLoad,
    TreeView,
};

use lucide_icons::Icon;

use crate::tokens;

pub const ROW_BG: Color = Color::NONE;
const INDENT_WIDTH: f32 = 16.0;
const TOGGLE_WIDTH: f32 = 18.0;
const DOT_COLUMN_WIDTH: f32 = 14.0;
/// How tall a tree row is: the text plus the padding and border around
/// it. Not measured, because the gap zones are laid out before anything
/// has been; it is what the row's own values add up to.
const ROW_HEIGHT: f32 = tokens::TEXT_SIZE_PX + 2.0 * tokens::SPACING_XS + 2.0;

/// How tall the strip standing for the gap between two rows is.
///
/// A third of a row each side of the boundary, so the band that means
/// "between these two" is two thirds of a row wide. Four pixels a side
/// with a two-pixel overhang, which is what this was, left a dead band
/// in the middle of the row and a gap that had to be hit rather than
/// aimed at.
const INSERT_ZONE_HEIGHT: f32 = ROW_HEIGHT / 3.0;

/// How long the pointer rests on a collapsed row before the drag opens
/// it. Long enough that crossing a parent on the way somewhere else
/// opens nothing, short enough that resting on one answers promptly.
const SPRING_LOAD_DELAY: f32 = 0.4;

/// How close to the top or bottom edge of the list the pointer has to be
/// for a drag to scroll it, and how fast it scrolls there.
const AUTO_SCROLL_MARGIN: f32 = 24.0;
const AUTO_SCROLL_SPEED: f32 = 320.0;

/// Parameters for tree row icon font rendering
#[derive(Clone)]
pub struct TreeRowStyle {
    pub icon_font: Handle<Font>,
}

/// Returns the display color for an entity category. When `inherited`
/// is true, the muted `CATEGORY_INHERITED` color wins; this is how
/// resolver-materialised descendants of a prefab instance get drawn
/// faintly regardless of their underlying category (Mesh, Light, etc.).
pub fn category_color(category: EntityCategory, inherited: bool) -> Color {
    if inherited {
        return tokens::CATEGORY_INHERITED;
    }
    match category {
        EntityCategory::Camera => tokens::CATEGORY_CAMERA,
        EntityCategory::Light => tokens::CATEGORY_LIGHT,
        EntityCategory::Mesh => tokens::CATEGORY_MESH,
        EntityCategory::Scene => tokens::CATEGORY_SCENE,
        EntityCategory::Prefab => tokens::CATEGORY_PREFAB,
        EntityCategory::Inherited => tokens::CATEGORY_INHERITED,
        EntityCategory::AssetPart => tokens::CATEGORY_ASSET_PART,
        EntityCategory::Group | EntityCategory::Entity => tokens::CATEGORY_ENTITY,
    }
}

/// Creates a tree row bundle for displaying an entity in the hierarchy.
/// `inherited` flips the dot color to the muted "inherited from prefab"
/// tone while leaving the icon character to reflect the underlying
/// category (Lightbulb for an inherited light, Video for an inherited
/// camera, etc.).
///
/// The row spawns with an empty expand toggle; call
/// [`set_row_expand_toggle`] to give it a disclosure control once the
/// source entity's children are known.
pub fn tree_row(
    label: &str,
    selected: bool,
    source: Entity,
    category: EntityCategory,
    inherited: bool,
    icon_override: Option<Icon>,
    style: &TreeRowStyle,
) -> impl Bundle {
    (
        TreeNode(source),
        TreeNodeExpanded(false),
        TreeChildrenPopulated(false),
        MonitorSelf,
        NotifyChanged::<TreeNodeExpanded>::default(),
        Node {
            flex_direction: FlexDirection::Column,
            width: percent(100),
            ..default()
        },
        children![
            // The clickable row content
            tree_row_content(
                label,
                selected,
                source,
                category,
                inherited,
                icon_override,
                style
            ),
            // Container for child rows (initially empty, populated lazily)
            (
                TreeRowChildren,
                Node {
                    flex_direction: FlexDirection::Column,
                    // Indent via padding only. A left margin on a full-width box
                    // is clamped to `parent_width - margin`, so each nesting
                    // level would pull the row's right edge (and the eye toggle
                    // anchored there) inward. Padding and a left border shift the
                    // content rightward without moving the right edge.
                    padding: UiRect::left(px(INDENT_WIDTH + tokens::SPACING_SM)),
                    border: UiRect::left(px(1.0)),
                    width: percent(100),
                    display: Display::None,
                    ..default()
                },
                BorderColor::all(tokens::CONNECTION_LINE),
            ),
            // The gaps above and below the row. Last, so they sit over the
            // row's own drop target where the two meet.
            insertion_zone(false),
            insertion_zone(true)
        ],
        // React to TreeNodeExpanded changes: children visibility + Checked
        observe(
            |mutation: On<Mutation<TreeNodeExpanded>>,
             expanded_query: Query<(&TreeNodeExpanded, &Children)>,
             children_container: Query<Entity, With<TreeRowChildren>>,
             content_query: Query<&Children, With<TreeRowContent>>,
             toggle_query: Query<&Children, With<TreeNodeExpandToggle>>,
             mut node_query: Query<&mut Node>,
             mut commands: Commands| {
                let entity = mutation.event_target();
                let Ok((expanded, children)) = expanded_query.get(entity) else {
                    return;
                };

                for child in children.iter() {
                    if children_container.contains(child)
                        && let Ok(mut node) = node_query.get_mut(child)
                    {
                        node.display = if expanded.0 {
                            Display::Flex
                        } else {
                            Display::None
                        };
                    }

                    // TreeRowContent -> TreeNodeExpandToggle -> disclosure.
                    // A childless leaf carries no disclosure at all, so it
                    // keeps its blank toggle even when the reveal machinery
                    // marks it expanded.
                    let Ok(content_children) = content_query.get(child) else {
                        continue;
                    };
                    for cc in content_children.iter() {
                        let Ok(toggle_children) = toggle_query.get(cc) else {
                            continue;
                        };
                        for disclosure in toggle_children.iter() {
                            let mut ec = commands.entity(disclosure);
                            if expanded.0 {
                                ec.insert(Checked);
                            } else {
                                ec.remove::<Checked>();
                            }
                        }
                    }
                }
            },
        ),
    )
}

/// The strip standing for the gap above (`after == false`) or below
/// (`after == true`) a row. Dropping there reorders the dragged entity
/// among the row's siblings instead of making it the row's child.
///
/// Placed absolutely over the top or bottom third of the row, so the two
/// strips either side of a boundary meet with nothing between them: the
/// gap is a band to aim at rather than a line to hit. The lower strip
/// sits under the row's whole subtree, which is where "after this row"
/// is once the row is expanded.
fn insertion_zone(after: bool) -> impl Bundle {
    (
        TreeRowInsertZone { after },
        Node {
            position_type: PositionType::Absolute,
            left: px(0.0),
            right: px(0.0),
            top: if after { Val::Auto } else { px(0.0) },
            bottom: if after { px(0.0) } else { Val::Auto },
            height: px(INSERT_ZONE_HEIGHT),
            ..default()
        },
        BackgroundColor(Color::NONE),
        // A strip covers the top and the bottom third of the row, so two
        // thirds of every row is a gap. A click there is a click on the
        // row: without this it would bubble past the row (which has no
        // click observer of its own) to the container and select nothing,
        // and only the middle third of the outliner would answer a
        // pointer.
        observe(
            |mut click: On<Pointer<Click>>,
             mut commands: Commands,
             parents: Query<&ChildOf>,
             children: Query<&Children>,
             tree_nodes: Query<&TreeNode>,
             contents: Query<(), With<TreeRowContent>>| {
                if click.event.button != PointerButton::Primary {
                    return;
                }
                click.propagate(false);
                let Ok(&ChildOf(row)) = parents.get(click.event_target()) else {
                    return;
                };
                let Ok(node) = tree_nodes.get(row) else {
                    return;
                };
                let Some(content) = children
                    .get(row)
                    .ok()
                    .and_then(|children| children.iter().find(|child| contents.contains(*child)))
                else {
                    return;
                };
                commands.trigger(TreeRowClicked {
                    entity: content,
                    source_entity: node.0,
                });
            },
        ),
        // Every move, not only the first: the line has to follow the
        // pointer along the gap, and the level a release would land at
        // changes with the pointer's x without it leaving the zone.
        observe(
            |mut over: On<Pointer<DragOver>>,
             zones: Query<&TreeRowInsertZone>,
             parents: Query<&ChildOf>,
             children: Query<&Children>,
             tree_nodes: Query<&TreeNode>,
             expanded: Query<&TreeNodeExpanded>,
             row_children: Query<(), With<TreeRowChildren>>,
             transforms: Query<(&ComputedNode, &UiGlobalTransform)>,
             mut line: ResMut<TreeDropLine>| {
                over.propagate(false);
                let zone = over.event_target();
                let cursor = over.pointer_location.position;
                let depth = resolve_drop_depth(
                    zone,
                    cursor,
                    &zones,
                    &parents,
                    &children,
                    &tree_nodes,
                    &expanded,
                    &row_children,
                    &transforms,
                )
                .map_or(0, |(_, depth)| depth);
                line.zone = Some(zone);
                line.indent = depth as f32 * (INDENT_WIDTH + tokens::SPACING_SM);
            },
        ),
        observe(
            |mut leave: On<Pointer<DragLeave>>, mut line: ResMut<TreeDropLine>| {
                leave.propagate(false);
                if line.zone == Some(leave.event_target()) {
                    line.zone = None;
                }
            },
        ),
        observe(
            |mut drop: On<Pointer<DragDrop>>,
             mut commands: Commands,
             parents: Query<&ChildOf>,
             children: Query<&Children>,
             tree_nodes: Query<&TreeNode>,
             expanded: Query<&TreeNodeExpanded>,
             zones: Query<&TreeRowInsertZone>,
             row_children: Query<(), With<TreeRowChildren>>,
             transforms: Query<(&ComputedNode, &UiGlobalTransform)>,
             mut line: ResMut<TreeDropLine>| {
                drop.propagate(false);
                let zone = drop.event_target();
                line.zone = None;
                let Ok(side) = zones.get(zone) else {
                    return;
                };
                let cursor = drop.pointer_location.position;
                let Some((row, _)) = resolve_drop_depth(
                    zone,
                    cursor,
                    &zones,
                    &parents,
                    &children,
                    &tree_nodes,
                    &expanded,
                    &row_children,
                    &transforms,
                ) else {
                    return;
                };
                let Ok(target) = tree_nodes.get(row) else {
                    return;
                };
                let Some(dragged_source) = find_source_entity(drop.dropped, &parents, &tree_nodes)
                else {
                    return;
                };
                commands.trigger(TreeRowInserted {
                    entity: zone,
                    dragged_source,
                    target: target.0,
                    index: usize::from(side.after),
                });
            },
        ),
    )
}

/// Which row's gap the pointer is in, and at what depth.
///
/// An expanded row's after-gap is drawn on the same pixels as its last
/// descendant's, because the descendant is the last thing under it. So one
/// strip stands for several places a drop could land, and which zone
/// entity the picking backend hands over says nothing about which of them
/// the user meant: the zones are spawned after the rows, so the shallowest
/// one wins the pick every time. The candidates are therefore collected
/// from the row tree ([`coincident_after_gaps`]), and the pointer's x
/// picks between them against the indent each level is actually drawn at,
/// the way it does in a file tree.
///
/// Returns `(row, depth)`. `None` for a zone that is not under a row.
fn resolve_drop_depth(
    zone: Entity,
    cursor: Vec2,
    zones: &Query<&TreeRowInsertZone>,
    parents: &Query<&ChildOf>,
    children: &Query<&Children>,
    tree_nodes: &Query<&TreeNode>,
    expanded: &Query<&TreeNodeExpanded>,
    row_children: &Query<(), With<TreeRowChildren>>,
    transforms: &Query<(&ComputedNode, &UiGlobalTransform)>,
) -> Option<(Entity, usize)> {
    let Ok(&ChildOf(row)) = parents.get(zone) else {
        return None;
    };
    tree_nodes.get(row).ok()?;
    let depth = row_depth(row, parents, row_children);
    // Only the gap below a row can coincide with anything: the gap above
    // one is its own, whatever is nested above it.
    if !zones.get(zone).is_ok_and(|side| side.after) {
        return Some((row, depth));
    }

    let candidates = coincident_after_gaps(
        row,
        depth,
        parents,
        children,
        tree_nodes,
        expanded,
        row_children,
    );
    Some(level_at_cursor(&candidates, cursor.x, transforms))
}

/// Every row whose after-gap is drawn on the same pixels as `row`'s,
/// shallowest first, `row` included.
///
/// Downwards: an expanded row's gap sits below its last descendant, so
/// that descendant's own gap is the same strip. Upwards: when `row` is the
/// last thing under its parent, the parent's gap is that strip too.
fn coincident_after_gaps(
    row: Entity,
    depth: usize,
    parents: &Query<&ChildOf>,
    children: &Query<&Children>,
    tree_nodes: &Query<&TreeNode>,
    expanded: &Query<&TreeNodeExpanded>,
    row_children: &Query<(), With<TreeRowChildren>>,
) -> Vec<(Entity, usize)> {
    let mut above = Vec::new();
    let mut current = row;
    let mut current_depth = depth;
    // A tree is not deep; the bounds here guard against a cycle rather
    // than limiting anything anyone builds.
    for _ in 0..64 {
        let Some(parent_row) = enclosing_row_if_last(current, parents, children, row_children)
        else {
            break;
        };
        if tree_nodes.get(parent_row).is_err() || current_depth == 0 {
            break;
        }
        current_depth -= 1;
        above.push((parent_row, current_depth));
        current = parent_row;
    }
    above.reverse();

    let mut candidates = above;
    candidates.push((row, depth));

    let mut current = row;
    let mut current_depth = depth;
    for _ in 0..64 {
        let Some(last) =
            last_visible_child_row(current, children, tree_nodes, expanded, row_children)
        else {
            break;
        };
        current_depth += 1;
        candidates.push((last, current_depth));
        current = last;
    }
    candidates
}

/// The last row drawn under `row`, when `row` is expanded and holds any.
fn last_visible_child_row(
    row: Entity,
    children: &Query<&Children>,
    tree_nodes: &Query<&TreeNode>,
    expanded: &Query<&TreeNodeExpanded>,
    row_children: &Query<(), With<TreeRowChildren>>,
) -> Option<Entity> {
    if !expanded.get(row).is_ok_and(|expanded| expanded.0) {
        return None;
    }
    let container = children
        .get(row)
        .ok()?
        .iter()
        .find(|&child| row_children.get(child).is_ok())?;
    children
        .get(container)
        .ok()?
        .iter()
        .rev()
        .find(|&child| tree_nodes.get(child).is_ok())
}

/// The candidate the pointer is pointing at: the deepest one whose own
/// indent it has reached, and the shallowest when it has reached none.
///
/// Both sides of the comparison are logical pixels. `UiGlobalTransform`
/// and `ComputedNode` are physical, and `Pointer::pointer_location` is
/// logical, so at any scale factor but 1 an unconverted comparison pins
/// every gap to the shallowest level.
fn level_at_cursor(
    candidates: &[(Entity, usize)],
    cursor_x: f32,
    transforms: &Query<(&ComputedNode, &UiGlobalTransform)>,
) -> (Entity, usize) {
    let mut chosen = candidates[0];
    for &(row, depth) in candidates {
        let Ok((computed, transform)) = transforms.get(row) else {
            continue;
        };
        let scale = computed.inverse_scale_factor();
        let left = (transform.translation.x - computed.size().x / 2.0) * scale;
        if cursor_x >= left {
            chosen = (row, depth);
        }
    }
    chosen
}

/// How many levels deep `row` sits, counting the child containers crossed
/// on the way to the tree's root.
fn row_depth(
    row: Entity,
    parents: &Query<&ChildOf>,
    row_children: &Query<(), With<TreeRowChildren>>,
) -> usize {
    let mut depth = 0;
    let mut current = row;
    // A tree is not deep; the bound is a guard against a cycle, not a
    // limit anyone reaches.
    for _ in 0..64 {
        let Ok(&ChildOf(parent)) = parents.get(current) else {
            return depth;
        };
        if row_children.get(parent).is_ok() {
            depth += 1;
        }
        current = parent;
    }
    depth
}

/// The row `row` is nested in, when `row` is the last thing under it, so
/// the two share an after-gap. `None` otherwise.
fn enclosing_row_if_last(
    row: Entity,
    parents: &Query<&ChildOf>,
    children: &Query<&Children>,
    row_children: &Query<(), With<TreeRowChildren>>,
) -> Option<Entity> {
    let &ChildOf(container) = parents.get(row).ok()?;
    row_children.get(container).ok()?;
    let last = children.get(container).ok()?.iter().next_back()?;
    if last != row {
        return None;
    }
    let &ChildOf(parent_row) = parents.get(container).ok()?;
    Some(parent_row)
}

/// The row's expand toggle container, reached as
/// row -> [`TreeRowContent`] -> [`TreeNodeExpandToggle`].
fn expand_toggle_of(world: &World, row: Entity) -> Option<Entity> {
    let content = world
        .get::<Children>(row)?
        .iter()
        .find(|&child| world.get::<TreeRowContent>(child).is_some())?;
    world
        .get::<Children>(content)?
        .iter()
        .find(|&child| world.get::<TreeNodeExpandToggle>(child).is_some())
}

/// Give `row` a [`FeathersDisclosureToggle`] when `has_children`, and take it
/// away when not, so a row that gains its first child becomes expandable and
/// one that loses its last stops advertising children. [`Checked`] mirrors the
/// row's [`TreeNodeExpanded`], which is what points the chevron down.
pub fn set_row_expand_toggle(world: &mut World, row: Entity, has_children: bool) {
    let Some(toggle) = expand_toggle_of(world, row) else {
        return;
    };
    let existing: Vec<Entity> = world
        .get::<Children>(toggle)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    if !has_children {
        for child in existing {
            if let Ok(entity) = world.get_entity_mut(child) {
                entity.despawn();
            }
        }
        return;
    }
    let expanded = world
        .get::<TreeNodeExpanded>(row)
        .map(|expanded| expanded.0)
        .unwrap_or(false);
    if let Some(&disclosure) = existing.first() {
        set_checked(world, disclosure, expanded);
        return;
    }
    let Ok(mut disclosure) = world.spawn_scene(bsn! { @FeathersDisclosureToggle }) else {
        return;
    };
    disclosure.insert(ChildOf(toggle));
    if expanded {
        disclosure.insert(Checked);
    }
    disclosure.observe(
        |change: On<ValueChange<bool>>,
         mut commands: Commands,
         parent_query: Query<&ChildOf>,
         tree_node_query: Query<(), With<TreeNodeExpanded>>| {
            let mut current = change.event_target();
            for _ in 0..4 {
                if tree_node_query.contains(current) {
                    commands
                        .entity(current)
                        .insert(TreeNodeExpanded(change.event().value));
                    return;
                }
                let Ok(&ChildOf(parent)) = parent_query.get(current) else {
                    return;
                };
                current = parent;
            }
        },
    );
}

fn set_checked(world: &mut World, entity: Entity, checked: bool) {
    let Ok(mut entity) = world.get_entity_mut(entity) else {
        return;
    };
    if checked {
        entity.insert(Checked);
    } else {
        entity.remove::<Checked>();
    }
}

fn tree_row_content(
    label: &str,
    selected: bool,
    source: Entity,
    category: EntityCategory,
    inherited: bool,
    icon_override: Option<Icon>,
    style: &TreeRowStyle,
) -> impl Bundle {
    let bg = if selected {
        tokens::SELECTED_BG
    } else {
        ROW_BG
    };
    let border = if selected {
        tokens::SELECTED_BORDER
    } else {
        Color::NONE
    };

    (
        TreeRowContent,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding: UiRect::axes(px(tokens::SPACING_SM), px(tokens::SPACING_XS)),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(6.0)),
            width: percent(100),
            ..default()
        },
        BackgroundColor(bg),
        BorderColor::all(border),
        children![
            // Expand toggle (the disclosure lands via `set_row_expand_toggle`)
            expand_toggle(),
            // Category icon
            category_dot(category, inherited, icon_override, &style.icon_font),
            // Label
            (
                TreeRowLabel,
                TreeRowLabelEllipsis,
                Text::new(label),
                TextFont {
                    font_size: tokens::TEXT_SIZE,
                    ..default()
                },
                // One line, and no wider than the room the row has left:
                // without the zero minimum a long name is the row's
                // min-content width, which pushes the lock and the eye out
                // of the panel and squeezes the rename entry to nothing.
                // `ellipsize_tree_row_labels` cuts what is left over.
                TextLayout {
                    linebreak: LineBreak::NoWrap,
                    ..default()
                },
                Node {
                    flex_grow: 1.0,
                    // A floor rather than nothing: with a zero minimum the
                    // label is the first thing a narrow panel takes the room
                    // from, and a row that shows three letters of a name is
                    // no better than one that shows none. This is the width
                    // the name, and the rename entry that replaces it, are
                    // always given.
                    min_width: px(LABEL_MIN_WIDTH),
                    overflow: Overflow::clip(),
                    margin: UiRect::left(px(tokens::SPACING_SM)),
                    ..default()
                },
                // The tooltip the cut name is read in full from needs the
                // hover state, and picking only updates a `Hovered` that is
                // already on the entity.
                bevy::picking::hover::Hovered::default(),
                ThemedText,
            ),
            // Lock toggle, then the visibility toggle (eye icon)
            lock_toggle(source, &style.icon_font),
            visibility_toggle(source, &style.icon_font)
        ],
        // Click handler for selection (left-click only)
        observe(
            move |mut click: On<Pointer<Click>>, mut commands: Commands| {
                if click.event.button != PointerButton::Primary {
                    return;
                }
                click.propagate(false);
                commands.trigger(TreeRowClicked {
                    entity: click.event_target(),
                    source_entity: source,
                });
            },
        ),
        // Hover effects (skip selected rows)
        observe(
            |hover: On<Pointer<Over>>,
             mut bg_query: Query<
                &mut BackgroundColor,
                (With<TreeRowContent>, Without<TreeRowSelected>),
            >| {
                if let Ok(mut bg) = bg_query.get_mut(hover.event_target()) {
                    bg.0 = tokens::HOVER_BG;
                }
            },
        ),
        observe(
            |out: On<Pointer<Out>>,
             mut bg_query: Query<
                &mut BackgroundColor,
                (With<TreeRowContent>, Without<TreeRowSelected>),
            >| {
                if let Ok(mut bg) = bg_query.get_mut(out.event_target()) {
                    bg.0 = ROW_BG;
                }
            },
        ),
        // Drag-and-drop: highlight drop target with border accent, and
        // start the clock that opens a closed row rested on.
        observe(
            |mut drag_enter: On<Pointer<DragEnter>>,
             mut query: Query<(&mut BackgroundColor, &mut Node), With<TreeRowContent>>,
             parents: Query<&ChildOf>,
             mut spring: ResMut<TreeSpringLoad>,
             mut commands: Commands| {
                drag_enter.propagate(false);
                let content = drag_enter.event_target();
                if let Ok((mut bg, mut node)) = query.get_mut(content) {
                    bg.0 = tokens::DROP_TARGET_BG;
                    node.border = UiRect::left(px(3.0));
                    commands.entity(content).insert(TreeDropPainted);
                }
                if let Ok(&ChildOf(row)) = parents.get(content) {
                    spring.row = Some(row);
                    spring.waited = 0.0;
                }
            },
        ),
        observe(
            |mut drag_leave: On<Pointer<DragLeave>>,
             mut query: Query<(&mut BackgroundColor, &mut Node), With<TreeRowContent>>,
             selected: Query<(), With<TreeRowSelected>>,
             parents: Query<&ChildOf>,
             mut spring: ResMut<TreeSpringLoad>,
             mut commands: Commands| {
                drag_leave.propagate(false);
                if let Ok((mut bg, mut node)) = query.get_mut(drag_leave.event_target()) {
                    bg.0 = if selected.contains(drag_leave.event_target()) {
                        tokens::SELECTED_BG
                    } else {
                        ROW_BG
                    };
                    node.border = UiRect::all(px(1.0));
                    commands
                        .entity(drag_leave.event_target())
                        .remove::<TreeDropPainted>();
                }
                if let Ok(&ChildOf(row)) = parents.get(drag_leave.event_target())
                    && spring.row == Some(row)
                {
                    spring.row = None;
                }
            },
        ),
        // Drag-and-drop: resolve source entities and fire TreeRowDropped
        observe(
            |mut drag_drop: On<Pointer<DragDrop>>,
             mut commands: Commands,
             parent_query: Query<&ChildOf>,
             tree_nodes: Query<&TreeNode>,
             mut query: Query<(&mut BackgroundColor, &mut Node), With<TreeRowContent>>,
             selected_query: Query<(), With<TreeRowSelected>>,
             mut cancelled: ResMut<TreeDragCancelled>| {
                drag_drop.propagate(false);
                let target_content = drag_drop.event_target();

                // Revert drop target styling
                if let Ok((mut bg, mut node)) = query.get_mut(target_content) {
                    bg.0 = if selected_query.contains(target_content) {
                        tokens::SELECTED_BG
                    } else {
                        ROW_BG
                    };
                    node.border = UiRect::all(px(1.0));
                    commands.entity(target_content).remove::<TreeDropPainted>();
                }
                // The drag was called off; the release is only the pointer
                // catching up with a gesture that is already over.
                if std::mem::take(&mut cancelled.0) {
                    return;
                }

                // Resolve both target and dragged to their scene source entities
                let Ok(&ChildOf(target_tree_row)) = parent_query.get(target_content) else {
                    return;
                };
                let Ok(target_node) = tree_nodes.get(target_tree_row) else {
                    return;
                };
                let Some(dragged_source) =
                    find_source_entity(drag_drop.dropped, &parent_query, &tree_nodes)
                else {
                    return;
                };

                commands.trigger(TreeRowDropped {
                    entity: target_content,
                    dragged_source,
                    target_source: target_node.0,
                });
            },
        ),
    )
}

fn expand_toggle() -> impl Bundle {
    (
        TreeNodeExpandToggle,
        Node {
            width: px(TOGGLE_WIDTH),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        observe(
            |mut click: On<Pointer<Click>>,
             mut commands: Commands,
             parent_query: Query<&ChildOf>,
             tree_node_query: Query<(Entity, &TreeNodeExpanded)>| {
                if click.event.button != PointerButton::Primary {
                    return;
                }
                click.propagate(false);
                // Walk up ChildOf chain to find the nearest TreeNode ancestor
                let mut current = click.event_target();
                for _ in 0..4 {
                    if let Ok((entity, expanded)) = tree_node_query.get(current) {
                        commands
                            .entity(entity)
                            .insert(TreeNodeExpanded(!expanded.0));
                        return;
                    }
                    let Ok(&ChildOf(parent)) = parent_query.get(current) else {
                        return;
                    };
                    current = parent;
                }
            },
        ),
    )
}

/// The row's lock control: a tool button whose glyph says whether the
/// canvas will pick this node up.
///
/// A feathers tool button rather than a bare glyph, because it is a
/// control: it takes the focus ring, the hover and pressed treatments and
/// the disabled state from the same place every other button in the editor
/// does. The consumer refreshes the glyph by writing the button's own
/// caption text; see the editor's `refresh_row_lock_glyph`.
fn lock_toggle(source: Entity, icon_font: &Handle<Font>) -> impl Bundle + use<> {
    (
        TreeRowLockToggle,
        crate::button::icon_button(
            crate::button::IconButtonProps::new(Icon::LockOpen).with_alpha(LOCK_IDLE_ALPHA),
            icon_font,
        ),
        observe(
            move |click: On<crate::button::ButtonClickEvent>, mut commands: Commands| {
                commands.trigger(TreeRowLockToggled {
                    entity: click.entity,
                    source_entity: source,
                });
            },
        ),
    )
}

/// How faint an unlocked row's padlock is. The control is on every row and
/// means nothing on most of them, so it stays out of the way until it is
/// either hovered or set.
pub const LOCK_IDLE_ALPHA: f32 = 0.4;

/// Eye icon for toggling entity visibility.
fn visibility_toggle(source: Entity, icon_font: &Handle<Font>) -> impl Bundle {
    (
        TreeRowVisibilityToggle,
        Node {
            width: px(18.0),
            height: px(18.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        children![(
            Text::new(String::from(Icon::Eye.unicode())),
            TextFont {
                font: icon_font.clone().into(),
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY.with_alpha(0.4)),
        )],
        observe(
            move |mut click: On<Pointer<Click>>, mut commands: Commands| {
                if click.event.button != PointerButton::Primary {
                    return;
                }
                click.propagate(false);
                commands.trigger(TreeRowVisibilityToggled {
                    entity: click.event_target(),
                    source_entity: source,
                });
            },
        ),
        observe(
            |hover: On<Pointer<Over>>,
             children_query: Query<&Children>,
             mut text_color: Query<&mut TextColor>| {
                let entity = hover.event_target();
                if let Ok(children) = children_query.get(entity) {
                    for child in children.iter() {
                        if let Ok(mut color) = text_color.get_mut(child) {
                            color.0 = tokens::TEXT_SECONDARY;
                        }
                    }
                }
            },
        ),
        observe(
            |out: On<Pointer<Out>>,
             children_query: Query<&Children>,
             mut text_color: Query<&mut TextColor>| {
                let entity = out.event_target();
                if let Ok(children) = children_query.get(entity) {
                    for child in children.iter() {
                        if let Ok(mut color) = text_color.get_mut(child) {
                            color.0 = tokens::TEXT_SECONDARY.with_alpha(0.4);
                        }
                    }
                }
            },
        ),
    )
}

/// Icon indicating entity category. When `inherited` is true the row is
/// drawn in the muted prefab-inherited color, but the icon still reflects
/// the underlying category so an inherited light still looks like a light,
/// an inherited camera still looks like a camera. `icon_override`, when
/// present, replaces the category glyph while keeping the category color.
fn category_dot(
    category: EntityCategory,
    inherited: bool,
    icon_override: Option<Icon>,
    icon_font: &Handle<Font>,
) -> impl Bundle {
    let color = category_color(category, inherited);
    let icon_char = icon_override.unwrap_or(match category {
        EntityCategory::Camera => Icon::Video,
        EntityCategory::Light => Icon::Lightbulb,
        EntityCategory::Prefab => Icon::Package,
        EntityCategory::Scene => Icon::Clapperboard,
        EntityCategory::Inherited | EntityCategory::Mesh => Icon::Box,
        EntityCategory::AssetPart => Icon::Component,
        EntityCategory::Group => Icon::Folder,
        EntityCategory::Entity => Icon::Dot,
    });
    (
        TreeRowDot,
        Node {
            width: px(DOT_COLUMN_WIDTH),
            height: px(DOT_COLUMN_WIDTH),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        children![(
            Text::new(String::from(icon_char.unicode())),
            TextFont {
                font: icon_font.clone().into(),
                font_size: tokens::TEXT_SIZE,
                ..default()
            },
            TextColor(color),
        )],
    )
}

/// Walk up the [`ChildOf`] chain from any deeply-nested UI entity until we find
/// a [`TreeNode`] ancestor, then return its source entity.
fn find_source_entity(
    entity: Entity,
    parents: &Query<&ChildOf>,
    tree_nodes: &Query<&TreeNode>,
) -> Option<Entity> {
    let mut current = entity;
    for _ in 0..8 {
        if let Ok(node) = tree_nodes.get(current) {
            return Some(node.0);
        }
        let Ok(&ChildOf(parent)) = parents.get(current) else {
            break;
        };
        current = parent;
    }
    None
}

/// The line drawn where a release would put what is being dragged.
///
/// One per tree, reconciled from [`TreeDropLine`] rather than spawned by
/// whichever zone was entered, so it follows the pointer along a gap and
/// between gaps instead of appearing once and staying put.
#[derive(Component)]
pub struct TreeDropIndicator;

/// What a row's label really says, beside what it has room to show.
///
/// A name too long for the panel is cut down to fit, so the full one has
/// to be kept somewhere: this is where, and it is also what the tooltip
/// reads. `shown` is the last text this crate wrote, which is how a name
/// changed from outside is told apart from the cut this made.
#[derive(Component, Debug, Clone, Default)]
pub struct TreeRowLabelFit {
    /// The name in full.
    pub full: String,
    /// The text last written into the label.
    pub shown: String,
    /// How wide the whole name was drawn, in logical pixels, from the
    /// last frame it was the text on screen. Zero until it has been.
    ///
    /// Kept because a cut label measures the cut text: without the width
    /// of the whole name a row wide enough to hold it again could only be
    /// found by writing it back, and a name that does not fit would be
    /// cut on the next frame and written back on the one after.
    pub full_width: f32,
}

/// Marks a row label laid out to fill the row, and so one that may be
/// cut down to what the row has room for.
///
/// [`TreeRowLabel`] is shared with trees whose labels are laid out to
/// their own text. Cutting one of those narrows it, which lowers the
/// budget, which cuts it again, until a name is one letter -- so only a
/// label given the row's leftover room is one whose width says anything
/// about how much of the name fits.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct TreeRowLabelEllipsis;

/// The narrowest a row's label is ever laid out, in logical pixels.
///
/// Nine or so characters of the editor's body face: enough that a cut name
/// still says which node it is, and enough for the rename entry that
/// stands in its place to be typed into and read back.
pub const LABEL_MIN_WIDTH: f32 = 64.0;

/// What the ellipsis is written with. Three ASCII dots rather than the
/// single character, which the icon font has no glyph for.
const ELLIPSIS: &str = "...";

/// How wide one character is taken to be, as a fraction of the font
/// size, for a label that has not been laid out yet.
///
/// Only the frames before the text has a measurement use this. It is
/// deliberately under the average of the editor's body face: an estimate
/// that is a little narrow shows a character too many for one frame,
/// while one that is wide cuts a name that fits.
const CHAR_WIDTH_RATIO: f32 = 0.5;

/// Cut a row's label down to the room the row gave it, and hang the full
/// name off it as a tooltip.
///
/// Bevy has no ellipsis mode, so the cut is made here. A name that fits is
/// left exactly as it was written and carries no tooltip: a tooltip
/// repeating what is already on screen is noise.
///
/// Whether a name fits is a measurement, not an estimate: the width the
/// whole name was drawn at is kept on [`TreeRowLabelFit`] the frame it is
/// on screen, and every cut afterwards is worked out from it.
///
/// A label being renamed is left alone. The entry stands in its place and
/// the label itself is laid out to nothing, so the room it measures says
/// nothing about the name; cutting it would only mean the name that came
/// back on a cancel was the cut one.
pub fn ellipsize_tree_row_labels(
    mut labels: Query<
        (
            Entity,
            &mut Text,
            &ComputedNode,
            &TextLayoutInfo,
            Option<&mut TreeRowLabelFit>,
        ),
        (With<TreeRowLabelEllipsis>, Without<TreeRowInlineRename>),
    >,
    mut commands: Commands,
) {
    for (entity, mut text, computed, layout, fit) in &mut labels {
        let scale = computed.inverse_scale_factor();
        let available = computed.size().x * scale;
        if available <= 0.0 {
            continue;
        }
        let full = match fit.as_deref() {
            // The text is the one this wrote, so the name is whatever it
            // was cut from; anything else is a fresh write from outside.
            Some(fit) if fit.shown == text.0 => fit.full.clone(),
            _ => text.0.clone(),
        };
        let drawn = layout.size.x * scale;
        let full_width = if text.0 == full && drawn > 0.0 {
            drawn
        } else {
            match fit.as_deref() {
                Some(fit) if fit.full == full => fit.full_width,
                _ => 0.0,
            }
        };
        let per_char = if full_width > 0.0 {
            full_width / full.chars().count().max(1) as f32
        } else {
            tokens::TEXT_SIZE_PX * CHAR_WIDTH_RATIO
        };
        let wanted = if full_width > 0.0 && full_width <= available {
            full.clone()
        } else {
            let budget = (available / per_char).floor();
            let budget = if budget.is_finite() && budget > 0.0 {
                budget as usize
            } else {
                0
            };
            cut_to_fit(&full, budget)
        };
        if text.0 != wanted {
            text.0.clone_from(&wanted);
        }
        match fit {
            Some(mut fit) => {
                fit.full.clone_from(&full);
                fit.shown.clone_from(&wanted);
                fit.full_width = full_width;
            }
            None => {
                commands.entity(entity).insert(TreeRowLabelFit {
                    full: full.clone(),
                    shown: wanted.clone(),
                    full_width,
                });
            }
        }
        if wanted == full {
            commands.entity(entity).remove::<crate::tooltip::Tooltip>();
        } else {
            commands
                .entity(entity)
                .insert(crate::tooltip::Tooltip::title(full));
        }
    }
}

/// `name` shortened to `budget` characters, ending in [`ELLIPSIS`] when
/// anything was taken off.
///
/// A budget with no room for the ellipsis itself yields as many characters
/// of the name as there is room for: a row too narrow to say anything
/// should still say something.
fn cut_to_fit(name: &str, budget: usize) -> String {
    if name.chars().count() <= budget {
        return name.to_string();
    }
    if budget <= ELLIPSIS.len() {
        return name.chars().take(budget).collect();
    }
    let kept: String = name.chars().take(budget - ELLIPSIS.len()).collect();
    format!("{kept}{ELLIPSIS}")
}

/// Marks a row or a list currently painted for a drag hanging over it.
///
/// `DragEnter` paints; `DragLeave` and a drop paint back. A drag called
/// off part way through raises neither, so without a record of what is
/// painted the tint has nothing to take it off again. This is that
/// record, and it is also what says a drag is still live enough to cancel.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct TreeDropPainted;

/// Call off a drag on Escape: paint every tinted row and list back, drop
/// the line and the rest, and mark the lists so the release that follows
/// moves nothing.
///
/// Escape is read here rather than through the keymap for the reason the
/// arrow keys are: it is how a gesture in a list is abandoned, not a
/// command of its own, and the operator that owns Escape elsewhere would
/// have to know what a tree row is to answer it.
pub fn cancel_tree_drag_on_escape(
    keys: Res<ButtonInput<KeyCode>>,
    painted: Query<Entity, With<TreeDropPainted>>,
    rows: Query<(), With<TreeRowContent>>,
    selected: Query<(), With<TreeRowSelected>>,
    mut colours: Query<&mut BackgroundColor>,
    mut nodes: Query<&mut Node>,
    mut line: ResMut<TreeDropLine>,
    mut spring: ResMut<TreeSpringLoad>,
    mut cancelled: ResMut<TreeDragCancelled>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::Escape) || painted.is_empty() {
        return;
    }
    for entity in &painted {
        if let Ok(mut colour) = colours.get_mut(entity) {
            colour.0 = if !rows.contains(entity) {
                Color::NONE
            } else if selected.contains(entity) {
                tokens::SELECTED_BG
            } else {
                ROW_BG
            };
        }
        if rows.contains(entity)
            && let Ok(mut node) = nodes.get_mut(entity)
        {
            node.border = UiRect::all(px(1.0));
        }
        commands.entity(entity).remove::<TreeDropPainted>();
    }
    line.zone = None;
    spring.row = None;
    cancelled.0 = true;
}

/// Keep the drop line where [`TreeDropLine`] says, and nowhere when it
/// says nothing.
pub fn sync_tree_drop_line(
    mut commands: Commands,
    line: Res<TreeDropLine>,
    zones: Query<&TreeRowInsertZone>,
    parents: Query<&ChildOf>,
    transforms: Query<(&ComputedNode, &UiGlobalTransform)>,
    roots: Query<Entity, With<TreeRoot>>,
    indicators: Query<Entity, With<TreeDropIndicator>>,
    mut nodes: Query<&mut Node, With<TreeDropIndicator>>,
) {
    let wanted = line.zone.filter(|zone| zones.contains(*zone));
    let Some(zone) = wanted else {
        for indicator in &indicators {
            if let Ok(mut entity) = commands.get_entity(indicator) {
                entity.despawn();
            }
        }
        return;
    };
    let Some(root) = ancestor_tree_root(zone, &parents, &roots) else {
        return;
    };
    let (Ok((zone_computed, zone_transform)), Ok((root_computed, root_transform))) =
        (transforms.get(zone), transforms.get(root))
    else {
        return;
    };
    let side = zones.get(zone).is_ok_and(|side| side.after);
    // The gap the zone stands for is its own outer edge: the top of the
    // strip above a row, the bottom of the strip below one.
    //
    // Converted to logical pixels before it is written, because that is
    // what a `Node` offset is measured in while `UiGlobalTransform` and
    // `ComputedNode` are physical: at a scale factor of 2 the raw figure
    // would put the line twice as far down the tree as the gap it marks.
    let scale = root_computed.inverse_scale_factor();
    let centre =
        zone_transform.translation.y + if side { 1.0 } else { -1.0 } * zone_computed.size().y / 2.0;
    let top = (centre - (root_transform.translation.y - root_computed.size().y / 2.0)) * scale;
    let left = line.indent;

    match indicators.iter().next() {
        Some(indicator) => {
            if let Ok(mut node) = nodes.get_mut(indicator) {
                node.left = px(left);
                node.top = px(top - 1.0);
            }
        }
        None => {
            commands.spawn((
                TreeDropIndicator,
                Node {
                    position_type: PositionType::Absolute,
                    left: px(left),
                    right: px(0.0),
                    top: px(top - 1.0),
                    height: px(2.0),
                    ..default()
                },
                BackgroundColor(tokens::SELECTED_BORDER),
                ZIndex(10),
                Pickable::IGNORE,
                ChildOf(root),
            ));
        }
    }
}

/// Open a collapsed row the pointer has rested on during a drag.
///
/// A drag holds what it is carrying, so a subtree that is closed cannot
/// be opened by clicking it. Resting on it opens it instead, which is how
/// a nested drop is reached without letting go.
pub fn spring_load_tree_rows(
    time: Res<Time>,
    mut spring: ResMut<TreeSpringLoad>,
    expanded: Query<&TreeNodeExpanded>,
    mut commands: Commands,
) {
    let Some(row) = spring.row else { return };
    let Ok(state) = expanded.get(row) else {
        spring.row = None;
        return;
    };
    if state.0 {
        spring.row = None;
        return;
    }
    spring.waited += time.delta_secs();
    if spring.waited < SPRING_LOAD_DELAY {
        return;
    }
    spring.row = None;
    spring.waited = 0.0;
    if let Ok(mut entity) = commands.get_entity(row) {
        entity.insert(TreeNodeExpanded(true));
    }
}

/// Scroll the list while a drag rests near its top or bottom edge, so a
/// drop below the fold is reachable without letting go.
pub fn auto_scroll_tree_on_drag(
    time: Res<Time>,
    line: Res<TreeDropLine>,
    spring: Res<TreeSpringLoad>,
    pointers: Query<&bevy::picking::pointer::PointerLocation>,
    parents: Query<&ChildOf>,
    roots: Query<Entity, With<TreeRoot>>,
    transforms: Query<(&ComputedNode, &UiGlobalTransform)>,
    mut scrolls: Query<&mut ScrollPosition>,
) {
    let over = line.zone.or(spring.row);
    let Some(over) = over else { return };
    let Some(root) = ancestor_tree_root(over, &parents, &roots) else {
        return;
    };
    let Some(cursor) = pointers
        .iter()
        .find_map(|location| location.location().map(|it| it.position))
    else {
        return;
    };
    let Ok((computed, transform)) = transforms.get(root) else {
        return;
    };
    let half = computed.size().y / 2.0;
    let top = transform.translation.y - half;
    let bottom = transform.translation.y + half;
    let delta = if cursor.y < top + AUTO_SCROLL_MARGIN {
        -AUTO_SCROLL_SPEED
    } else if cursor.y > bottom - AUTO_SCROLL_MARGIN {
        AUTO_SCROLL_SPEED
    } else {
        return;
    };
    if let Ok(mut scroll) = scrolls.get_mut(root) {
        let content = computed.content_size().y * computed.inverse_scale_factor();
        let view = computed.size().y * computed.inverse_scale_factor();
        let max = (content - view).max(0.0);
        scroll.y = (scroll.y + delta * time.delta_secs()).clamp(0.0, max);
    }
}

/// The [`TreeRoot`] container `entity` sits under.
fn ancestor_tree_root(
    entity: Entity,
    parents: &Query<&ChildOf>,
    roots: &Query<Entity, With<TreeRoot>>,
) -> Option<Entity> {
    let mut current = entity;
    for _ in 0..64 {
        if roots.contains(current) {
            return Some(current);
        }
        let &ChildOf(parent) = parents.get(current).ok()?;
        current = parent;
    }
    None
}

/// Returns observers for the root tree container to handle deparenting (drop-to-root).
///
/// The wash the container paints means "release here and the entity
/// leaves its parent", so it belongs to the container's own empty space
/// and nothing else. Pointer events bubble, and the gap strips over every
/// row stop their `DragLeave` and `DragDrop` but let `DragEnter` through:
/// painting on a bubbled enter would wash the whole list from a drag that
/// only crossed a row, and nothing would ever paint it back.
///
/// It also belongs to a row going somewhere. A press on the list's empty
/// space and a drag from there is a gesture with nothing in it, and the
/// whole panel turning green until the button came back up said the
/// release would move something.
pub fn tree_container_drop_observers() -> impl Bundle {
    (
        observe(
            |mut drag_enter: On<Pointer<DragEnter>>,
             parents: Query<&ChildOf>,
             tree_nodes: Query<&TreeNode>,
             mut bg_query: Query<&mut BackgroundColor>,
             mut commands: Commands| {
                drag_enter.propagate(false);
                if drag_enter.event_target() != drag_enter.original_event_target() {
                    return;
                }
                if find_source_entity(drag_enter.dragged, &parents, &tree_nodes).is_none() {
                    return;
                }
                if let Ok(mut bg) = bg_query.get_mut(drag_enter.event_target()) {
                    bg.0 = tokens::CONTAINER_DROP_TARGET_BG;
                    commands
                        .entity(drag_enter.event_target())
                        .insert(TreeDropPainted);
                }
            },
        ),
        observe(
            |mut drag_leave: On<Pointer<DragLeave>>,
             mut bg_query: Query<&mut BackgroundColor>,
             mut line: ResMut<TreeDropLine>,
             mut spring: ResMut<TreeSpringLoad>,
             mut commands: Commands| {
                drag_leave.propagate(false);
                if let Ok(mut bg) = bg_query.get_mut(drag_leave.event_target()) {
                    bg.0 = Color::NONE;
                    commands
                        .entity(drag_leave.event_target())
                        .remove::<TreeDropPainted>();
                }
                // The drag has left the tree, so there is nowhere it would
                // land and nothing it is resting on.
                line.zone = None;
                spring.row = None;
            },
        ),
        observe(
            |mut drag_drop: On<Pointer<DragDrop>>,
             mut commands: Commands,
             parent_query: Query<&ChildOf>,
             tree_nodes: Query<&TreeNode>,
             mut bg_query: Query<&mut BackgroundColor>,
             mut cancelled: ResMut<TreeDragCancelled>| {
                drag_drop.propagate(false);
                let container = drag_drop.event_target();

                // Revert background
                if let Ok(mut bg) = bg_query.get_mut(container) {
                    bg.0 = Color::NONE;
                    commands.entity(container).remove::<TreeDropPainted>();
                }
                if std::mem::take(&mut cancelled.0) {
                    return;
                }

                // Resolve the dragged entity to its scene source
                let Some(dragged_source) =
                    find_source_entity(drag_drop.dropped, &parent_query, &tree_nodes)
                else {
                    return;
                };

                commands.trigger(TreeRowDroppedOnRoot {
                    entity: container,
                    dragged_source,
                });
            },
        ),
    )
}

/// Keyboard navigation for tree views: arrow keys, Enter, F2, Delete
///
/// These keys are read here rather than through the keymap, because they
/// are how a list is walked rather than commands of their own. That also
/// means nothing else stands them down, so the keybind dialog's recorder
/// has to be checked here: without it, naming a chord in the dialog walks
/// the outliner, or renames a row.
pub fn tree_keyboard_navigation(
    keyboard: Res<ButtonInput<KeyCode>>,
    capture: Option<Res<jackdaw_commands::KeymapCapture>>,
    mut focused: ResMut<TreeFocused>,
    tree_view: Query<&Children, With<TreeView>>,
    tree_nodes: Query<(Entity, &TreeNodeExpanded, &Children), With<TreeNode>>,
    tree_row_children: Query<&Children, With<TreeRowChildren>>,
    tree_row_contents: Query<Entity, With<TreeRowContent>>,
    node_query: Query<&Node>,
    mut commands: Commands,
    tree_node_query: Query<&TreeNode>,
    input_focus: Res<bevy::input_focus::InputFocus>,
) {
    if jackdaw_commands::KeymapCapture::is_recording(capture.as_deref()) {
        return;
    }
    // Skip tree keyboard navigation when a text input is focused
    // to avoid Enter/arrow keys interfering with text editing.
    if input_focus.get().is_some() {
        return;
    }
    // Collect all visible tree rows in order
    let visible_rows =
        collect_visible_rows(&tree_view, &tree_nodes, &tree_row_children, &node_query);

    if visible_rows.is_empty() {
        return;
    }

    let current_idx = focused
        .0
        .and_then(|f| visible_rows.iter().position(|&e| e == f));

    if keyboard.just_pressed(KeyCode::ArrowDown) {
        let next = match current_idx {
            Some(i) if i + 1 < visible_rows.len() => Some(visible_rows[i + 1]),
            None if !visible_rows.is_empty() => Some(visible_rows[0]),
            _ => focused.0,
        };
        focused.0 = next;
    }

    if keyboard.just_pressed(KeyCode::ArrowUp) {
        let prev = match current_idx {
            Some(i) if i > 0 => Some(visible_rows[i - 1]),
            None if !visible_rows.is_empty() => Some(*visible_rows.last().unwrap()),
            _ => focused.0,
        };
        focused.0 = prev;
    }

    if keyboard.just_pressed(KeyCode::ArrowLeft)
        && let Some(focused_entity) = focused.0
        && let Ok((entity, expanded, _)) = tree_nodes.get(focused_entity)
        && expanded.0
    {
        // Collapse the node
        commands.entity(entity).insert(TreeNodeExpanded(false));
    }
    // If already collapsed, could move to parent, but skipping for now.

    if keyboard.just_pressed(KeyCode::ArrowRight)
        && let Some(focused_entity) = focused.0
        && let Ok((entity, expanded, children)) = tree_nodes.get(focused_entity)
    {
        let has_children = children.iter().any(|c| tree_row_children.contains(c));
        if has_children && !expanded.0 {
            // Expand the node
            commands.entity(entity).insert(TreeNodeExpanded(true));
        }
    }

    // Enter/Space: select focused node
    if (keyboard.just_pressed(KeyCode::Enter) || keyboard.just_pressed(KeyCode::Space))
        && let Some(focused_entity) = focused.0
        && let Ok(tree_node) = tree_node_query.get(focused_entity)
    {
        // Find the TreeRowContent child to use as event target
        if let Ok((_, _, children)) = tree_nodes.get(focused_entity) {
            for child in children.iter() {
                if tree_row_contents.contains(child) {
                    commands.trigger(TreeRowClicked {
                        entity: child,
                        source_entity: tree_node.0,
                    });
                    break;
                }
            }
        }
    }

    // F2: start inline rename
    if keyboard.just_pressed(KeyCode::F2)
        && let Some(focused_entity) = focused.0
        && let Ok(tree_node) = tree_node_query.get(focused_entity)
    {
        commands.trigger(TreeRowStartRename {
            entity: focused_entity,
            source_entity: tree_node.0,
        });
    }
}

/// Collect all visible tree row entities in depth-first order
fn collect_visible_rows(
    tree_view: &Query<&Children, With<TreeView>>,
    tree_nodes: &Query<(Entity, &TreeNodeExpanded, &Children), With<TreeNode>>,
    tree_row_children: &Query<&Children, With<TreeRowChildren>>,
    node_query: &Query<&Node>,
) -> Vec<Entity> {
    let mut result = Vec::new();

    for view_children in tree_view.iter() {
        for child in view_children.iter() {
            collect_visible_rows_recursive(
                child,
                tree_nodes,
                tree_row_children,
                node_query,
                &mut result,
            );
        }
    }

    result
}

fn collect_visible_rows_recursive(
    entity: Entity,
    tree_nodes: &Query<(Entity, &TreeNodeExpanded, &Children), With<TreeNode>>,
    tree_row_children: &Query<&Children, With<TreeRowChildren>>,
    node_query: &Query<&Node>,
    result: &mut Vec<Entity>,
) {
    let Ok((_, expanded, children)) = tree_nodes.get(entity) else {
        return;
    };

    // Check if this node is visible (Display::Flex or default)
    if let Ok(node) = node_query.get(entity)
        && node.display == Display::None
    {
        return;
    }

    result.push(entity);

    if expanded.0 {
        // Find TreeRowChildren container and recurse into its children
        for child in children.iter() {
            if let Ok(row_children) = tree_row_children.get(child) {
                for grandchild in row_children.iter() {
                    collect_visible_rows_recursive(
                        grandchild,
                        tree_nodes,
                        tree_row_children,
                        node_query,
                        result,
                    );
                }
            }
        }
    }
}
