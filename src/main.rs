#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> bevy::app::AppExit {
    jackdaw::launch::run(|_| {})
}
