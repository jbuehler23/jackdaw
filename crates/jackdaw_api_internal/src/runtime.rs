//! Reserved for future runtime-side helpers.
//!
//! Previously housed `GameApp` / `GamePlugin` / `GameSystems` /
//! `GameRegistry` / `GameBookkeeping` — a `&mut World` adapter the
//! dylib loader used to install user plugins after the editor's
//! `App::run()` had been called.
//!
//! With the `SubApp` redesign (Phase 5), the user's plugin is a
//! vanilla `bevy::app::Plugin` installed against a `GameSubApp` at
//! editor startup. The post-run `&mut World` install path is gone,
//! so this file's contents are obsolete.
//!
//! Kept as an empty module to preserve the `crate::runtime::*` import
//! path consumers reach for. New runtime helpers (e.g. game-side
//! reflection helpers, PIE bookkeeping) can land here.
