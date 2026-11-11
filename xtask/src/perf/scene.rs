//! What the probe reads from `jackdaw/scene_tree`.

use serde_json::Value;

/// The size of an open scene, and the nodes the probe acts on.
#[derive(Debug, Default, PartialEq)]
pub struct SceneShape {
    pub nodes: usize,
    pub brushes: usize,
    /// The editor entity of the first brush, else of the first node.
    pub target: Option<u64>,
    /// The scene node id of the last node in outliner order that is not a
    /// brush, else of the last node.
    pub last_node: Option<u64>,
}

impl SceneShape {
    /// Read a `jackdaw/scene_tree` answer.
    pub fn of(tree: &Value) -> Self {
        let mut shape = Self::default();
        let mut first_node = None;
        let mut last_brush = None;
        let mut last_other = None;
        let mut stack: Vec<&Value> = tree["tree"]
            .as_array()
            .map(|roots| roots.iter().rev().collect())
            .unwrap_or_default();
        while let Some(node) = stack.pop() {
            shape.nodes += 1;
            let entity = node["entity"].as_u64();
            first_node = first_node.or(entity);
            let brush = node["components"]
                .as_array()
                .is_some_and(|components| components.iter().any(is_brush));
            let id = node["node"].as_u64();
            if brush {
                shape.brushes += 1;
                shape.target = shape.target.or(entity);
                last_brush = id.or(last_brush);
            } else {
                last_other = id.or(last_other);
            }
            if let Some(children) = node["children"].as_array() {
                stack.extend(children.iter().rev());
            }
        }
        shape.target = shape.target.or(first_node);
        shape.last_node = last_other.or(last_brush);
        shape
    }
}

fn is_brush(component: &Value) -> bool {
    component
        .as_str()
        .and_then(|path| path.rsplit("::").next())
        .is_some_and(|name| name == "Brush")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(entity: u64, id: u64, components: &[&str], children: Vec<Value>) -> Value {
        json!({ "entity": entity, "node": id, "components": components, "children": children })
    }

    #[test]
    fn counts_every_node_and_brush_at_any_depth() {
        let brush = "jackdaw_scene_types::types::Brush";
        let tree = json!({ "tree": [
            node(1, 10, &["bevy::Name"], vec![
                node(2, 20, &[brush], vec![]),
                node(3, 30, &["game::BrushSettings"], vec![node(4, 40, &[brush], vec![])]),
            ]),
            node(5, 50, &["bevy::Name"], vec![]),
        ]});
        let shape = SceneShape::of(&tree);
        assert_eq!(shape.nodes, 5);
        assert_eq!(shape.brushes, 2);
        assert_eq!(shape.target, Some(2), "the first brush");
        assert_eq!(
            shape.last_node,
            Some(50),
            "the last node that is not a brush"
        );
    }

    #[test]
    fn a_scene_without_brushes_targets_its_first_node() {
        let tree = json!({ "tree": [node(7, 70, &[], vec![]), node(8, 80, &[], vec![])] });
        let shape = SceneShape::of(&tree);
        assert_eq!(
            (shape.brushes, shape.target, shape.last_node),
            (0, Some(7), Some(80))
        );
    }

    #[test]
    fn an_empty_scene_has_nothing_to_act_on() {
        assert_eq!(
            SceneShape::of(&json!({ "tree": [] })),
            SceneShape::default()
        );
    }
}
