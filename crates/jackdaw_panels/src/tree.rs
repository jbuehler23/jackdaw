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

use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::area::DockAreaStyle;

/// Stable handle to a node inside a [`DockTree`].
///
/// Backed by a monotonically-incrementing `u64`. Ids are never reused,
/// so a removed-then-reinserted node gets a fresh id.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct NodeId(pub u64);

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

/// A leaf in the dock tree: an area that hosts tabbed windows.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DockLeaf {
    /// Stable area id. Built-in areas have canonical ids (see [`DefaultArea`](crate::area::DefaultArea));
    /// dynamic split areas use synthetic ids.
    pub area_id: String,
    pub style: DockAreaStyle,
    /// Window ids in tab order.
    pub windows: Vec<String>,
    /// Which window is currently shown. `None` means the leaf is empty.
    pub active: Option<String>,
}

impl DockLeaf {
    pub fn new(area_id: impl Into<String>, style: DockAreaStyle) -> Self {
        Self {
            area_id: area_id.into(),
            style,
            windows: Vec::new(),
            active: None,
        }
    }

    pub fn with_windows(mut self, windows: Vec<String>) -> Self {
        self.active = windows.first().cloned();
        self.windows = windows;
        self
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
    /// Single-tree root. Used by simple cases (and unit tests).
    pub root: Option<NodeId>,
    /// Multi-tree anchors keyed by stable slot id (e.g. `"left_top"`).
    /// Each anchor's value is the root of a sub-tree for that slot.
    /// When a sub-tree is split at runtime, the anchor is updated to
    /// point at the new sub-tree root.
    #[serde(default)]
    pub anchors: HashMap<String, NodeId>,
    #[serde(default)]
    next_id: u64,
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

    pub fn insert(&mut self, node: DockNode) -> NodeId {
        let id = self.fresh_id();
        self.nodes.insert(id, node);
        id
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

    /// Create a new leaf and bind it to the named anchor. If the anchor
    /// already exists, its previous root is replaced (but not despawned;
    /// the caller should clean up if needed).
    pub fn set_anchor_leaf(&mut self, anchor: impl Into<String>, leaf: DockLeaf) -> NodeId {
        let id = self.insert(DockNode::Leaf(leaf));
        self.anchors.insert(anchor.into(), id);
        id
    }

    /// Look up the root node for a named anchor.
    pub fn anchor(&self, anchor: &str) -> Option<NodeId> {
        self.anchors.get(anchor).copied()
    }

    /// Iterate `(anchor_name, root_node_id)` pairs in arbitrary order.
    pub fn iter_anchors(&self) -> impl Iterator<Item = (&str, NodeId)> {
        self.anchors.iter().map(|(k, v)| (k.as_str(), *v))
    }

    /// True if the given node is referenced by an anchor.
    pub fn is_anchor_root(&self, id: NodeId) -> bool {
        self.anchors.values().any(|v| *v == id)
    }

    /// Iterate every leaf reachable from a specific anchor's sub-tree.
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

    /// Find the leaf that contains the given window id.
    pub fn find_leaf(&self, window_id: &str) -> Option<NodeId> {
        self.nodes.iter().find_map(|(id, node)| match node {
            DockNode::Leaf(l) if l.windows.iter().any(|w| w == window_id) => Some(*id),
            _ => None,
        })
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

    /// Split `target` along `edge` and place `window` into the newly-
    /// created sibling leaf. Returns the id of the new leaf.
    ///
    /// `target` must be a leaf. The split's fraction defaults to 0.5
    /// (equal sizes); adjust afterwards via [`Self::set_fraction`].
    pub fn split(&mut self, target: NodeId, edge: Edge, window: String) -> Option<NodeId> {
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
        self.nodes.insert(
            new_leaf_id,
            DockNode::Leaf(
                DockLeaf::new(fresh_area_id(&window, new_leaf_id), new_style)
                    .with_windows(vec![window]),
            ),
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

        // Rewrite the parent pointer (or root / anchor) to point at the new split.
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
                // Target was a root. Update the single root and any anchor
                // pointing at it.
                if self.root == Some(target) {
                    self.root = Some(split_id);
                }
                for v in self.anchors.values_mut() {
                    if *v == target {
                        *v = split_id;
                    }
                }
            }
        }

        Some(new_leaf_id)
    }

    /// Set the split's fraction, clamped to `(0.05, 0.95)`.
    pub fn set_fraction(&mut self, split: NodeId, fraction: f32) {
        if let Some(DockNode::Split(s)) = self.nodes.get_mut(&split) {
            s.fraction = fraction.clamp(0.05, 0.95);
        }
    }

    /// Set which window is active in a leaf. No-op if the window isn't in
    /// the leaf's tab list.
    pub fn set_active(&mut self, leaf: NodeId, window_id: &str) {
        if let Some(DockNode::Leaf(l)) = self.nodes.get_mut(&leaf)
            && l.windows.iter().any(|w| w == window_id)
        {
            l.active = Some(window_id.to_string());
        }
    }

    /// Move `window` out of its current leaf and into `to` as the active
    /// tab. If the source leaf becomes empty, it is removed and the tree
    /// simplified. No-op if `window` isn't in the tree or `to` isn't a leaf.
    pub fn move_window(&mut self, window: &str, to: NodeId) {
        self.insert_window(window, to, false, None);
    }

    /// Move `window` out of its current leaf and into `to` at index `index` if some,
    /// otherwise as the last tab as the active tab.
    /// If the source leaf becomes empty, it is removed and the tree
    /// simplified. No-op if `window` isn't in the tree or `to` isn't a leaf.
    pub fn insert_window(
        &mut self,
        window: &str,
        to: NodeId,
        allow_same: bool,
        index: Option<usize>,
    ) {
        let Some(from) = self.find_leaf(window) else {
            return;
        };
        if !allow_same && from == to {
            return;
        }
        if !matches!(self.nodes.get(&to), Some(DockNode::Leaf(_))) {
            return;
        }
        // Remove from source.
        if let Some(DockNode::Leaf(l)) = self.nodes.get_mut(&from) {
            l.windows.retain(|w| w != window);
            if l.active.as_deref() == Some(window) {
                l.active = l.windows.first().cloned();
            }
        }
        // Append to destination and activate.
        if let Some(DockNode::Leaf(l)) = self.nodes.get_mut(&to) {
            if let Some(index) = index {
                l.windows
                    .insert(index.clamp(0, l.windows.len()), window.to_string());
            } else {
                l.windows.push(window.to_string());
            }
            l.active = Some(window.to_string());
        }
        // Source may be empty now; simplify will collapse it.
        self.simplify();
    }

    /// Remove a window from its leaf. If the leaf goes empty, the tree
    /// is simplified.
    pub fn remove_window(&mut self, window: &str) {
        let Some(leaf) = self.find_leaf(window) else {
            return;
        };
        if let Some(DockNode::Leaf(l)) = self.nodes.get_mut(&leaf) {
            l.windows.retain(|w| w != window);
            if l.active.as_deref() == Some(window) {
                l.active = l.windows.first().cloned();
            }
        }
        self.simplify();
    }

    /// Collapse the tree:
    /// - Remove empty leaves that aren't a top-level root or anchor root.
    ///   The surviving sibling of a removed leaf takes its place in the parent.
    /// - Splits whose children collapsed away are themselves removed.
    ///
    /// Never removes a leaf referenced by `root` or `anchors` even if
    /// empty. An empty root/anchor keeps the slot valid (e.g. after
    /// closing the last window in a built-in panel).
    pub fn simplify(&mut self) {
        loop {
            let single_root = self.root;
            let empty_leaf_with_parent: Option<NodeId> = self
                .nodes
                .iter()
                .find(|(id, node)| match node {
                    DockNode::Leaf(l) => {
                        l.windows.is_empty()
                            && Some(**id) != single_root
                            && !self.is_anchor_root(**id)
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

            // Rewrite grandparent pointer (or root / anchor).
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
                    for v in self.anchors.values_mut() {
                        if *v == parent_id {
                            *v = survivor;
                        }
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
/// splits of the same window don't collide.
fn fresh_area_id(window_id: &str, leaf_id: NodeId) -> String {
    format!("split.{window_id}.{}", leaf_id.0)
}

#[cfg(test)]
mod tests {
    use crate::area::{DefaultArea, ToAnchorId};

    use super::*;

    fn leaf(area_id: &str, windows: &[&str]) -> DockLeaf {
        DockLeaf::new(area_id, DockAreaStyle::TabBar)
            .with_windows(windows.iter().map(ToString::to_string).collect())
    }

    #[test]
    fn set_root_leaf_works() {
        let mut t = DockTree::new();
        let id = t.set_root_leaf(leaf("root", &["a"]));
        assert_eq!(t.root, Some(id));
        assert_eq!(t.leaves().count(), 1);
    }

    #[test]
    fn split_inserts_new_leaf_and_wraps_target() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a"]));
        let new_leaf = t.split(root, Edge::Right, "b".into()).unwrap();

        // Root is now a split.
        let root_split = t.nodes[&t.root.unwrap()].as_split().unwrap();
        assert_eq!(root_split.axis, SplitAxis::Horizontal);
        assert_eq!(root_split.a, root);
        assert_eq!(root_split.b, new_leaf);
        assert_eq!(root_split.fraction, 0.5);

        // The original leaf still has window "a".
        assert_eq!(t.nodes[&root].as_leaf().unwrap().windows, vec!["a"]);
        // New leaf has "b".
        assert_eq!(t.nodes[&new_leaf].as_leaf().unwrap().windows, vec!["b"]);
    }

    #[test]
    fn split_top_puts_new_in_a() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a"]));
        let new_leaf = t.split(root, Edge::Top, "b".into()).unwrap();
        let s = t.nodes[&t.root.unwrap()].as_split().unwrap();
        assert_eq!(s.a, new_leaf);
        assert_eq!(s.b, root);
    }

    #[test]
    fn split_bottom_puts_new_in_b() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a"]));
        let new_leaf = t.split(root, Edge::Bottom, "b".into()).unwrap();
        let s = t.nodes[&t.root.unwrap()].as_split().unwrap();
        assert_eq!(s.a, root);
        assert_eq!(s.b, new_leaf);
    }

    #[test]
    fn split_of_nested_leaf_preserves_other_sibling() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf(&DefaultArea::Left.anchor_id(), &["a"]));
        let right = t.split(root, Edge::Right, "b".into()).unwrap();
        let _deeper = t.split(right, Edge::Bottom, "c".into()).unwrap();

        // Left leaf (id == root) still reachable and unchanged.
        assert_eq!(t.nodes[&root].as_leaf().unwrap().windows, vec!["a"]);
        // New leaf under `right` still has "b" in the correct leaf.
        let b_leaf = t.find_leaf("b").unwrap();
        assert_eq!(t.nodes[&b_leaf].as_leaf().unwrap().windows, vec!["b"]);
    }

    #[test]
    fn move_window_relocates_and_activates() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a", "b"]));
        let right = t.split(root, Edge::Right, "c".into()).unwrap();
        t.move_window("a", right);

        assert_eq!(t.nodes[&root].as_leaf().unwrap().windows, vec!["b"]);
        let dest = t.nodes[&right].as_leaf().unwrap();
        assert_eq!(dest.windows, vec!["c", "a"]);
        assert_eq!(dest.active.as_deref(), Some("a"));
    }

    #[test]
    fn move_last_window_simplifies_tree() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a"]));
        let right = t.split(root, Edge::Right, "b".into()).unwrap();
        // Move "a" to the right leaf. Left leaf is now empty and should collapse.
        t.move_window("a", right);

        // The tree should now be a single leaf (right) at the root.
        assert!(matches!(t.nodes[&t.root.unwrap()], DockNode::Leaf(_)));
        assert_eq!(t.leaves().count(), 1);
        let surviving = t.nodes[&t.root.unwrap()].as_leaf().unwrap();
        assert_eq!(surviving.windows, vec!["b", "a"]);
    }

    #[test]
    fn remove_last_window_keeps_root_empty_leaf() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a"]));
        t.remove_window("a");

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
    fn set_active_requires_window_in_leaf() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a", "b"]));
        t.set_active(root, "b");
        assert_eq!(
            t.nodes[&root].as_leaf().unwrap().active.as_deref(),
            Some("b")
        );
        // Non-member window is a no-op.
        t.set_active(root, "z");
        assert_eq!(
            t.nodes[&root].as_leaf().unwrap().active.as_deref(),
            Some("b")
        );
    }

    #[test]
    fn serde_round_trip() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a"]));
        t.split(root, Edge::Right, "b".into());

        let json = serde_json::to_string(&t).unwrap();
        let restored: DockTree = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.leaves().count(), 2);
        assert!(restored.find_leaf("a").is_some());
        assert!(restored.find_leaf("b").is_some());
    }

    #[test]
    fn anchors_track_split_root_changes() {
        let mut t = DockTree::new();
        let leaf_id = t.set_anchor_leaf("left_top", leaf("left_top", &["scene_tree"]));
        assert_eq!(t.anchor("left_top"), Some(leaf_id));

        // Split the anchor's leaf. The anchor must follow.
        let _new = t.split(leaf_id, Edge::Bottom, "import".into()).unwrap();
        let new_anchor_root = t.anchor("left_top").unwrap();
        assert_ne!(new_anchor_root, leaf_id);
        assert!(matches!(t.nodes[&new_anchor_root], DockNode::Split(_)));
    }

    #[test]
    fn anchors_track_simplify_collapses() {
        let mut t = DockTree::new();
        let original = t.set_anchor_leaf("right", leaf("right", &["a"]));
        t.split(original, Edge::Right, "b".into()).unwrap();
        // Drain "a" out so the original leaf goes empty and gets collapsed.
        t.move_window("a", t.find_leaf("b").unwrap());
        // Anchor should now point at the surviving leaf containing "b".
        let anchor_root = t.anchor("right").unwrap();
        let surviving = t.nodes[&anchor_root].as_leaf().unwrap();
        assert!(surviving.windows.iter().any(|w| w == "b"));
    }

    #[test]
    fn nested_split_chain_simplifies_when_drained() {
        let mut t = DockTree::new();
        let root = t.set_root_leaf(leaf("root", &["a"]));
        let right = t.split(root, Edge::Right, "b".into()).unwrap();
        let _bottom = t.split(right, Edge::Bottom, "c".into()).unwrap();

        // Drain everything off the right subtree via move.
        t.move_window("b", root);
        t.move_window("c", root);

        assert!(matches!(t.nodes[&t.root.unwrap()], DockNode::Leaf(_)));
        assert_eq!(t.leaves().count(), 1);
    }
}
