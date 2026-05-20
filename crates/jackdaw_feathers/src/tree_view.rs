use bevy::{feathers::theme::ThemedText, prelude::*, ui_widgets::observe};
use bevy_monitors::prelude::{MonitorSelf, Mutation, NotifyChanged};
use jackdaw_widgets::tree_view::{
    EntityCategory, TreeChildrenPopulated, TreeFocused, TreeNode, TreeNodeExpandToggle,
    TreeNodeExpanded, TreeRowChildren, TreeRowClicked, TreeRowContent, TreeRowDot, TreeRowDropped,
    TreeRowDroppedOnRoot, TreeRowLabel, TreeRowSelected, TreeRowStartRename,
    TreeRowVisibilityToggle, TreeRowVisibilityToggled, TreeView,
};

use lucide_icons::Icon;

use crate::tokens;

pub const ROW_BG: Color = Color::NONE;
const INDENT_WIDTH: f32 = 16.0;
const TOGGLE_WIDTH: f32 = 18.0;
const DOT_COLUMN_WIDTH: f32 = 14.0;

/// Parameters for tree row icon font rendering
#[derive(Clone)]
pub struct TreeRowStyle {
    pub icon_font: Handle<Font>,
}

/// Returns the display color for an entity category.
pub fn category_color(category: EntityCategory) -> Color {
    match category {
        EntityCategory::Camera => tokens::CATEGORY_CAMERA,
        EntityCategory::Light => tokens::CATEGORY_LIGHT,
        EntityCategory::Mesh => tokens::CATEGORY_MESH,
        EntityCategory::Scene => tokens::CATEGORY_SCENE,
        EntityCategory::Prefab => tokens::CATEGORY_PREFAB,
        EntityCategory::Inherited => tokens::CATEGORY_INHERITED,
        EntityCategory::Entity => tokens::CATEGORY_ENTITY,
    }
}

/// Creates a tree row bundle for displaying an entity in the hierarchy
pub fn tree_row(
    label: &str,
    has_children: bool,
    selected: bool,
    source: Entity,
    category: EntityCategory,
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
            tree_row_content(label, has_children, selected, source, category, style),
            // Container for child rows (initially empty, populated lazily)
            (
                TreeRowChildren,
                Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::left(px(INDENT_WIDTH)),
                    margin: UiRect::left(px(tokens::SPACING_SM)),
                    border: UiRect::left(px(1.0)),
                    width: percent(100),
                    display: Display::None,
                    ..default()
                },
                BorderColor::all(tokens::CONNECTION_LINE),
            )
        ],
        // React to TreeNodeExpanded changes: toggle children visibility + chevron icon
        observe(
            |mutation: On<Mutation<TreeNodeExpanded>>,
             expanded_query: Query<(&TreeNodeExpanded, &Children)>,
             children_container: Query<Entity, With<TreeRowChildren>>,
             content_query: Query<&Children, With<TreeRowContent>>,
             toggle_query: Query<&Children, With<TreeNodeExpandToggle>>,
             mut node_query: Query<&mut Node>,
             mut text_query: Query<&mut Text>| {
                let entity = mutation.event_target();
                let Ok((expanded, children)) = expanded_query.get(entity) else {
                    return;
                };

                for child in children.iter() {
                    // Toggle TreeRowChildren display
                    if children_container.contains(child)
                        && let Ok(mut node) = node_query.get_mut(child)
                    {
                        node.display = if expanded.0 {
                            Display::Flex
                        } else {
                            Display::None
                        };
                    }

                    // Update chevron: TreeRowContent -> TreeNodeExpandToggle -> Text
                    if let Ok(content_children) = content_query.get(child) {
                        for cc in content_children.iter() {
                            if let Ok(toggle_children) = toggle_query.get(cc) {
                                for tc in toggle_children.iter() {
                                    if let Ok(mut text) = text_query.get_mut(tc) {
                                        text.0 = if expanded.0 {
                                            Icon::ChevronDown.unicode().to_string()
                                        } else {
                                            Icon::ChevronRight.unicode().to_string()
                                        };
                                    }
                                }
                            }
                        }
                    }
                }
            },
        ),
    )
}

fn tree_row_content(
    label: &str,
    has_children: bool,
    selected: bool,
    source: Entity,
    category: EntityCategory,
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
            // Expand toggle (chevron)
            expand_toggle(has_children, &style.icon_font),
            // Category icon
            category_dot(category, &style.icon_font),
            // Label
            (
                TreeRowLabel,
                Text::new(label),
                TextFont {
                    font_size: tokens::FONT_MD,
                    ..default()
                },
                Node {
                    flex_grow: 1.0,
                    ..default()
                },
                ThemedText,
            ),
            // Visibility toggle (eye icon)
            visibility_toggle(source, &style.icon_font)
        ],
        // Click handler for selection (left-click only)
        observe(move |mut click: On<Pointer<Click>>, mut commands: Commands| {
            if click.event.button != PointerButton::Primary {
                return;
            }
            click.propagate(false);
            commands.trigger(TreeRowClicked {
                entity: click.event_target(),
                source_entity: source,
            });
        }),
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
        // Drag-and-drop: highlight drop target with border accent
        observe(
            |mut drag_enter: On<Pointer<DragEnter>>,
             mut query: Query<(&mut BackgroundColor, &mut Node), With<TreeRowContent>>| {
                drag_enter.propagate(false);
                if let Ok((mut bg, mut node)) = query.get_mut(drag_enter.event_target()) {
                    bg.0 = tokens::DROP_TARGET_BG;
                    node.border = UiRect::left(px(3.0));
                }
            },
        ),
        observe(
            |mut drag_leave: On<Pointer<DragLeave>>,
             mut query: Query<(&mut BackgroundColor, &mut Node), With<TreeRowContent>>,
             selected: Query<(), With<TreeRowSelected>>| {
                drag_leave.propagate(false);
                if let Ok((mut bg, mut node)) = query.get_mut(drag_leave.event_target()) {
                    bg.0 = if selected.contains(drag_leave.event_target()) {
                        tokens::SELECTED_BG
                    } else {
                        ROW_BG
                    };
                    node.border = UiRect::all(px(1.0));
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
             selected_query: Query<(), With<TreeRowSelected>>| {
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

fn expand_toggle(has_children: bool, icon_font: &Handle<Font>) -> impl Bundle {
    let (text, font) = if has_children {
        (
            String::from(Icon::ChevronRight.unicode()),
            icon_font.clone(),
        )
    } else {
        (String::from(" "), Handle::default())
    };

    (
        TreeNodeExpandToggle,
        Node {
            width: px(TOGGLE_WIDTH),
            justify_content: JustifyContent::Center,
            ..default()
        },
        children![(
            Text::new(text),
            TextFont {
                font,
                font_size: tokens::FONT_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
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
                font: icon_font.clone(),
                font_size: tokens::FONT_SM,
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

/// Icon indicating entity category (matches Figma reference).
fn category_dot(category: EntityCategory, icon_font: &Handle<Font>) -> impl Bundle {
    let color = category_color(category);
    let icon_char = match category {
        EntityCategory::Camera => Icon::Video,
        EntityCategory::Light => Icon::Lightbulb,
        EntityCategory::Prefab => Icon::Package,
        EntityCategory::Inherited | EntityCategory::Mesh | EntityCategory::Scene => Icon::Box,
        EntityCategory::Entity => Icon::Dot,
    };
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
                font: icon_font.clone(),
                font_size: 12.0,
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

/// Returns observers for the root tree container to handle deparenting (drop-to-root).
pub fn tree_container_drop_observers() -> impl Bundle {
    (
        observe(
            |mut drag_enter: On<Pointer<DragEnter>>, mut bg_query: Query<&mut BackgroundColor>| {
                drag_enter.propagate(false);
                if let Ok(mut bg) = bg_query.get_mut(drag_enter.event_target()) {
                    bg.0 = tokens::CONTAINER_DROP_TARGET_BG;
                }
            },
        ),
        observe(
            |mut drag_leave: On<Pointer<DragLeave>>, mut bg_query: Query<&mut BackgroundColor>| {
                drag_leave.propagate(false);
                if let Ok(mut bg) = bg_query.get_mut(drag_leave.event_target()) {
                    bg.0 = Color::NONE;
                }
            },
        ),
        observe(
            |mut drag_drop: On<Pointer<DragDrop>>,
             mut commands: Commands,
             parent_query: Query<&ChildOf>,
             tree_nodes: Query<&TreeNode>,
             mut bg_query: Query<&mut BackgroundColor>| {
                drag_drop.propagate(false);
                let container = drag_drop.event_target();

                // Revert background
                if let Ok(mut bg) = bg_query.get_mut(container) {
                    bg.0 = Color::NONE;
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
pub fn tree_keyboard_navigation(
    keyboard: Res<ButtonInput<KeyCode>>,
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
    // Skip tree keyboard navigation when a text input is focused
    // to avoid Enter/arrow keys interfering with text editing.
    if input_focus.0.is_some() {
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
