//! Extensible registry of node types.
//!
//! Consumer crates (animation, shaders, materials) register their node
//! descriptors here during plugin setup. The canvas reads the registry to
//! build node UI, populate the "Add Node" context menu, and validate
//! connection compatibility.

use bevy::prelude::*;
use std::collections::{BTreeMap, HashMap};

use crate::graph::TerminalDirection;

/// Blueprint for one input or output terminal on a node type.
#[derive(Clone, Debug)]
pub struct TerminalDescriptor {
    /// Display label shown next to the terminal dot.
    pub label: String,
    /// Compatibility key; only terminals with matching `data_type` can connect.
    pub data_type: String,
    /// Dot color.
    pub color: Color,
}

/// Blueprint for a node type.
///
/// Registered once during plugin setup. Instances are spawned via
/// `AddGraphNodeCmd` which reads this descriptor to create the UI and body
/// components.
#[derive(Clone, Debug)]
pub struct NodeTypeDescriptor {
    /// Registry key (e.g. `"anim.state"`). Also stored on `GraphNode.node_type`.
    pub id: String,
    /// Human-readable name shown in the title bar and context menu.
    pub display_name: String,
    /// Context-menu grouping key (e.g. `"Animation"`, `"Math"`).
    pub category: String,
    /// Title-bar accent color.
    pub accent_color: Color,
    pub inputs: Vec<TerminalDescriptor>,
    pub outputs: Vec<TerminalDescriptor>,
    /// Full type paths of extra reflected components spawned alongside the node
    /// (for inline parameter editing via the inspector reflect-field UI).
    pub body_components: Vec<String>,
}

impl NodeTypeDescriptor {
    /// Look up a terminal descriptor by direction + index.
    pub fn terminal(
        &self,
        direction: TerminalDirection,
        index: u32,
    ) -> Option<&TerminalDescriptor> {
        let idx = index as usize;
        match direction {
            TerminalDirection::Input => self.inputs.get(idx),
            TerminalDirection::Output => self.outputs.get(idx),
        }
    }
}

/// Resource holding every registered node type, keyed by id.
#[derive(Resource, Default, Debug)]
pub struct NodeTypeRegistry {
    types: HashMap<String, NodeTypeDescriptor>,
}

impl NodeTypeRegistry {
    /// Register a node type. Replaces any existing entry with the same id.
    pub fn register(&mut self, descriptor: NodeTypeDescriptor) {
        self.types.insert(descriptor.id.clone(), descriptor);
    }

    pub fn get(&self, id: &str) -> Option<&NodeTypeDescriptor> {
        self.types.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &NodeTypeDescriptor> {
        self.types.values()
    }

    /// Group registered node types by their `category`, sorted alphabetically
    /// within each group. Used to populate the "Add Node" context menu.
    pub fn by_category(&self) -> BTreeMap<&str, Vec<&NodeTypeDescriptor>> {
        let mut out: BTreeMap<&str, Vec<&NodeTypeDescriptor>> = BTreeMap::new();
        for desc in self.types.values() {
            out.entry(desc.category.as_str()).or_default().push(desc);
        }
        for entries in out.values_mut() {
            entries.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        }
        out
    }
}
