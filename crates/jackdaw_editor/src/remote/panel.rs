use bevy::{prelude::*, ui_widgets::observe};
use jackdaw_feathers::{popover, tokens};

use super::connection::{ConnectionManager, ConnectionState};

/// Marker for the connection status indicator in the status bar.
#[derive(Component)]
pub struct ConnectionIndicator;

/// Marker for the colored dot in the status indicator.
#[derive(Component)]
pub struct ConnectionDot;

/// Marker for the text label in the status indicator.
#[derive(Component)]
pub struct ConnectionLabel;

/// Marker for the connection popover entity.
#[derive(Component)]
pub struct ConnectionPopover;

/// Marker for the endpoint text input in the popover.
#[derive(Component)]
pub struct EndpointInput;

/// Resource tracking the popover entity.
#[derive(Resource, Default)]
pub struct ConnectionPopoverState {
    pub entity: Option<Entity>,
}

/// Colors for the connection status dot.
const DOT_DISCONNECTED: Color = Color::srgba(0.5, 0.5, 0.5, 1.0);
const DOT_CONNECTING: Color = Color::srgba(1.0, 0.8, 0.0, 1.0);
const DOT_CONNECTED: Color = Color::srgba(0.2, 0.8, 0.3, 1.0);
const DOT_ERROR: Color = Color::srgba(0.9, 0.2, 0.2, 1.0);

/// Build the connection status indicator bundle for the status bar.
/// This is a small clickable area with a colored dot and label.
pub fn connection_indicator() -> impl Bundle {
    (
        ConnectionIndicator,
        Interaction::default(),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(tokens::SPACING_XS),
            padding: UiRect::horizontal(Val::Px(tokens::SPACING_SM)),
            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
            ..Default::default()
        },
        children![
            // Colored dot
            (
                ConnectionDot,
                Node {
                    width: Val::Px(8.0),
                    height: Val::Px(8.0),
                    border_radius: BorderRadius::all(Val::Px(4.0)),
                    ..Default::default()
                },
                BackgroundColor(DOT_DISCONNECTED),
            ),
            // Label
            (
                ConnectionLabel,
                Text::new("Disconnected"),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            )
        ],
    )
}

/// Update the connection indicator dot color and label based on current state.
pub fn update_connection_status_indicator(
    manager: Res<ConnectionManager>,
    mut dots: Query<&mut BackgroundColor, With<ConnectionDot>>,
    mut labels: Query<&mut Text, With<ConnectionLabel>>,
) {
    if !manager.is_changed() {
        return;
    }

    let (color, label) = match &manager.state {
        ConnectionState::Disconnected => (DOT_DISCONNECTED, "Disconnected".to_string()),
        ConnectionState::Connecting => (DOT_CONNECTING, "Connecting...".to_string()),
        ConnectionState::Connected { app_info } => {
            (DOT_CONNECTED, format!("Connected: {}", app_info.app_name))
        }
        ConnectionState::Error(msg) => {
            // Truncate error message for status bar display
            let short = if msg.len() > 30 {
                format!("{}...", &msg[..30])
            } else {
                msg.clone()
            };
            (DOT_ERROR, format!("Error: {short}"))
        }
    };

    for mut bg in &mut dots {
        bg.0 = color;
    }
    for mut text in &mut labels {
        text.0 = label.clone();
    }
}

/// Handle clicks on the connection indicator. Toggles connection popover.
pub fn on_connection_indicator_click(
    trigger: On<Pointer<Click>>,
    indicators: Query<(), With<ConnectionIndicator>>,
    mut commands: Commands,
    manager: Res<ConnectionManager>,
    mut popover_state: Local<Option<Entity>>,
) {
    let entity = trigger.event_target();
    if indicators.get(entity).is_err() {
        return;
    }

    // Toggle: if popover exists, despawn it
    if let Some(popover_entity) = popover_state.take() {
        if let Ok(mut ec) = commands.get_entity(popover_entity) {
            ec.despawn();
        }
        return;
    }

    let is_connected = manager.is_connected();
    let endpoint = manager.endpoint.clone();

    let popover_entity = commands
        .spawn(popover::popover(
            popover::PopoverProps::new(entity)
                .with_placement(popover::PopoverPlacement::TopStart)
                .with_padding(8.0)
                .with_z_index(200),
        ))
        .with_children(|parent| {
            parent
                .spawn(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(tokens::SPACING_SM),
                    padding: Val::Px(tokens::SPACING_SM).into(),
                    min_width: Val::Px(260.0),
                    ..Default::default()
                })
                .with_children(|col| {
                    // Title
                    col.spawn((
                        Text::new("Remote Connection"),
                        TextFont {
                            font_size: tokens::FONT_LG,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_PRIMARY),
                    ));

                    // Endpoint display
                    col.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(tokens::SPACING_SM),
                        align_items: AlignItems::Center,
                        ..Default::default()
                    })
                    .with_children(|row| {
                        row.spawn((
                            Text::new("Endpoint:"),
                            TextFont {
                                font_size: tokens::FONT_SM,
                                ..Default::default()
                            },
                            TextColor(tokens::TEXT_SECONDARY),
                        ));
                        row.spawn((
                            Text::new(endpoint),
                            TextFont {
                                font_size: tokens::FONT_SM,
                                ..Default::default()
                            },
                            TextColor(tokens::TEXT_PRIMARY),
                        ));
                    });

                    // Connect / Disconnect button
                    let (button_text, button_color) = if is_connected {
                        ("Disconnect", Color::srgba(0.7, 0.2, 0.2, 1.0))
                    } else {
                        ("Connect", Color::srgba(0.2, 0.5, 0.8, 1.0))
                    };

                    col.spawn((
                        ConnectButton,
                        Interaction::default(),
                        Node {
                            padding: UiRect::axes(
                                Val::Px(tokens::SPACING_LG),
                                Val::Px(tokens::SPACING_SM),
                            ),
                            border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
                            justify_content: JustifyContent::Center,
                            ..Default::default()
                        },
                        BackgroundColor(button_color),
                        children![(
                            Text::new(button_text),
                            TextFont {
                                font_size: tokens::FONT_SM,
                                ..Default::default()
                            },
                            TextColor(tokens::TEXT_PRIMARY),
                        )],
                        observe(
                            move |_: On<Pointer<Click>>,
                                  mut commands: Commands,
                                  mut manager: ResMut<ConnectionManager>| {
                                if manager.is_connected() {
                                    super::connection::disconnect(&mut commands, &mut manager);
                                } else {
                                    manager.state = ConnectionState::Connecting;
                                    super::connection::start_connect(
                                        &mut commands,
                                        &manager.endpoint,
                                    );
                                }
                            },
                        ),
                    ));

                    // Refresh Registry button (only when connected)
                    if is_connected {
                        col.spawn((
                            Interaction::default(),
                            Node {
                                padding: UiRect::axes(
                                    Val::Px(tokens::SPACING_LG),
                                    Val::Px(tokens::SPACING_SM),
                                ),
                                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
                                justify_content: JustifyContent::Center,
                                ..Default::default()
                            },
                            BackgroundColor(Color::srgba(0.3, 0.3, 0.5, 1.0)),
                            children![(
                                Text::new("Refresh Registry"),
                                TextFont {
                                    font_size: tokens::FONT_SM,
                                    ..Default::default()
                                },
                                TextColor(tokens::TEXT_PRIMARY),
                            )],
                            observe(
                                |_: On<Pointer<Click>>,
                                 mut commands: Commands,
                                 manager: Res<ConnectionManager>| {
                                    super::registry_fetch::start_registry_fetch(
                                        &mut commands,
                                        &manager.endpoint,
                                    );
                                },
                            ),
                        ));
                    }
                });
        })
        .id();

    *popover_state = Some(popover_entity);
}

/// Marker for the connect/disconnect button.
#[derive(Component)]
struct ConnectButton;
