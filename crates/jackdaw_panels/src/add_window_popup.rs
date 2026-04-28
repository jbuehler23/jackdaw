use bevy::prelude::*;
use bevy::ui::UiGlobalTransform;
use jackdaw_feathers::tokens;

use crate::{
    DockTabBar, DockTabContent, DockWindow, WindowRegistry,
    reconcile::LeafBinding,
    tabs::{DockTabAddButton, DockTabRow},
    tree::{DockNode, DockTree},
};

#[derive(Component)]
pub struct AddWindowPopup {
    pub area_entity: Entity,
}

#[derive(Component)]
pub struct AddWindowPopupItem {
    pub window_id: String,
    pub area_entity: Entity,
}

#[derive(Component)]
pub struct AddWindowPopupBackdrop;

pub struct AddWindowPopupPlugin;

impl Plugin for AddWindowPopupPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_add_button_click)
            .add_observer(on_item_click)
            .add_observer(on_backdrop_click)
            .add_systems(Update, hover_popup_items);
    }
}

fn on_add_button_click(
    trigger: On<Pointer<Click>>,
    buttons: Query<(&DockTabAddButton, &UiGlobalTransform, &ComputedNode)>,
    existing_popups: Query<Entity, With<AddWindowPopup>>,
    registry: Res<WindowRegistry>,
    existing_windows: Query<(&DockTabContent, &ChildOf)>,
    mut commands: Commands,
) {
    let entity = trigger.event_target();
    let Ok((button, global_transform, computed)) = buttons.get(entity) else {
        return;
    };

    for popup in &existing_popups {
        commands.entity(popup).despawn();
    }

    let area_entity = button.area_entity;

    let already_in_area: Vec<String> = existing_windows
        .iter()
        .filter(|(_, co)| co.parent() == area_entity)
        .map(|(content, _)| content.window_id.clone())
        .collect();

    let available: Vec<(String, String)> = registry
        .iter()
        .filter(|w| !already_in_area.contains(&w.id))
        .map(|w| (w.id.clone(), w.name.clone()))
        .collect();

    if available.is_empty() {
        return;
    }

    let (_scale, _angle, center) = global_transform.to_scale_angle_translation();
    let size = computed.size() * computed.inverse_scale_factor();
    let pos = center;

    commands.spawn((
        AddWindowPopupBackdrop,
        Interaction::default(),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        GlobalZIndex(250),
        BackgroundColor(Color::NONE),
    ));

    let popup = commands
        .spawn((
            AddWindowPopup { area_entity },
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(pos.x - 100.0),
                top: Val::Px(pos.y + size.y / 2.0 + 4.0),
                min_width: Val::Px(160.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(tokens::MENU_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            GlobalZIndex(300),
        ))
        .id();

    for (window_id, name) in &available {
        commands.spawn((
            AddWindowPopupItem {
                window_id: window_id.clone(),
                area_entity,
            },
            Interaction::default(),
            Node {
                padding: UiRect::axes(Val::Px(10.0), Val::Px(5.0)),
                border_radius: BorderRadius::all(Val::Px(3.0)),
                ..default()
            },
            BackgroundColor(Color::NONE),
            ChildOf(popup),
            children![(
                Text::new(name.clone()),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            )],
        ));
    }
}

fn on_item_click(
    trigger: On<Pointer<Click>>,
    items: Query<&AddWindowPopupItem>,
    popups: Query<Entity, With<AddWindowPopup>>,
    backdrops: Query<Entity, With<AddWindowPopupBackdrop>>,
    mut commands: Commands,
) {
    let entity = trigger.event_target();
    let Ok(item) = items.get(entity) else { return };

    let window_id = item.window_id.clone();
    let area_entity = item.area_entity;

    for popup in &popups {
        commands.entity(popup).despawn();
    }
    for backdrop in &backdrops {
        commands.entity(backdrop).despawn();
    }

    commands.queue(move |world: &mut World| {
        add_window_to_area(world, &window_id, area_entity);
    });
}

fn on_backdrop_click(
    trigger: On<Pointer<Click>>,
    backdrops: Query<(), With<AddWindowPopupBackdrop>>,
    popups: Query<Entity, With<AddWindowPopup>>,
    backdrop_entities: Query<Entity, With<AddWindowPopupBackdrop>>,
    mut commands: Commands,
) {
    if backdrops.get(trigger.event_target()).is_err() {
        return;
    }
    for popup in &popups {
        commands.entity(popup).despawn();
    }
    for backdrop in &backdrop_entities {
        commands.entity(backdrop).despawn();
    }
}

fn hover_popup_items(
    mut items: Query<
        (&Interaction, &mut BackgroundColor),
        (Changed<Interaction>, With<AddWindowPopupItem>),
    >,
) {
    for (interaction, mut bg) in &mut items {
        bg.0 = match interaction {
            Interaction::Hovered => tokens::HOVER_BG,
            _ => Color::NONE,
        };
    }
}

fn add_window_to_area(world: &mut World, window_id: &str, area_entity: Entity) {
    let (name, build) = {
        let registry = world.resource::<WindowRegistry>();
        let Some(descriptor) = registry.get(window_id) else {
            return;
        };
        (descriptor.name.clone(), descriptor.build.clone())
    };

    let Some(binding) = world.entity(area_entity).get::<LeafBinding>().copied() else {
        return;
    };

    {
        let mut tree = world.resource_mut::<DockTree>();
        if let Some(DockNode::Leaf(leaf)) = tree.get_mut(binding.0) {
            if !leaf.windows.iter().any(|w| w == window_id) {
                leaf.windows.push(window_id.to_string());
            }
            leaf.active = Some(window_id.to_string());
        } else {
            return;
        }
    }

    // Walk: area → DockTabBar → DockTabRow.
    let tab_row = world
        .entity(area_entity)
        .get::<Children>()
        .and_then(|children| {
            children
                .iter()
                .find(|&e| world.entity(e).contains::<DockTabBar>())
        })
        .and_then(|tab_bar| {
            world
                .entity(tab_bar)
                .get::<Children>()
                .and_then(|c| c.iter().find(|&e| world.entity(e).contains::<DockTabRow>()))
        });

    if let Some(tab_row) = tab_row {
        crate::tabs::spawn_tab_in_world(world, tab_row, window_id, &name, false);
    }

    let content_entity = world
        .spawn((
            DockWindow {
                descriptor_id: window_id.to_string(),
            },
            DockTabContent {
                window_id: window_id.to_string(),
            },
            Node {
                flex_grow: 1.0,
                width: Val::Percent(100.0),
                min_height: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                overflow: Overflow::clip(),
                display: Display::None,
                ..default()
            },
            ChildOf(area_entity),
        ))
        .id();

    (build)(&mut ChildSpawner::new(world, content_entity));
}
