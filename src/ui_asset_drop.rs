//! Dropping an image from the asset browser onto the 2D canvas.
//!
//! Three landings, decided by what is under the cursor when the drag is
//! released:
//!
//! - an `ImageNode` takes the texture, keeping the node that was already
//!   laid out where the author put it;
//! - a container takes a new image as its last child, laid out by whatever
//!   the container does;
//! - bare canvas -- the scene's own root -- takes a new image placed
//!   absolutely at the point the drop landed on.
//!
//! The new image is made through the widget palette, so a dropped image is
//! the same node the Add menu builds and carries the same name, and its
//! placement and texture go in the same history entry rather than following
//! it as a second.

use bevy::{asset::AssetServer, prelude::*};

use crate::{
    EditorEntity,
    commands::{CommandHistory, EditorCommand, sync_component_to_ast},
};

/// The palette definition a dropped image becomes.
const IMAGE_WIDGET: &str = "ui.image";

/// Undo label a drop that made a node lands under.
const DROP_LABEL: &str = "Drop image on canvas";

/// Set one `ImageNode`'s texture, in the ECS and in the document together.
struct SetImageTexture {
    entity: Entity,
    before: Handle<Image>,
    after: Handle<Image>,
}

impl SetImageTexture {
    fn write(&self, world: &mut World, texture: Handle<Image>) {
        let Some(mut image) = world.get_mut::<ImageNode>(self.entity) else {
            return;
        };
        image.image = texture;
        let value = image.clone();
        sync_component_to_ast(world, self.entity, ImageNode::type_path(), &value);
    }
}

impl EditorCommand for SetImageTexture {
    fn execute(&mut self, world: &mut World) {
        self.write(world, self.after.clone());
    }

    fn undo(&mut self, world: &mut World) {
        self.write(world, self.before.clone());
    }

    fn description(&self) -> &str {
        "Set image texture"
    }
}

/// Where a drop landed, and what it means.
pub enum ImageDrop {
    /// Onto a node that already draws an image.
    Texture(Entity),
    /// Into a container, as its last child.
    Inside(Entity),
    /// Onto the canvas itself, at a point in authored pixels.
    Canvas(Vec2),
}

/// What a drop at `target` means, given the node under the cursor.
///
/// `under` is the authored node the cursor is over, or `None` for a miss;
/// a hit on the scene's own root is the canvas rather than a container,
/// because the root is what the canvas is.
pub fn classify_drop(world: &World, under: Option<Entity>, at: Vec2) -> ImageDrop {
    match under {
        Some(entity) if world.get::<ImageNode>(entity).is_some() => ImageDrop::Texture(entity),
        Some(entity) if !is_scene_root(world, entity) && world.get::<Node>(entity).is_some() => {
            ImageDrop::Inside(entity)
        }
        _ => ImageDrop::Canvas(at),
    }
}

fn is_scene_root(world: &World, entity: Entity) -> bool {
    world
        .get::<jackdaw_scene_types::UiSceneRoot>(entity)
        .is_some()
        || world
            .get::<jackdaw_scene_types::Scene2dRoot>(entity)
            .is_some()
}

/// Land a dropped image, and say which node it ended up on.
///
/// The browser hands over the file's own path, which is absolute. A document
/// records a texture as the asset path the handle carries, so loading the
/// absolute path would write a path that resolves on this machine and nowhere
/// else -- and under Bevy's approved-path rule does not resolve at all, leaving
/// the saved document with an empty image. [`crate::entity_ops::to_asset_path`]
/// is what reduces it to the project-relative path both the load and the save
/// need.
pub fn drop_image(world: &mut World, path: &str, landing: ImageDrop) -> Option<Entity> {
    let asset_path = crate::entity_ops::to_asset_path(path);
    let texture: Handle<Image> = world.resource::<AssetServer>().load(asset_path);
    match landing {
        ImageDrop::Texture(entity) => {
            set_texture(world, entity, texture);
            Some(entity)
        }
        ImageDrop::Inside(parent) => spawn_image(world, Some(parent), None, texture),
        ImageDrop::Canvas(at) => spawn_image(world, None, Some(at), texture),
    }
}

/// One drop that made a node: the node, where it landed, and what it draws.
///
/// Making the node is the palette's own command, kept here so undo takes it
/// back. The placement and the texture are not commands beside it: the palette
/// respawns the node on redo, so anything addressing the old entity would write
/// to a node that no longer exists and put the image back at the palette's
/// default position with no texture on it. Doing all three inside one
/// `execute` is what makes a redo land where the drop did.
///
/// Nothing here touches [`CommandHistory`]: a redo runs `execute` with the
/// history taken out of the world.
struct DropImage {
    parent: Option<Entity>,
    at: Option<Vec2>,
    texture: Handle<Image>,
    /// The palette's entry for the node this drop made.
    made: Option<Box<dyn EditorCommand>>,
    /// The node the last `execute` made, for the caller to select.
    spawned: Option<Entity>,
}

impl EditorCommand for DropImage {
    fn execute(&mut self, world: &mut World) {
        self.spawned = None;
        let Ok((spawned, made)) =
            crate::ui_palette::instantiate_widget_command_under(world, IMAGE_WIDGET, self.parent)
        else {
            return;
        };
        self.made = Some(made);

        if let Some(at) = self.at
            && let Some(mut node) = world.get_mut::<Node>(spawned)
        {
            node.position_type = PositionType::Absolute;
            node.left = px(at.x);
            node.top = px(at.y);
            let value = node.clone();
            sync_component_to_ast(
                world,
                spawned,
                crate::inspector::node_card::node_type_path(),
                &value,
            );
        }
        if let Some(mut image) = world.get_mut::<ImageNode>(spawned) {
            image.image = self.texture.clone();
            let value = image.clone();
            sync_component_to_ast(world, spawned, ImageNode::type_path(), &value);
        }
        self.spawned = Some(spawned);
    }

    fn undo(&mut self, world: &mut World) {
        if let Some(mut made) = self.made.take() {
            made.undo(world);
        }
        self.spawned = None;
    }

    fn description(&self) -> &str {
        DROP_LABEL
    }
}

/// Give `entity` a new texture as one history entry.
fn set_texture(world: &mut World, entity: Entity, texture: Handle<Image>) {
    let Some(before) = world
        .get::<ImageNode>(entity)
        .map(|image| image.image.clone())
    else {
        return;
    };
    if before == texture {
        return;
    }
    let mut command = SetImageTexture {
        entity,
        before,
        after: texture,
    };
    command.execute(world);
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(command));
}

/// Make a new image node, place it, and give it its texture, as one entry.
///
/// A drop that made no node records nothing: there is nothing to undo, and no
/// half-made node is left outside history for an undo to miss.
fn spawn_image(
    world: &mut World,
    parent: Option<Entity>,
    at: Option<Vec2>,
    texture: Handle<Image>,
) -> Option<Entity> {
    let mut command = DropImage {
        parent,
        at,
        texture,
        made: None,
        spawned: None,
    };
    command.execute(world);
    let spawned = command.spawned?;
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(command));
    Some(spawned)
}

/// Whether `entity` is an authored node a drop may land on.
pub fn is_authored(world: &World, entity: Entity) -> bool {
    world.get::<EditorEntity>(entity).is_none() && world.get::<Node>(entity).is_some()
}
