//! Authoring the materials a placed model's parts wear in place of their own.

use std::collections::BTreeMap;

use bevy::gltf::GltfMaterialName;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::{GltfSource, InstanceMaterialOverrides, MaterialOverrides};

use crate::commands::EditorCommand;
use crate::selection::Selection;

const MODEL_OVERRIDES_TYPE_PATH: &str = "jackdaw_scene_types::types::MaterialOverrides";
const INSTANCE_OVERRIDES_TYPE_PATH: &str = "jackdaw_scene_types::types::InstanceMaterialOverrides";

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<MaterialOverrideOp>();
}

/// The placed model `entity` belongs to: itself, its nearest model ancestor, or the one model under it.
pub fn model_root(world: &World, entity: Entity) -> Option<Entity> {
    let mut at = entity;
    loop {
        if world.get::<GltfSource>(at).is_some() {
            return Some(at);
        }
        match world.get::<ChildOf>(at) {
            Some(parent) => at = parent.parent(),
            None => break,
        }
    }
    let mut models = Vec::new();
    let mut open = vec![entity];
    while let Some(at) = open.pop() {
        if world.get::<GltfSource>(at).is_some() {
            models.push(at);
            continue;
        }
        if let Some(children) = world.get::<Children>(at) {
            open.extend(children.iter());
        }
    }
    match models.as_slice() {
        [model] => Some(*model),
        _ => None,
    }
}

/// The glTF material names the parts under `root` wear, sorted and without repeats.
pub fn model_material_names(world: &World, root: Entity) -> Vec<String> {
    let mut names = Vec::new();
    let mut open = vec![root];
    while let Some(at) = open.pop() {
        if let Some(name) = world.get::<GltfMaterialName>(at) {
            names.push(name.0.clone());
        }
        if let Some(children) = world.get::<Children>(at) {
            open.extend(children.iter());
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Which component an override is written to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverrideLayer {
    /// The model's own [`MaterialOverrides`], which a prefab of it carries.
    Model,
    /// The enclosing prefab instance's [`InstanceMaterialOverrides`], laid over the model's.
    Instance,
}

impl OverrideLayer {
    fn type_path(self) -> &'static str {
        match self {
            Self::Model => MODEL_OVERRIDES_TYPE_PATH,
            Self::Instance => INSTANCE_OVERRIDES_TYPE_PATH,
        }
    }

    fn read(self, world: &World, entity: Entity) -> Option<BTreeMap<String, String>> {
        match self {
            Self::Model => world
                .get::<MaterialOverrides>(entity)
                .map(|overrides| overrides.materials.clone()),
            Self::Instance => world
                .get::<InstanceMaterialOverrides>(entity)
                .map(|overrides| overrides.materials.clone()),
        }
    }
}

/// The nearest prefab instance holding `entity`, itself included.
fn enclosing_instance(world: &World, entity: Entity) -> Option<Entity> {
    let mut at = Some(entity);
    while let Some(candidate) = at {
        if world.get::<crate::prefab::IsA>(candidate).is_some() {
            return Some(candidate);
        }
        at = world.get::<ChildOf>(candidate).map(ChildOf::parent);
    }
    None
}

/// One undo entry for a model's material overrides, or its prefab instance's.
pub struct SetMaterialOverrides {
    entity: Entity,
    layer: OverrideLayer,
    before: Option<BTreeMap<String, String>>,
    after: Option<BTreeMap<String, String>>,
}

impl SetMaterialOverrides {
    /// Set `names` on model `root` to `material` (clear when `None`), on its prefab instance if it is in one.
    pub fn new(world: &World, root: Entity, names: &[String], material: Option<&str>) -> Self {
        let (entity, layer) = match enclosing_instance(world, root) {
            Some(instance) => (instance, OverrideLayer::Instance),
            None => (root, OverrideLayer::Model),
        };
        let before = layer.read(world, entity);
        let mut materials = before.clone().unwrap_or_default();
        for name in names {
            match material {
                Some(path) => {
                    materials.insert(name.clone(), path.to_string());
                }
                None => {
                    materials.remove(name);
                }
            }
        }
        let after = (!materials.is_empty()).then_some(materials);
        Self {
            entity,
            layer,
            before,
            after,
        }
    }

    /// Whether running this would change nothing.
    pub fn is_noop(&self) -> bool {
        self.before == self.after
    }

    fn apply(&self, world: &mut World, materials: Option<&BTreeMap<String, String>>) {
        let Ok(mut node) = world.get_entity_mut(self.entity) else {
            return;
        };
        let Some(materials) = materials.cloned() else {
            match self.layer {
                OverrideLayer::Model => node.remove::<MaterialOverrides>(),
                OverrideLayer::Instance => node.remove::<InstanceMaterialOverrides>(),
            };
            if let Some(mut ast) = world.get_resource_mut::<jackdaw_bsn::SceneBsnAst>()
                && let Some(ast_node) = ast.ast_for(self.entity)
            {
                ast.remove_component_patch(ast_node, self.layer.type_path());
            }
            return;
        };
        match self.layer {
            OverrideLayer::Model => {
                let overrides = MaterialOverrides { materials };
                node.insert(overrides.clone());
                crate::commands::sync_component_to_ast(
                    world,
                    self.entity,
                    MODEL_OVERRIDES_TYPE_PATH,
                    &overrides,
                );
            }
            OverrideLayer::Instance => {
                let overrides = InstanceMaterialOverrides { materials };
                node.insert(overrides.clone());
                crate::commands::sync_component_to_ast(
                    world,
                    self.entity,
                    INSTANCE_OVERRIDES_TYPE_PATH,
                    &overrides,
                );
            }
        }
    }
}

impl EditorCommand for SetMaterialOverrides {
    fn execute(&mut self, world: &mut World) {
        let after = self.after.clone();
        self.apply(world, after.as_ref());
    }

    fn undo(&mut self, world: &mut World) {
        let before = self.before.clone();
        self.apply(world, before.as_ref());
    }

    fn description(&self) -> &str {
        "Material override"
    }
}

/// Set which material a placed model's parts wear in place of their own.
#[operator(
    id = "material.override",
    label = "Override Model Material",
    description = "Make a placed model's parts that wear one of its materials wear a material \
                   asset instead, saved with the scene.",
    allows_undo = false,
    params(
        entity(
            Entity,
            doc = "The placed model, or one of its parts. Defaults to the selected one."
        ),
        name(
            String,
            doc = "The model's material name to replace. Empty replaces every material the model wears."
        ),
        material(
            String,
            doc = "Path of the material asset to wear, e.g. materials/moss.bsn. Empty gives the \
                   model its own material back."
        ),
    )
)]
pub(crate) fn material_override(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let id = "material.override";
    let Some(target) = params
        .as_entity("entity")
        .or_else(|| world.resource::<Selection>().primary())
    else {
        warn_caller(world, format!("{id}: name a placed model or select one"));
        return OperatorResult::Cancelled;
    };
    let Some(root) = model_root(world, target) else {
        warn_caller(
            world,
            format!("{id}: {target} is not a placed model or one of its parts"),
        );
        return OperatorResult::Cancelled;
    };
    let names = match params.as_str("name").filter(|name| !name.is_empty()) {
        Some(name) => vec![name.to_string()],
        None if target != root => world
            .get::<GltfMaterialName>(target)
            .map(|name| vec![name.0.clone()])
            .unwrap_or_else(|| model_material_names(world, root)),
        None => model_material_names(world, root),
    };
    if names.is_empty() {
        warn_caller(
            world,
            format!("{id}: the model has no named materials yet; name one with name="),
        );
        return OperatorResult::Cancelled;
    }
    let material = params
        .as_str("material")
        .map(str::trim)
        .filter(|path| !path.is_empty());
    if let Some(path) = material
        && jackdaw_runtime::material_of_reference(world, path).is_none()
    {
        warn_caller(
            world,
            format!("{id}: {path} names no material this project holds"),
        );
        return OperatorResult::Cancelled;
    }

    let command = SetMaterialOverrides::new(world, root, &names, material);
    if command.is_noop() {
        return OperatorResult::Finished;
    }
    world.resource_scope(|world, mut history: Mut<CommandHistory>| {
        history.execute(Box::new(command), world);
    });
    OperatorResult::Finished
}
