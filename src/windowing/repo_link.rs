//! Clickable Jackdaw icon that opens the repository in the system browser.

use bevy::asset::{embedded_asset, load_embedded_asset};
use bevy::feathers::cursor::EntityCursor;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::window::SystemCursorIcon;
use jackdaw_feathers::button::{ButtonClickEvent, ButtonSize, ButtonVariant, EditorButton};
use jackdaw_feathers::tokens::{BORDER_RADIUS_MD, ICON_MD};

use crate::EditorEntity;

use super::chrome::WindowChromeStyle;
use super::native_hit_test::NativeHitTestClient;

const JACKDAW_REPO_URL: &str = "https://github.com/jbuehler23/jackdaw";

/// Leading repo link slot; `Node::margin.left` updated when the window fills the work area (macOS).
#[derive(Component)]
pub struct MacosHeaderLeadingInset;

#[derive(Resource, Clone)]
pub struct JackdawIcon(pub Handle<Image>);

#[derive(Component)]
struct JackdawRepoLinkButton;

pub struct RepoLinkPlugin;

impl Plugin for RepoLinkPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "../../assets/jackdaw_icon_small.png");
        let assets = app.world().resource::<AssetServer>();
        let handle = load_embedded_asset!(&*assets, "../../assets/jackdaw_icon_small.png");
        app.insert_resource(JackdawIcon(handle));
        app.add_observer(on_repo_link_click);
    }
}

/// Header row slot wrapping [`jackdaw_link_button`] with platform-appropriate leading inset and hit testing.
pub fn header_repo_link(image: Handle<Image>, chrome: WindowChromeStyle) -> impl Bundle {
    return (
        MacosHeaderLeadingInset,
        Pickable::IGNORE,
        NativeHitTestClient,
        Node {
            flex_shrink: 0.0,
            height: percent(100),
            align_items: AlignItems::Center,
            margin: UiRect {
                left: px(chrome.macos_traffic_light_inset()),
                ..default()
            },
            ..default()
        },
        children![jackdaw_link_button(image)],
    );
}

/// Button with icon to open the Jackdaw repository in the system browser.
pub fn jackdaw_link_button(image: Handle<Image>) -> impl Bundle {
    let variant = ButtonVariant::Ghost;
    (
        JackdawRepoLinkButton,
        EditorEntity,
        Button,
        EditorButton,
        variant,
        ButtonSize::Icon,
        Hovered::default(),
        EntityCursor::System(SystemCursorIcon::Pointer),
        Node {
            width: ButtonSize::Icon.width(),
            height: ButtonSize::Icon.height(),
            padding: UiRect::ZERO,
            border: UiRect::all(variant.border()),
            border_radius: BorderRadius::all(px(BORDER_RADIUS_MD)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundColor(
            variant
                .bg_color(false)
                .with_alpha(variant.bg_opacity(false))
                .into(),
        ),
        BorderColor::all(
            variant
                .border_color()
                .with_alpha(variant.border_opacity(false)),
        ),
        children![(
            ImageNode::new(image),
            Node {
                width: Val::Px(ICON_MD),
                height: Val::Px(ICON_MD),
                ..default()
            },
        )],
    )
}

fn on_repo_link_click(
    click: On<ButtonClickEvent>,
    buttons: Query<Entity, With<JackdawRepoLinkButton>>,
) {
    if buttons.get(click.entity).is_err() {
        return;
    }
    if let Err(error) = webbrowser::open(JACKDAW_REPO_URL) {
        bevy::log::warn!("jackdaw: failed to open repository URL: {error}");
    }
}
