//! Build stages for `jackdaw/batch`: a named group whose children one script
//! owns. Running the script again for the same stage deletes what the group
//! holds and builds it anew in the same undo entry, and refuses any call that
//! touches the document outside the group.

use std::any::TypeId;
use std::sync::Arc;

use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::remote::BrpError;
use jackdaw_commands::{CommandHistory, EditorCommand};
use jackdaw_scene_types::{BUILD_STAGE_TYPE_PATH, BuildStage, SceneNodeId};
use serde_json::{Value, json};

use super::server::{call_from_json, dispatch, invalid_params};
use crate::commands::AddComponent;
use crate::document_units::{UnitChange, UnitKey, Units};

/// The stage a batch builds, as the caller named it.
pub(super) struct StageSpec {
    pub(super) name: String,
    after: Vec<String>,
}

impl StageSpec {
    /// `stage` and `after` from a batch's parameters; `None` when no stage is
    /// named.
    pub(super) fn from_params(params: &Value) -> Result<Option<Self>, BrpError> {
        let after = match params.get("after") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(names)) => names
                .iter()
                .map(|name| {
                    name.as_str()
                        .filter(|name| !name.is_empty())
                        .map(str::to_string)
                        .ok_or_else(|| invalid_params("`after` lists stage names"))
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(invalid_params("`after` lists stage names")),
        };
        let name = match params.get("stage") {
            None | Some(Value::Null) => {
                if !after.is_empty() {
                    return Err(invalid_params("`after` needs a `stage`"));
                }
                return Ok(None);
            }
            Some(Value::String(name)) if !name.is_empty() => name.clone(),
            Some(_) => return Err(invalid_params("`stage` is a non-empty name")),
        };
        if after.contains(&name) {
            return Err(invalid_params(format!(
                "stage `{name}` cannot come after itself"
            )));
        }
        Ok(Some(Self { name, after }))
    }

    /// Refuse a stage whose name two groups share, or that comes after a
    /// stage the scene does not hold.
    pub(super) fn check(&self, world: &mut World) -> Result<(), BrpError> {
        find(world, &self.name)?;
        let missing: Vec<&str> = self
            .after
            .iter()
            .map(String::as_str)
            .filter(|name| !matches!(find(world, name), Ok(Some(_))))
            .collect();
        if !missing.is_empty() {
            return Err(invalid_params(format!(
                "stage `{}` comes after {}, which the scene does not hold; build {} first",
                self.name,
                quoted(&missing),
                if missing.len() == 1 { "it" } else { "them" }
            )));
        }
        Ok(())
    }
}

fn quoted(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The scene group holding stage `name`, if there is one.
fn find(world: &mut World, name: &str) -> Result<Option<Entity>, BrpError> {
    let mut stages = world.query_filtered::<(Entity, &Name), With<BuildStage>>();
    let found: Vec<Entity> = stages
        .iter(world)
        .filter(|(_, named)| named.as_str() == name)
        .map(|(entity, _)| entity)
        .collect();
    match found.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(*one)),
        _ => Err(invalid_params(format!(
            "{} groups are stage `{name}`; rename or delete all but one",
            found.len()
        ))),
    }
}

/// A stage being built: its group, emptied, and the document as the last
/// call left it.
pub(super) struct OpenStage {
    name: String,
    group: Entity,
    group_key: UnitKey,
    /// The nodes that already stood beside the group when the batch began;
    /// any other node a call leaves there moves into the group.
    beside: HashSet<u64>,
    units: Arc<Units>,
    replaced: usize,
    created: bool,
}

impl OpenStage {
    /// Find or add the stage's group, mark it, and delete what it holds.
    ///
    /// None of this is recorded as history of its own: the batch's scene
    /// change holds all of it.
    pub(super) fn open(world: &mut World, spec: &StageSpec) -> Result<Self, String> {
        let span = world.resource_mut::<CommandHistory>().begin_span();
        let opened = Self::open_unrecorded(world, spec);
        world.resource_mut::<CommandHistory>().take_span(span);
        opened
    }

    fn open_unrecorded(world: &mut World, spec: &StageSpec) -> Result<Self, String> {
        let (group, created) = match find(world, &spec.name).map_err(|err| err.message)? {
            Some(group) => (group, false),
            None => (add_group(world, &spec.name)?, true),
        };
        mark(world, group, &spec.after)?;

        let held: Vec<Entity> = {
            let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
            world
                .get::<Children>(group)
                .map(|children| {
                    children
                        .iter()
                        .filter(|child| ast.ast_for(*child).is_some())
                        .collect()
                })
                .unwrap_or_default()
        };
        crate::entity_ops::delete_entities(world, &held);

        let units = capture(world)?;
        let group_key = world
            .get::<SceneNodeId>(group)
            .map(|id| UnitKey::Node(id.0))
            .filter(|key| units.0.contains_key(key))
            .ok_or_else(|| format!("the group of stage `{}` is not in the document", spec.name))?;
        let beside = beside(world, group, &HashSet::new())
            .into_iter()
            .filter_map(|entity| world.get::<SceneNodeId>(entity).map(|id| id.0))
            .collect();
        Ok(Self {
            name: spec.name.clone(),
            group,
            group_key,
            beside,
            units,
            replaced: held.len(),
            created,
        })
    }

    /// Move the nodes a call added beside the group into it, then refuse the
    /// call if it changed the document anywhere outside the group.
    ///
    /// A change counts when any unit of the document differs: a node's own
    /// components, its parent or its place among its siblings, a prefab
    /// instance, or an embedded asset. A node must be inside the group both
    /// before and after; the group itself must not change; an embedded asset
    /// may be added but not changed or removed.
    pub(super) fn after_call(&mut self, world: &mut World) -> Result<(), String> {
        for entity in beside(world, self.group, &self.beside) {
            adopt(world, entity, self.group);
        }
        let now = capture(world)?;
        let outside: Vec<UnitKey> = self
            .units
            .changes_to(&now)
            .into_iter()
            .filter(|change| !self.allows(change, &now))
            .map(|change| change.key)
            .collect();
        if !outside.is_empty() {
            let by_id = entities_by_node_id(world);
            let named: Vec<String> = outside
                .iter()
                .map(|key| describe(world, &by_id, key))
                .collect();
            return Err(format!(
                "it changed {} outside stage `{}`; a stage only builds inside its own group",
                named.join(", "),
                self.name
            ));
        }
        self.units = now;
        Ok(())
    }

    fn allows(&self, change: &UnitChange, now: &Units) -> bool {
        match &change.key {
            UnitKey::Asset(_) => change.before.is_none(),
            key if *key == self.group_key => false,
            key => {
                (change.before.is_none() || inside(&self.units, key, &self.group_key))
                    && (change.after.is_none() || inside(now, key, &self.group_key))
            }
        }
    }

    /// What the batch result says about the stage.
    pub(super) fn report(&self) -> Value {
        json!({
            "name": self.name,
            "entity": self.group.to_bits(),
            "replaced": self.replaced,
            "created": self.created,
        })
    }
}

/// Whether `key` is `group` or sits somewhere under it in `units`.
fn inside(units: &Units, key: &UnitKey, group: &UnitKey) -> bool {
    let mut at = Some(key);
    for _ in 0..=units.0.len() {
        match at {
            Some(current) if current == group => return true,
            Some(current) => at = units.0.get(current).and_then(|unit| unit.parent.as_ref()),
            None => return false,
        }
    }
    false
}

fn capture(world: &mut World) -> Result<Arc<Units>, String> {
    let parent_path = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|root| root.root.clone())
        .unwrap_or_default();
    crate::document_capture::capture(world, &parent_path)
        .ok_or_else(|| "the scene document could not be compared before and after".to_string())
}

fn entities_by_node_id(world: &mut World) -> HashMap<u64, Entity> {
    let mut nodes = world.query::<(Entity, &SceneNodeId)>();
    nodes
        .iter(world)
        .map(|(entity, id)| (id.0, entity))
        .collect()
}

/// The scene nodes standing beside `group`, at the top level or under the
/// group's own parent, other than `known` and the group itself.
fn beside(world: &mut World, group: Entity, known: &HashSet<u64>) -> Vec<Entity> {
    let home = world.get::<ChildOf>(group).map(ChildOf::parent);
    let mut nodes = world.query::<(Entity, &SceneNodeId, Option<&ChildOf>)>();
    let found: Vec<(Entity, Option<Entity>)> = nodes
        .iter(world)
        .filter(|(entity, id, _)| *entity != group && !known.contains(&id.0))
        .map(|(entity, _, parent)| (entity, parent.map(ChildOf::parent)))
        .collect();
    found
        .into_iter()
        .filter(|(_, parent)| match parent {
            None => true,
            Some(parent) => Some(*parent) == home || world.get::<SceneNodeId>(*parent).is_none(),
        })
        .map(|(entity, _)| entity)
        .collect()
}

/// A unit as a person would name it: the node's name and id, or the asset's.
fn describe(world: &World, by_id: &HashMap<u64, Entity>, key: &UnitKey) -> String {
    match key {
        UnitKey::Asset(name) => format!("asset `{name}`"),
        UnitKey::Node(id) => {
            let entity = by_id.get(id).copied();
            match entity.and_then(|entity| world.get::<Name>(entity).map(|name| (entity, name))) {
                Some((entity, name)) => format!("`{name}` ({})", entity.to_bits()),
                None => match entity {
                    Some(entity) => format!("entity {}", entity.to_bits()),
                    None => format!("a node that is gone (node {id})"),
                },
            }
        }
    }
}

fn add_group(world: &mut World, name: &str) -> Result<Entity, String> {
    let call = call_from_json(
        world,
        &json!({ "id": "entity.add.group", "params": { "name": name } }),
    )
    .map_err(|err| err.message)?;
    let outcome = dispatch(world, call, false).map_err(|err| err.message)?;
    outcome
        .entities
        .first()
        .and_then(|&bits| Entity::try_from_bits(bits))
        .ok_or_else(|| format!("the group for stage `{name}` could not be added"))
}

/// Give `group` the stage marker, or bring its dependencies up to date.
fn mark(world: &mut World, group: Entity, after: &[String]) -> Result<(), String> {
    let current = world.get::<BuildStage>(group).cloned();
    if current.as_ref().is_some_and(|stage| stage.after == after) {
        return Ok(());
    }
    if current.is_some() {
        return crate::commands::try_field_edit_commit_on(
            world,
            group,
            BUILD_STAGE_TYPE_PATH,
            "after",
            &json!(after),
        )
        .map_err(|why| why.to_string());
    }
    let component_id = world.register_component::<BuildStage>();
    let mut add: Box<dyn EditorCommand> = Box::new(
        AddComponent::new(
            group,
            TypeId::of::<BuildStage>(),
            component_id,
            BUILD_STAGE_TYPE_PATH.to_string(),
        )
        .with_value(Box::new(BuildStage {
            after: after.to_vec(),
        })),
    );
    add.execute(world);
    match world.get::<BuildStage>(group) {
        Some(_) => Ok(()),
        None => Err("the stage marker could not be added to its group".to_string()),
    }
}

/// Move `entity` under `group`, keeping its local transform so the caller's
/// numbers read in the group's frame. The batch's own snapshot entry records
/// the move.
fn adopt(world: &mut World, entity: Entity, group: Entity) {
    crate::commands::place_entity(
        world,
        entity,
        crate::commands::HierarchyLocation {
            parent: Some(group),
            index: usize::MAX,
        },
        crate::commands::WorldTransform::Unplaced,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_units::Unit;

    fn unit(parent: Option<u64>) -> Unit {
        Unit {
            parent: parent.map(UnitKey::Node),
            follows: None,
            whole: false,
            text: "".into(),
        }
    }

    #[test]
    fn a_node_is_inside_a_group_through_any_number_of_parents() {
        let units = Units(
            [
                (UnitKey::Node(1), unit(None)),
                (UnitKey::Node(2), unit(Some(1))),
                (UnitKey::Node(3), unit(Some(2))),
                (UnitKey::Node(4), unit(None)),
            ]
            .into_iter()
            .collect(),
        );
        let group = UnitKey::Node(1);
        assert!(inside(&units, &UnitKey::Node(3), &group));
        assert!(inside(&units, &group, &group));
        assert!(!inside(&units, &UnitKey::Node(4), &group));
        assert!(!inside(&units, &UnitKey::Node(9), &group));
    }

    #[test]
    fn a_stage_names_itself_and_what_it_comes_after() {
        let spec = StageSpec::from_params(&json!({ "stage": "Inn", "after": ["Walls"] }))
            .expect("valid")
            .expect("a stage");
        assert_eq!(spec.name, "Inn");
        assert_eq!(spec.after, vec!["Walls".to_string()]);
        assert!(StageSpec::from_params(&json!({})).expect("valid").is_none());
        for refused in [
            json!({ "stage": "" }),
            json!({ "stage": 3 }),
            json!({ "after": ["Walls"] }),
            json!({ "stage": "Inn", "after": ["Inn"] }),
            json!({ "stage": "Inn", "after": "Walls" }),
        ] {
            assert!(StageSpec::from_params(&refused).is_err(), "{refused}");
        }
    }
}
