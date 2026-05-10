//! Pure data model for the dock layout.
//!
//! Mature docking systems (`egui_dock`, Dear `ImGui`, Dockview) separate the
//! layout *data* from the UI *entities*. Mutations happen on the tree; a
//! reconciler materializes the tree into UI each frame. This module owns
//! the data side. No Bevy UI imports.
//!
//! Binary tree: every split has exactly two children. Multi-way layouts
//! are nested binary splits. Matches `egui_dock`'s `Node` enum and `ImGui`'s
//! `DockNode.ChildNodes[2]`.
//!
//! There is exactly one tree per workspace: a single `root` node that
//! is either a `Leaf` (the whole layout is one tabbed area) or a
//! `Split` containing the rest of the layout. Earlier versions kept a
//! separate sub-tree per named anchor (`left`, `right_sidebar`, etc.);
//! that's gone now in favour of a flat single-tree, which lets panels
//! be dragged anywhere without an anchor wall between them.

use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::area::DockAreaStyle;

/// Synthetic-area-id prefix produced by `fresh_area_id` for leaves
/// born from runtime splits.
pub const SYNTHETIC_AREA_ID_PREFIX: &str = "split.";

/// Stable handle to a node inside a [`DockTree`].
///
/// Backed by a monotonically-incrementing `u64`. Ids are never reused,
/// so a removed-then-reinserted node gets a fresh id.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct NodeId(pub u64);

/// Stable handle to a tab inside a [`DockLeaf`].
///
/// Distinct from [`NodeId`]: a `TabId` identifies a specific tab
/// instance, not the leaf that hosts it. Two tabs can carry the same
/// `window_id` (e.g. two Outliner tabs side-by-side) and still be
/// addressed independently for activate / move / close. Allocated
/// from a per-tree monotonic counter; never reused.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct TabId(pub u64);

impl TabId {
    /// Sentinel used while a [`DockLeaf`] is being constructed via
    /// [`DockLeaf::with_windows`]. The tree rewrites these to fresh
    /// ids when the leaf is inserted, so they should never appear in
    /// a live tree.
    pub(crate) const PENDING: TabId = TabId(0);
}

/// Which way a split divides its two children.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum SplitAxis {
    /// `a` is on the left, `b` is on the right.
    Horizontal,
    /// `a` is on the top, `b` is on the bottom.
    Vertical,
}

/// Which edge of a target the user dropped a window on.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

impl Edge {
    pub fn axis(self) -> SplitAxis {
        match self {
            Edge::Top | Edge::Bottom => SplitAxis::Vertical,
            Edge::Left | Edge::Right => SplitAxis::Horizontal,
        }
    }

    /// When splitting at this edge, does the new window go into child `a`
    /// (first/top/left) or child `b` (second/bottom/right)?
    pub fn puts_new_in_a(self) -> bool {
        matches!(self, Edge::Top | Edge::Left)
    }
}

/// One tab inside a [`DockLeaf`]. Pairs a `window_id` with a
/// per-tree-unique `TabId`, so two tabs of the same window kind can
/// coexist in one leaf and still be addressed independently.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DockTabEntry {
    pub window_id: String,
    pub id: TabId,
}

/// A leaf in the dock tree: an area that hosts tabbed windows.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DockLeaf {
    /// Stable area id. Built-in areas have canonical ids (see [`DefaultArea`](crate::area::DefaultArea));
    /// dynamic split areas use synthetic ids.
    pub area_id: String,
    pub style: DockAreaStyle,
    /// Tabs in display order.
    pub windows: Vec<DockTabEntry>,
    /// Which tab is currently shown. `None` means the leaf is empty.
    pub active: Option<TabId>,
    /// If true, [`DockTree::simplify`] keeps this leaf in the tree
    /// even when its window list is empty. Built-in editor regions
    /// (left sidebar, right sidebar, bottom dock, viewport center)
    /// flip this on so closing the last panel inside them leaves an
    /// empty placeholder rather than collapsing the surrounding split.
    ///
    /// Defaults to `false`; runtime splits and ad-hoc leaves are
    /// transient and should collapse when drained.
    #[serde(default)]
    pub persistent: bool,
}

impl DockLeaf {
    pub fn new(area_id: impl Into<String>, style: DockAreaStyle) -> Self {
        Self {
            area_id: area_id.into(),
            style,
            windows: Vec::new(),
            active: None,
            persistent: false,
        }
    }

    /// Seed the leaf with one tab per window id. The tabs carry
    /// `TabId::PENDING` until the leaf is inserted into a
    /// [`DockTree`], which rewrites them to fresh ids. Direct callers
    /// that already have a tree should use [`DockTree::add_tab`]
    /// instead.
    pub fn with_windows(mut self, windows: Vec<String>) -> Self {
        self.windows = windows
            .into_iter()
            .map(|window_id| DockTabEntry {
                window_id,
                id: TabId::PENDING,
            })
            .collect();
        self.active = self.windows.first().map(|t| t.id);
        self
    }

    /// Mark the leaf as persistent. Persistent leaves are preserved by
    /// [`DockTree::simplify`] when their window list goes empty.
    pub fn persistent(mut self) -> Self {
        self.persistent = true;
        self
    }

    /// True if [`DockTree::simplify`] should preserve this leaf when
    /// it goes empty.
    pub fn is_persistent(&self) -> bool {
        self.persistent
    }

    /// Iterate `(window_id, tab_id)` pairs in display order. Helper
    /// for callers that want both halves of every tab without
    /// destructuring `DockTabEntry`.
    pub fn tabs(&self) -> impl Iterator<Item = (&str, TabId)> {
        self.windows.iter().map(|t| (t.window_id.as_str(), t.id))
    }

    /// Position of the given tab in the tab bar, or `None` if the tab
    /// isn't in this leaf.
    pub fn tab_index(&self, id: TabId) -> Option<usize> {
        self.windows.iter().position(|t| t.id == id)
    }

    /// True if any tab in this leaf carries the given `window_id`.
    pub fn has_window(&self, window_id: &str) -> bool {
        self.windows.iter().any(|t| t.window_id == window_id)
    }
}

/// An internal split node. Divides its rect into two adjacent children.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DockSplit {
    pub axis: SplitAxis,
    /// Fraction of the parent's size given to child `a`, in `(0.0, 1.0)`.
    /// Clamped on write via [`DockTree::set_fraction`].
    pub fraction: f32,
    pub a: NodeId,
    pub b: NodeId,
}

/// Either a leaf (tabbed area) or a split (two children + axis).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DockNode {
    Leaf(DockLeaf),
    Split(DockSplit),
}

impl DockNode {
    pub fn as_leaf(&self) -> Option<&DockLeaf> {
        match self {
            DockNode::Leaf(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_leaf_mut(&mut self) -> Option<&mut DockLeaf> {
        match self {
            DockNode::Leaf(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_split(&self) -> Option<&DockSplit> {
        match self {
            DockNode::Split(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_split_mut(&mut self) -> Option<&mut DockSplit> {
        match self {
            DockNode::Split(s) => Some(s),
            _ => None,
        }
    }
}

/// The dock layout, as a pure data tree. Source of truth.
///
/// The ECS reconciler watches this for changes and keeps UI entities in
/// sync. Drag/drop/resize operations should mutate `DockTree`, never the
/// entities directly.
#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize)]
pub struct DockTree {
    pub nodes: HashMap<NodeId, DockNode>,
    /// Single tree root. Splits and leaves below it form the entire
    /// editor layout. `None` means the workspace has no layout yet
    /// (caller should seed one before reconciliation).
    pub root: Option<NodeId>,
    #[serde(default)]
    next_id: u64,
    /// Counter for [`TabId`] allocation. Starts at 1 so [`TabId::PENDING`]
    /// (zero) never collides with a live id.
    #[serde(default)]
    next_tab_id: u64,
}

impl DockTree {
    pub fn new() -> Self {
        Self::default()
    }

    fn fresh_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    fn fresh_tab_id(&mut self) -> TabId {
        // Skip 0 so live ids never collide with [`TabId::PENDING`].
        // The first call after a fresh tree returns `TabId(1)`.
        self.next_tab_id = self.next_tab_id.saturating_add(1).max(1);
        TabId(self.next_tab_id)
    }

    pub fn insert(&mut self, mut node: DockNode) -> NodeId {
        // Stamp fresh `TabId`s on any pending tabs the leaf was
        // constructed with via [`DockLeaf::with_windows`]. Splits
        // pass through untouched.
        if let DockNode::Leaf(ref mut leaf) = node {
            self.assign_pending_tab_ids(leaf);
        }
        let id = self.fresh_id();
        self.nodes.insert(id, node);
        id
    }

    fn assign_pending_tab_ids(&mut self, leaf: &mut DockLeaf) {
        // Replace `TabId::PENDING` placeholders with fresh ids. Any
        // already-real ids are kept (round-tripping a tree through
        // serde shouldn't re-stamp).
        let active_was_pending = leaf.active == Some(TabId::PENDING);
        let mut first_real: Option<TabId> = None;
        for tab in leaf.windows.iter_mut() {
            if tab.id == TabId::PENDING {
                tab.id = self.fresh_tab_id();
            }
            if first_real.is_none() {
                first_real = Some(tab.id);
            }
        }
        if active_was_pending {
            leaf.active = first_real;
        }
    }

    /// Append a fresh tab carrying `window_id` to `leaf`, allocate a
    /// [`TabId`], and make it the active tab. Returns the new id.
    /// No-op (returns `None`) if `leaf` isn't a leaf node.
    pub fn add_tab(&mut self, leaf: NodeId, window_id: impl Into<String>) -> Option<TabId> {
        let window_id = window_id.into();
        if !matches!(self.nodes.get(&leaf), Some(DockNode::Leaf(_))) {
            return None;
        }
        let id = self.fresh_tab_id();
        let DockNode::Leaf(l) = self.nodes.get_mut(&leaf)? else {
            return None;
        };
        l.windows.push(DockTabEntry { window_id, id });
        l.active = Some(id);
        Some(id)
    }

    pub fn get(&self, id: NodeId) -> Option<&DockNode> {
        self.nodes.get(&id)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut DockNode> {
        self.nodes.get_mut(&id)
    }

    /// Set `root` to a freshly-inserted leaf. Returns its id.
    pub fn set_root_leaf(&mut self, leaf: DockLeaf) -> NodeId {
        let id = self.insert(DockNode::Leaf(leaf));
        self.root = Some(id);
        id
    }

    /// Iterate every leaf reachable from the given subtree root.
    pub fn leaves_under(&self, root: NodeId) -> Vec<(NodeId, &DockLeaf)> {
        let mut out = Vec::new();
        self.leaves_under_inner(root, &mut out);
        out
    }

    fn leaves_under_inner<'a>(&'a self, id: NodeId, out: &mut Vec<(NodeId, &'a DockLeaf)>) {
        match self.nodes.get(&id) {
            Some(DockNode::Leaf(l)) => out.push((id, l)),
            Some(DockNode::Split(s)) => {
                let (a, b) = (s.a, s.b);
                self.leaves_under_inner(a, out);
                self.leaves_under_inner(b, out);
            }
            None => {}
        }
    }

    /// Find the leaf that contains a tab carrying the given window
    /// id. Returns the first match; multi-instance windows can live in
    /// several leaves at once, in which case prefer
    /// [`Self::find_leaf_for_tab`] with a specific [`TabId`].
    pub fn find_leaf_with_window(&self, window_id: &str) -> Option<NodeId> {
        self.nodes.iter().find_map(|(id, node)| match node {
            DockNode::Leaf(l) if l.has_window(window_id) => Some(*id),
            _ => None,
        })
    }

    /// Find the leaf hosting the given tab id.
    pub fn find_leaf_for_tab(&self, tab: TabId) -> Option<NodeId> {
        self.nodes.iter().find_map(|(id, node)| match node {
            DockNode::Leaf(l) if l.windows.iter().any(|t| t.id == tab) => Some(*id),
            _ => None,
        })
    }

    /// Iterate every `(leaf, tab)` pair across the tree. Useful for
    /// callers that need to enumerate every instance of a window kind.
    pub fn tabs(&self) -> impl Iterator<Item = (NodeId, &DockTabEntry)> {
        self.leaves()
            .flat_map(|(leaf_id, leaf)| leaf.windows.iter().map(move |t| (leaf_id, t)))
    }

    /// Find the leaf with the given canonical `area_id`.
    pub fn find_by_area_id(&self, area_id: &str) -> Option<NodeId> {
        self.nodes.iter().find_map(|(id, node)| match node {
            DockNode::Leaf(l) if l.area_id == area_id => Some(*id),
            _ => None,
        })
    }

    /// Return the parent split of a node, or `None` if it's the root.
    pub fn parent_of(&self, child: NodeId) -> Option<NodeId> {
        self.nodes.iter().find_map(|(id, node)| match node {
            DockNode::Split(s) if s.a == child || s.b == child => Some(*id),
            _ => None,
        })
    }

    /// Every leaf in the tree, in arbitrary order.
    pub fn leaves(&self) -> impl Iterator<Item = (NodeId, &DockLeaf)> {
        self.nodes.iter().filter_map(|(id, node)| match node {
            DockNode::Leaf(l) => Some((*id, l)),
            _ => None,
        })
    }

    /// Depth-first iteration from `root`, yielding `(id, depth)`.
    pub fn iter_dfs(&self) -> Vec<(NodeId, usize)> {
        let mut out = Vec::new();
        if let Some(root) = self.root {
            self.dfs_into(root, 0, &mut out);
        }
        out
    }

    fn dfs_into(&self, id: NodeId, depth: usize, out: &mut Vec<(NodeId, usize)>) {
        out.push((id, depth));
        if let Some(DockNode::Split(s)) = self.nodes.get(&id) {
            let (a, b) = (s.a, s.b);
            self.dfs_into(a, depth + 1, out);
            self.dfs_into(b, depth + 1, out);
        }
    }

    /// Split `target` along `edge` and place a new tab carrying
    /// `window` into the freshly-created sibling leaf. Returns
    /// `(new_leaf, tab_id)` so callers can drive follow-up
    /// activate / move logic against the just-spawned tab.
    ///
    /// `target` must be a leaf. The split's fraction defaults to 0.5
    /// (equal sizes); adjust afterwards via [`Self::set_fraction`].
    pub fn split(&mut self, target: NodeId, edge: Edge, window: String) -> Option<(NodeId, TabId)> {
        // Ensure target is a leaf.
        if !matches!(self.nodes.get(&target), Some(DockNode::Leaf(_))) {
            return None;
        }

        // Inherit the target leaf's style for the new sibling.
        let new_style = self
            .nodes
            .get(&target)
            .and_then(|n| n.as_leaf())
            .map(|l| l.style.clone())
            .unwrap_or_default();

        // New leaf holding the dropped window. Reserve a NodeId first
        // so we can use it to make the synthetic area_id unique;
        // otherwise multiple splits of the same window would collide.
        let new_leaf_id = self.fresh_id();
        let tab_id = self.fresh_tab_id();
        self.nodes.insert(
            new_leaf_id,
            DockNode::Leaf(DockLeaf {
                area_id: fresh_area_id(&window, new_leaf_id),
                style: new_style,
                windows: vec![DockTabEntry {
                    window_id: window,
                    id: tab_id,
                }],
                active: Some(tab_id),
                persistent: false,
            }),
        );

        // Figure out target's parent first.
        let parent = self.parent_of(target);

        // Assemble a new split node.
        let (a, b) = if edge.puts_new_in_a() {
            (new_leaf_id, target)
        } else {
            (target, new_leaf_id)
        };
        let split_id = self.insert(DockNode::Split(DockSplit {
            axis: edge.axis(),
            fraction: 0.5,
            a,
            b,
        }));

        // Rewrite the parent pointer (or root) to point at the new split.
        match parent {
            Some(parent_id) => {
                if let Some(DockNode::Split(s)) = self.nodes.get_mut(&parent_id) {
                    if s.a == target {
                        s.a = split_id;
                    }
                    if s.b == target {
                        s.b = split_id;
                    }
                }
            }
            None => {
                if self.root == Some(target) {
                    self.root = Some(split_id);
                }
            }
        }

        Some((new_leaf_id, tab_id))
    }

    /// Set the split's fraction, clamped to `(0.05, 0.95)`.
    pub fn set_fraction(&mut self, split: NodeId, fraction: f32) {
        if let Some(DockNode::Split(s)) = self.nodes.get_mut(&split) {
            s.fraction = fraction.clamp(0.05, 0.95);
        }
    }

    /// Make `tab` the active tab in `leaf`. No-op if the tab isn't in
    /// the leaf's tab list.
    pub fn set_active(&mut self, leaf: NodeId, tab: TabId) {
        if let Some(DockNode::Leaf(l)) = self.nodes.get_mut(&leaf)
            && l.windows.iter().any(|t| t.id == tab)
        {
            l.active = Some(tab);
        }
    }

    /// Move `tab` out of its current leaf and into `to` as the active
    /// tab. If the source leaf becomes empty (and isn't persistent), it
    /// is removed and the tree simplified. No-op if `tab` isn't in the
    /// tree or `to` isn't a leaf.
    pub fn move_tab(&mut self, tab: TabId, to: NodeId) {
        self.insert_tab(tab, to, false, None);
    }

    /// Move `tab` out of its current leaf and into `to`. `index` slots
    /// the tab at the given position (clamped); `None` appends.
    /// `allow_same = true` lets a tab be reordered within its current
    /// leaf, otherwise same-leaf moves are no-ops.
    pub fn insert_tab(&mut self, tab: TabId, to: NodeId, allow_same: bool, index: Option<usize>) {
        let Some(from) = self.find_leaf_for_tab(tab) else {
            return;
        };
        if !allow_same && from == to {
            return;
        }
        if !matches!(self.nodes.get(&to), Some(DockNode::Leaf(_))) {
            return;
        }
        // Pluck the entry out of the source. Holds the (window_id,
        // tab_id) pair while we move it; ids never change as a tab
        // changes leaves.
        let entry = {
            let Some(DockNode::Leaf(l)) = self.nodes.get_mut(&from) else {
                return;
            };
            let Some(pos) = l.windows.iter().position(|t| t.id == tab) else {
                return;
            };
            let entry = l.windows.remove(pos);
            if l.active == Some(tab) {
                l.active = l.windows.first().map(|t| t.id);
            }
            entry
        };
        if let Some(DockNode::Leaf(l)) = self.nodes.get_mut(&to) {
            let new_id = entry.id;
            match index {
                Some(idx) => l.windows.insert(idx.clamp(0, l.windows.len()), entry),
                None => l.windows.push(entry),
            }
            l.active = Some(new_id);
        }
        // Source may be empty now; simplify will collapse it.
        self.simplify();
    }

    /// Remove `tab` from its leaf. If the leaf goes empty (and isn't
    /// persistent), the tree is simplified.
    pub fn remove_tab(&mut self, tab: TabId) {
        let Some(leaf) = self.find_leaf_for_tab(tab) else {
            return;
        };
        if let Some(DockNode::Leaf(l)) = self.nodes.get_mut(&leaf) {
            l.windows.retain(|t| t.id != tab);
            if l.active == Some(tab) {
                l.active = l.windows.first().map(|t| t.id);
            }
        }
        self.simplify();
    }

    /// Convenience: drop every tab whose `window_id` matches. Used by
    /// dock-tree maintenance paths that want to purge a kind of
    /// window wholesale (e.g. removing a viewport panel via the icon
    /// sidebar's close action).
    pub fn remove_window_kind(&mut self, window_id: &str) {
        let to_remove: Vec<TabId> = self
            .tabs()
            .filter(|(_, t)| t.window_id == window_id)
            .map(|(_, t)| t.id)
            .collect();
        for tab in to_remove {
            self.remove_tab(tab);
        }
    }

    /// Collapse the tree:
    /// - Remove empty leaves that aren't the root or marked persistent.
    ///   The surviving sibling of a removed leaf takes its place in the parent.
    /// - Splits whose children collapsed away are themselves removed.
    ///
    /// Never removes a persistent leaf (one with a stable hand-picked
    /// `area_id`, see [`DockLeaf::is_persistent`]) even when empty. An
    /// empty persistent leaf keeps the built-in slot visible (e.g.
    /// after closing the last panel in the right sidebar).
    pub fn simplify(&mut self) {
        loop {
            let root = self.root;
            let empty_leaf_with_parent: Option<NodeId> = self
                .nodes
                .iter()
                .find(|(id, node)| match node {
                    DockNode::Leaf(l) => {
                        l.windows.is_empty() && Some(**id) != root && !l.is_persistent()
                    }
                    _ => false,
                })
                .map(|(id, _)| *id);

            let Some(empty_id) = empty_leaf_with_parent else {
                return;
            };
            let Some(parent_id) = self.parent_of(empty_id) else {
                // No parent. The leaf is a stray root we can't simplify.
                self.nodes.remove(&empty_id);
                continue;
            };
            let Some(DockNode::Split(s)) = self.nodes.get(&parent_id).cloned() else {
                return;
            };
            // The other child of the parent replaces the parent.
            let survivor = if s.a == empty_id { s.b } else { s.a };

            // Rewrite grandparent pointer (or root).
            let grandparent = self.parent_of(parent_id);
            match grandparent {
                Some(gp_id) => {
                    if let Some(DockNode::Split(gs)) = self.nodes.get_mut(&gp_id) {
                        if gs.a == parent_id {
                            gs.a = survivor;
                        }
                        if gs.b == parent_id {
                            gs.b = survivor;
                        }
                    }
                }
                None => {
                    if self.root == Some(parent_id) {
                        self.root = Some(survivor);
                    }
                }
            }

            // Despawn the now-orphaned empty leaf and parent split.
            self.nodes.remove(&empty_id);
            self.nodes.remove(&parent_id);
        }
    }
}

/// Generate a unique synthetic area id for a newly-created split leaf.
/// Pairs the source window with the new leaf's `NodeId` so independent
/// splits of the same window don't collide. The
/// [`SYNTHETIC_AREA_ID_PREFIX`] is what [`DockLeaf::is_persistent`]
/// uses to distinguish runtime-split leaves from built-in canonical
/// regions.
fn fresh_area_id(window_id: &str, leaf_id: NodeId) -> String {
    format!("{SYNTHETIC_AREA_ID_PREFIX}{window_id}.{}", leaf_id.0)
}

#[cfg(test)]
mod tests {
    use crate::area::{DefaultArea, ToAnchorId};

    use super::*;

    fn leaf(area_id: &str, windows: &[&str]) -> DockLeaf {
        DockLeaf::new(area_id, DockAreaStyle::TabBar)
            .with_windows(windows.iter().map(ToString::to_string).collect())
    }

    /// Window ids on a leaf in tab order (drops the `TabId`s for
    /// readable assertions).
    fn window_ids(t: &DockTree, leaf: NodeId) -> Vec<String> {
        t.nodes[&leaf]
            .as_leaf()
            .unwrap()
            .windows
            .iter()
            .map(|w| w.window_id.clone())
            .collect()
    }

    /// `TabId` of the active tab on a leaf.
    fn active_window_id<'a>(t: &'a DockTree, leaf: NodeId) -> Option<&'a str> {
        let l = t.nodes[&leaf].as_leaf()?;
        let id = l.active?;
        l.windows
            .iter()
            .find(|w| w.id == id)
            .map(|w| w.window_id.as_str())
    }

    /// `TabId` of the first tab carrying the given `window_id` in the
    /// given leaf. Useful in tests because builders insert tabs with
    /// fresh ids that aren't known at the call site.
    fn tab_id_for(t: &DockTree, leaf: NodeId, window_id: &str) -> TabId {
        t.nodes[&leaf]
            .as_leaf()
            .unwrap()
            .windows
            .iter()
            .find(|w| w.window_id == window_id)
            .unwrap()
            .id
    }

    #[test]
    fn set_root_leaf_works() {
        let mut t = DockTree::new();
        let id = t.set_root_leaf(leaf("root", &["a"]));
        assert_eq!(t.root, Some(id));
        assert_eq!(t.leaves().count(), 1);
    }

    #[test]
    fn pending_tab_ids_are_stamped_on_insert() {
        // `with_windows` seeds tabs with `TabId::PENDING`; the tree
        // must rewrite them as the leaf is inserted so live ids never
        // collide with the sentinel and active points at a real tab.
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a", "b"]));
        let l = t.nodes[&root].as_leaf().unwrap();
        assert!(l.windows.iter().all(|w| w.id != TabId::PENDING));
        assert_eq!(l.active, Some(l.windows[0].id));
    }

    #[test]
    fn split_inserts_new_leaf_and_wraps_target() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a"]));
        let (new_leaf, _) = t.split(root, Edge::Right, "b".into()).unwrap();

        // Root is now a split.
        let root_split = t.nodes[&t.root.unwrap()].as_split().unwrap();
        assert_eq!(root_split.axis, SplitAxis::Horizontal);
        assert_eq!(root_split.a, root);
        assert_eq!(root_split.b, new_leaf);
        assert_eq!(root_split.fraction, 0.5);

        assert_eq!(window_ids(&t, root), vec!["a"]);
        assert_eq!(window_ids(&t, new_leaf), vec!["b"]);
    }

    #[test]
    fn split_top_puts_new_in_a() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a"]));
        let (new_leaf, _) = t.split(root, Edge::Top, "b".into()).unwrap();
        let s = t.nodes[&t.root.unwrap()].as_split().unwrap();
        assert_eq!(s.a, new_leaf);
        assert_eq!(s.b, root);
    }

    #[test]
    fn split_bottom_puts_new_in_b() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a"]));
        let (new_leaf, _) = t.split(root, Edge::Bottom, "b".into()).unwrap();
        let s = t.nodes[&t.root.unwrap()].as_split().unwrap();
        assert_eq!(s.a, root);
        assert_eq!(s.b, new_leaf);
    }

    #[test]
    fn split_of_nested_leaf_preserves_other_sibling() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf(&DefaultArea::Left.anchor_id(), &["a"]));
        let (right, _) = t.split(root, Edge::Right, "b".into()).unwrap();
        let _deeper = t.split(right, Edge::Bottom, "c".into()).unwrap();

        assert_eq!(window_ids(&t, root), vec!["a"]);
        let b_leaf = t.find_leaf_with_window("b").unwrap();
        assert_eq!(window_ids(&t, b_leaf), vec!["b"]);
    }

    #[test]
    fn move_tab_relocates_and_activates() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a", "b"]));
        let (right, _) = t.split(root, Edge::Right, "c".into()).unwrap();
        let tab_a = tab_id_for(&t, root, "a");
        t.move_tab(tab_a, right);

        assert_eq!(window_ids(&t, root), vec!["b"]);
        assert_eq!(window_ids(&t, right), vec!["c", "a"]);
        assert_eq!(active_window_id(&t, right), Some("a"));
    }

    #[test]
    fn move_last_tab_simplifies_tree() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a"]));
        let (right, _) = t.split(root, Edge::Right, "b".into()).unwrap();
        let tab_a = tab_id_for(&t, root, "a");
        t.move_tab(tab_a, right);

        assert!(matches!(t.nodes[&t.root.unwrap()], DockNode::Leaf(_)));
        assert_eq!(t.leaves().count(), 1);
        let surviving = t.root.unwrap();
        assert_eq!(window_ids(&t, surviving), vec!["b", "a"]);
    }

    #[test]
    fn remove_last_tab_keeps_root_empty_leaf() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a"]));
        let tab_a = tab_id_for(&t, root, "a");
        t.remove_tab(tab_a);

        assert_eq!(t.root, Some(root));
        assert!(t.nodes[&root].as_leaf().unwrap().windows.is_empty());
    }

    #[test]
    fn set_fraction_clamps() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a"]));
        t.split(root, Edge::Right, "b".into());
        let split_id = t.root.unwrap();
        t.set_fraction(split_id, 0.0);
        assert!(t.nodes[&split_id].as_split().unwrap().fraction >= 0.05);
        t.set_fraction(split_id, 1.5);
        assert!(t.nodes[&split_id].as_split().unwrap().fraction <= 0.95);
    }

    #[test]
    fn set_active_requires_tab_in_leaf() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a", "b"]));
        let tab_b = tab_id_for(&t, root, "b");
        t.set_active(root, tab_b);
        assert_eq!(active_window_id(&t, root), Some("b"));
        // Stranger tab id from elsewhere is a no-op.
        t.set_active(root, TabId(9999));
        assert_eq!(active_window_id(&t, root), Some("b"));
    }

    #[test]
    fn duplicate_window_kind_supported() {
        // The point of `TabId`: two tabs of the same window kind can
        // share a leaf and still be addressed independently.
        let mut t = DockTree::new();
        let root = t.set_root_leaf(DockLeaf::new("root", DockAreaStyle::TabBar));
        let first = t.add_tab(root, "outliner").unwrap();
        let second = t.add_tab(root, "outliner").unwrap();
        assert_ne!(first, second);
        assert_eq!(window_ids(&t, root), vec!["outliner", "outliner"]);

        // Closing the second leaves the first.
        t.remove_tab(second);
        let l = t.nodes[&root].as_leaf().unwrap();
        assert_eq!(l.windows.len(), 1);
        assert_eq!(l.windows[0].id, first);
        assert_eq!(l.active, Some(first));
    }

    #[test]
    fn serde_round_trip() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a"]));
        t.split(root, Edge::Right, "b".into());

        let json = serde_json::to_string(&t).unwrap();
        let restored: DockTree = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.leaves().count(), 2);
        assert!(restored.find_leaf_with_window("a").is_some());
        assert!(restored.find_leaf_with_window("b").is_some());
    }

    #[test]
    fn persistent_leaf_kept_when_emptied_via_simplify() {
        // A leaf marked persistent stays in the tree even after its
        // windows go empty, so closing the last panel in a built-in
        // sidebar leaves an empty placeholder rather than collapsing
        // the surrounding split.
        let mut t = DockTree::new();
        let original = t.insert(DockNode::Leaf(
            DockLeaf::new("right", DockAreaStyle::TabBar)
                .with_windows(vec!["a".into()])
                .persistent(),
        ));
        t.root = Some(original);
        let (other, _) = t.split(original, Edge::Right, "b".into()).unwrap();
        let tab_a = tab_id_for(&t, original, "a");
        t.move_tab(tab_a, other);
        let persistent_leaf = t.nodes[&original].as_leaf().unwrap();
        assert!(persistent_leaf.windows.is_empty());
        assert!(persistent_leaf.is_persistent());
    }

    #[test]
    fn nested_split_chain_simplifies_when_drained() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a"]));
        let (right, _) = t.split(root, Edge::Right, "b".into()).unwrap();
        let _bottom = t.split(right, Edge::Bottom, "c".into()).unwrap();

        // Drain everything off the right subtree via move.
        let tab_b = tab_id_for(&t, right, "b");
        t.move_tab(tab_b, root);
        let bottom_leaf = t.find_leaf_with_window("c").unwrap();
        let tab_c = tab_id_for(&t, bottom_leaf, "c");
        t.move_tab(tab_c, root);

        assert!(matches!(t.nodes[&t.root.unwrap()], DockNode::Leaf(_)));
        assert_eq!(t.leaves().count(), 1);
    }
}
