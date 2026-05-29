//! Clickable Jackdaw brand icon that opens the repository in the system browser.

use bevy::asset::{embedded_asset, load_embedded_asset};
use bevy::feathers::cursor::EntityCursor;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::window::SystemCursorIcon;
use jackdaw_feathers::button::{ButtonClickEvent, ButtonSize, ButtonVariant, EditorButton};
use jackdaw_feathers::tokens::BORDER_RADIUS_MD;

use crate::EditorEntity;

pub const JACKDAW_REPO_URL: &str = "https://github.com/jbuehler23/jackdaw";

const BRAND_ICON_SIZE_PX: f32 = 18.0;

#[derive(Resource, Clone)]
pub struct JackdawBrandIcon(pub Handle<Image>);

#[derive(Component)]
struct JackdawRepoLinkButton;

pub struct RepoLinkPlugin;

impl Plugin for RepoLinkPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "../assets/jackdaw_icon_small.png");
        // Insert during plugin setup so `OnEnter` spawn systems (which run
        // before `Startup` on the first frame) can require this resource.
        let assets = app.world().resource::<AssetServer>();
        let handle = load_embedded_asset!(&*assets, "../assets/jackdaw_icon_small.png");
        app.insert_resource(JackdawBrandIcon(handle));
        app.add_observer(on_repo_link_click);
    }
}

/// Small square brand control for the window chrome row.
pub fn brand_link_button(image: Handle<Image>) -> impl Bundle {
    let variant = ButtonVariant::Ghost;
    (
        JackdawRepoLinkButton,
        EditorEntity,
        Button,
        EditorButton,
        variant,
        ButtonSize::IconSM,
        Hovered::default(),
        EntityCursor::System(SystemCursorIcon::Pointer),
        Node {
            width: ButtonSize::IconSM.width(),
            height: ButtonSize::IconSM.height(),
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
                width: Val::Px(BRAND_ICON_SIZE_PX),
                height: Val::Px(BRAND_ICON_SIZE_PX),
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
