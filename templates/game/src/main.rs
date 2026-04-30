use bevy::prelude::*;
use {{crate_name}}::MyGamePlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(MyGamePlugin)
        .run();
}
