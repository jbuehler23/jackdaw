//! Tags on scene nodes: `entity.tag`, the command it undoes through, and the
//! chips the inspector header shows them as.

use bevy::ecs::system::SystemState;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_commands::EditorCommand;
use jackdaw_feathers::button::{ButtonOperatorCall, ButtonProps, ButtonVariant, button};
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::text_edit::{TextEditCommitEvent, TextEditProps, text_edit};
use jackdaw_feathers::tokens;
use jackdaw_scene_types::{PropertyValue, TAGS_TYPE_PATH, Tags};

use crate::EditorEntity;
use crate::commands::CommandHistory;
use crate::selection::{Selected, Selection};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<EntityTagOp>();
}

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(on_tags_mount_added)
        .add_observer(on_tags_removed)
        .add_observer(on_tag_field_commit)
        .add_systems(Update, refresh_tag_chips_on_change);
}

/// Add tags to entities or take them off, as one undo step.
#[operator(
    id = "entity.tag",
    label = "Tag Entities",
    description = "Add tags to entities or take them off, so a reference such as #roof names them all.",
    allows_undo = false,
    params(
        entities(
            String,
            doc = "Entities to tag: a list of ids, names, paths or queries such as #roof. \
                   Defaults to the selection."
        ),
        add(
            String,
            doc = "Tags to add, as a list or one comma-separated string. A tag is ASCII letters, \
                   digits, `_`, `-` and `.`."
        ),
        remove(
            String,
            doc = "Tags to take off, as a list or one comma-separated string."
        ),
    )
)]
pub(crate) fn entity_tag(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let changes = match read_tag(world, &params) {
        Ok(changes) => changes,
        Err(why) => {
            warn_caller(world, format!("entity.tag: {why}"));
            set_outcome(world, Outcome::Invalid);
            return OperatorResult::Cancelled;
        }
    };
    if changes.is_empty() {
        set_outcome(world, Outcome::Unchanged);
        return OperatorResult::Finished;
    }
    let mut command: Box<dyn EditorCommand> = Box::new(SetTags { changes });
    command.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(command);
    set_outcome(world, Outcome::Changed);
    OperatorResult::Finished
}

const TAG_KEYS: [&str; 3] = ["entities", "add", "remove"];

fn read_tag(world: &mut World, params: &OperatorParameters) -> Result<Vec<TagChange>, String> {
    if let Some(key) = params.keys().find(|key| !TAG_KEYS.contains(&key.as_str())) {
        return Err(format!("this takes no `{key}`"));
    }
    let add = tag_list(params, "add")?;
    let remove = tag_list(params, "remove")?;
    if add.is_empty() && remove.is_empty() {
        return Err("give `add`, `remove` or both".into());
    }
    if let Some(both) = add.iter().find(|tag| remove.contains(tag)) {
        return Err(format!("`{both}` is both added and removed"));
    }
    let mut state: SystemState<(Res<Selection>, Query<(), Without<EditorEntity>>)> =
        SystemState::new(world);
    let (selection, authored) = state.get(world).map_err(|err| err.to_string())?;
    let targets = crate::boot_ops::target_entities(params, &selection, &authored)?;
    if targets.is_empty() {
        return Err("nothing is selected and `entities` names nothing".into());
    }
    let mut changes = Vec::new();
    for entity in targets {
        crate::primitive_ops::scene_node(world, entity, "entities")?;
        let before = world.get::<Tags>(entity).cloned().unwrap_or_default();
        let mut after = before.clone();
        for tag in &add {
            after.insert(tag);
        }
        for tag in &remove {
            after.remove(tag);
        }
        if after != before {
            changes.push(TagChange {
                entity,
                before,
                after,
            });
        }
    }
    Ok(changes)
}

/// The tags a parameter names, each checked.
fn tag_list(params: &OperatorParameters, key: &str) -> Result<Vec<String>, String> {
    let text = match params.get(key) {
        None => return Ok(Vec::new()),
        Some(PropertyValue::String(text)) => text.to_string(),
        Some(PropertyValue::Int(number)) => number.to_string(),
        Some(other) => return Err(format!("`{key}` is a list of tags, not {other}")),
    };
    let tags: Vec<String> = crate::entity_refs::list_items(&text)
        .map_err(|why| format!("`{key}`: {why}"))?
        .into_iter()
        .map(|tag| tag.trim().to_string())
        .collect();
    if let Some(why) = tags.iter().find_map(|tag| Tags::refuse(tag)) {
        return Err(format!("`{key}`: {why}"));
    }
    Ok(tags)
}

struct TagChange {
    entity: Entity,
    before: Tags,
    after: Tags,
}

/// Undoable tag edit over any number of nodes.
struct SetTags {
    changes: Vec<TagChange>,
}

impl EditorCommand for SetTags {
    fn execute(&mut self, world: &mut World) {
        for change in &mut self.changes {
            change.entity = crate::scene_nodes::live_entity(world, change.entity);
            write_tags(world, change.entity, &change.after);
        }
    }

    fn undo(&mut self, world: &mut World) {
        for change in &mut self.changes {
            change.entity = crate::scene_nodes::live_entity(world, change.entity);
            write_tags(world, change.entity, &change.before);
        }
    }

    fn description(&self) -> &str {
        "Tag"
    }
}

/// Put `tags` on `entity` in the ECS and the document together. An empty set
/// takes the component off both, except on a node a prefab provides: there
/// the source's tags would come back on reload, so an empty set is written
/// over them instead.
fn write_tags(world: &mut World, entity: Entity, tags: &Tags) {
    let inherited = world.get::<crate::prefab::IsA>(entity).is_some()
        || world.get::<crate::prefab::PrefabEntityId>(entity).is_some();
    if tags.0.is_empty() && !inherited {
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.remove::<Tags>();
        }
        let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
        if let Some(node) = ast.ast_for(entity) {
            ast.remove_component_patch(node, TAGS_TYPE_PATH);
        }
        return;
    }
    if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
        entity_mut.insert(tags.clone());
    }
    crate::commands::sync_component_to_ast(world, entity, TAGS_TYPE_PATH, tags);
}

/// Where the inspector header shows the selected node's tags.
#[derive(Component)]
pub struct InspectorTagsMount;

/// The node and tags a mount's chips were put up for.
#[derive(Component, PartialEq)]
struct TagChipsShown(Option<(Entity, Vec<String>)>);

/// The add field under a mount, naming the node it tags.
#[derive(Component)]
pub struct TagAddField(pub Entity);

fn on_tags_mount_added(trigger: On<Add<InspectorTagsMount>>, mut commands: Commands) {
    commands.entity(trigger.event_target()).insert(Node {
        flex_direction: FlexDirection::Row,
        flex_wrap: FlexWrap::Wrap,
        align_items: AlignItems::Center,
        column_gap: Val::Px(tokens::SPACING_XS),
        row_gap: Val::Px(tokens::SPACING_XS),
        width: Val::Percent(100.0),
        display: Display::None,
        ..Default::default()
    });
    commands.queue(refresh_tag_chips);
}

/// Put the chips up again when the selection, or the tags of a selected node,
/// changed, including an edit made in place that no observer sees.
fn refresh_tag_chips_on_change(
    selection: Option<Res<Selection>>,
    changed: Query<(), (Changed<Tags>, With<Selected>)>,
    mut commands: Commands,
) {
    if selection.is_some_and(|selection| selection.is_changed()) || !changed.is_empty() {
        commands.queue(refresh_tag_chips);
    }
}

fn on_tags_removed(
    trigger: On<Remove<Tags>>,
    selected: Query<(), With<Selected>>,
    mut commands: Commands,
) {
    if selected.contains(trigger.event_target()) {
        commands.queue(refresh_tag_chips);
    }
}

/// The node whose tags the header shows: the primary selection, when it is a
/// node of the scene document.
fn tagged_node(world: &World) -> Option<(Entity, Vec<String>)> {
    let entity = world.get_resource::<Selection>()?.primary()?;
    crate::primitive_ops::scene_node(world, entity, "entity").ok()?;
    let tags = world
        .get::<Tags>(entity)
        .map(|tags| tags.0.clone())
        .unwrap_or_default();
    Some((entity, tags))
}

/// Put up every mount's chips again where the node or its tags changed.
fn refresh_tag_chips(world: &mut World) {
    let shown = TagChipsShown(tagged_node(world));
    let mounts: Vec<Entity> = world
        .query_filtered::<Entity, With<InspectorTagsMount>>()
        .iter(world)
        .collect();
    for mount in mounts {
        if world.get::<TagChipsShown>(mount) == Some(&shown) {
            continue;
        }
        let old: Vec<Entity> = world
            .get::<Children>(mount)
            .map(|children| children.iter().collect())
            .unwrap_or_default();
        for child in old {
            if let Ok(child) = world.get_entity_mut(child) {
                child.despawn();
            }
        }
        if let Some(mut node) = world.get_mut::<Node>(mount) {
            node.display = if shown.0.is_some() {
                Display::Flex
            } else {
                Display::None
            };
        }
        if let Some((entity, tags)) = &shown.0 {
            spawn_tag_chips(world, mount, *entity, tags);
        }
        world
            .entity_mut(mount)
            .insert(TagChipsShown(shown.0.clone()));
    }
}

fn spawn_tag_chips(world: &mut World, mount: Entity, entity: Entity, tags: &[String]) {
    let target = entity.to_bits().to_string();
    for tag in tags {
        world.spawn((
            button(
                ButtonProps::new(tag.clone())
                    .with_variant(ButtonVariant::Active)
                    .with_right_icon(Icon::X),
            ),
            ButtonOperatorCall::new(EntityTagOp::ID)
                .with_param("entities", target.clone())
                .with_param("remove", tag.clone()),
            ChildOf(mount),
        ));
    }
    world.spawn((
        Node {
            flex_grow: 1.0,
            min_width: Val::Px(80.0),
            ..Default::default()
        },
        TagAddField(entity),
        ChildOf(mount),
        children![text_edit(
            TextEditProps::default()
                .with_placeholder("Add tag...")
                .allow_empty()
        )],
    ));
}

/// Enter in the add field tags the node through `entity.tag`, so it lands on
/// the undo stack like any other edit.
fn on_tag_field_commit(
    commit: On<TextEditCommitEvent>,
    fields: Query<&TagAddField>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let Some(entity) = std::iter::once(commit.entity)
        .chain(parents.iter_ancestors(commit.entity))
        .find_map(|ancestor| fields.get(ancestor).ok())
        .map(|field| field.0)
    else {
        return;
    };
    let tags = commit.text.trim().to_string();
    if tags.is_empty() {
        return;
    }
    commands.queue(move |world: &mut World| {
        let result = world
            .operator(EntityTagOp::ID)
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: true,
            })
            .param("entities", entity.to_bits().to_string())
            .param("add", tags)
            .call();
        if let Err(err) = result {
            error!("entity.tag: {err}");
        }
    });
}
