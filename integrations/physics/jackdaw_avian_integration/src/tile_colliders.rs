//! Static 2D colliders under every tilemap, built from the collision shapes
//! its tiles carry: one static body per chunk of each layer, with one
//! compound collider per physics layer and kind.
//!
//! Physics layers map to collision layer bits by index: the colliders on
//! physics layer `n` are members of `LayerMask(1 << n)` and filter nothing.
//! A physics layer past the 32 collision layers builds nothing, with one
//! warning per map layer. Triggers are [`Sensor`]s, and one-way shapes carry
//! [`OneWayTile`].

use std::collections::{HashMap, HashSet};

use avian2d::prelude::*;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use jackdaw_runtime::{RuntimeOnly, TilemapChunkCollision, TilemapCollision};
use jackdaw_scene_types::{SceneDerived, Tilemap};
use jackdaw_tilemap::{ChunkCoord, CollisionGroup};

pub(crate) fn plugin(app: &mut App) {
    app.register_required_components::<Tilemap, TilemapCollision>()
        .add_systems(
            PostUpdate,
            build_tile_colliders.before(TransformSystems::Propagate),
        )
        .add_observer(drop_collision_with_tilemap)
        .add_observer(despawn_chunk_bodies);
}

/// The static body of one chunk of one tilemap layer, a child of its map,
/// with a child collider per physics layer and kind.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileChunkBody {
    /// Index into the map's layers.
    pub layer: usize,
    pub chunk: ChunkCoord,
    /// The revision of the map's collision entry it was built from.
    pub revision: u64,
}

/// A tile collider that should block only bodies coming down onto it from
/// above. Avian has no one-way collision of its own, so a game filters the
/// contacts of these colliders in its collision hooks.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OneWayTile;

/// The map layers and physics layers already warned about having no
/// collision layer.
type Warned = HashSet<(Entity, usize, u32)>;

/// Rebuild the chunk bodies of every map whose collision changed, keeping
/// each body whose chunk did not.
fn build_tile_colliders(
    mut commands: Commands,
    maps: Query<(Entity, &TilemapCollision), Changed<TilemapCollision>>,
    bodies: Query<(Entity, &TileChunkBody, &ChildOf)>,
    mut warned: Local<Warned>,
) -> Result {
    for (map, collision) in &maps {
        let mut standing: HashMap<(usize, ChunkCoord), (Entity, u64)> = (bodies.iter())
            .filter(|(_, _, parent)| parent.parent() == map)
            .map(|(entity, body, _)| ((body.layer, body.chunk), (entity, body.revision)))
            .collect();
        for (&(layer, chunk), entry) in &collision.chunks {
            if let Some((body, revision)) = standing.remove(&(layer, chunk)) {
                if revision == entry.revision {
                    continue;
                }
                commands.get_entity(body)?.despawn();
            }
            spawn_chunk_body(&mut commands, map, layer, chunk, entry, &mut warned);
        }
        for (body, _) in standing.into_values() {
            commands.get_entity(body)?.despawn();
        }
    }
    Ok(())
}

/// A map that stops being a tilemap stops colliding.
fn drop_collision_with_tilemap(removed: On<Remove<Tilemap>>, mut commands: Commands) {
    if let Ok(mut map) = commands.get_entity(removed.event_target()) {
        map.try_remove::<TilemapCollision>();
    }
}

fn despawn_chunk_bodies(
    removed: On<Remove<TilemapCollision>>,
    bodies: Query<(Entity, &ChildOf), With<TileChunkBody>>,
    mut commands: Commands,
) {
    let map = removed.event_target();
    for (body, parent) in &bodies {
        if parent.parent() == map {
            commands.entity(body).try_despawn();
        }
    }
}

fn spawn_chunk_body(
    commands: &mut Commands,
    map: Entity,
    layer: usize,
    chunk: ChunkCoord,
    entry: &TilemapChunkCollision,
    warned: &mut Warned,
) {
    let body = commands
        .spawn((
            TileChunkBody {
                layer,
                chunk,
                revision: entry.revision,
            },
            RigidBody::Static,
            Transform::from_translation(entry.origin.extend(0.0)),
            ChildOf(map),
            SceneDerived,
            RuntimeOnly,
        ))
        .id();
    for group in &entry.groups {
        let Some(member) = 1u32.checked_shl(group.layer) else {
            if warned.insert((map, layer, group.layer)) {
                warn!(
                    "tile collision on physics layer {} has no collision layer; only 32 exist",
                    group.layer
                );
            }
            continue;
        };
        let Some(collider) = group_collider(group, entry.tile_size) else {
            continue;
        };
        let mut part = commands.spawn((
            collider,
            CollisionLayers::new(LayerMask(member), LayerMask::ALL),
            Transform::default(),
            ChildOf(body),
            SceneDerived,
            RuntimeOnly,
        ));
        if group.trigger {
            part.insert(Sensor);
        }
        if group.one_way {
            part.insert(OneWayTile);
        }
    }
}

/// A group's boxes and polygons in world units as one compound collider;
/// `None` when the group holds nothing.
fn group_collider(group: &CollisionGroup, tile_size: Vec2) -> Option<Collider> {
    let mut parts: Vec<(Position, Rotation, Collider)> = Vec::new();
    for [min, max] in &group.boxes {
        let (min, max) = (*min * tile_size, *max * tile_size);
        let size = max - min;
        parts.push((
            Position((min + max) / 2.0),
            Rotation::IDENTITY,
            Collider::rectangle(size.x, size.y),
        ));
    }
    for polygon in &group.polygons {
        let points: Vec<Vec2> = polygon.iter().map(|&p| p * tile_size).collect();
        let Some(collider) = Collider::convex_polyline(points.clone()) else {
            warn!("a tile collision polygon collides nowhere, as it has no area: {points:?}");
            continue;
        };
        parts.push((Position(Vec2::ZERO), Rotation::IDENTITY, collider));
    }
    (!parts.is_empty()).then(|| Collider::compound(parts))
}
