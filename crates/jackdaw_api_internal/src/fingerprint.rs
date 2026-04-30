//! Layout fingerprint computation for the FFI boundary.
//!
//! Both the editor binary and the user's cdylib emit a fingerprint at
//! compile time, derived from `(size_of, align_of)` of every Bevy
//! type that crosses the dlopen boundary. The loader compares the
//! cdylib's fingerprint to the editor's at dlopen time; mismatch
//! refuses the swap with a clear "restart needed" message.
//!
//! Cargo's content-addressed build cache normally prevents Bevy from
//! being recompiled when the user rebuilds gameplay code, so the
//! editor and the cdylib share identical layouts in the common case.
//! In the rare case where shared deps DO change (Bevy version bump,
//! `rust-toolchain.toml` change, feature flag flip on a transitive
//! dep), this fingerprint detects the layout drift and refuses the
//! dlopen rather than risking UB on the first vtable call.
//!
//! # Where this lives
//!
//! Defined in `jackdaw_api_internal` (rather than `jackdaw_runtime`)
//! because the [`export_game_plugin!`](crate::export_game_plugin)
//! macro lives here and emits the cdylib-side fingerprint export via
//! `$crate::fingerprint::LAYOUT_FINGERPRINT`. Editor-side code
//! reaches the same constant through the
//! `jackdaw_runtime::LAYOUT_FINGERPRINT` re-export so
//! `jackdaw_loader` does not need a direct dep on
//! `jackdaw_api_internal`'s implementation details.
//!
//! See the "ABI stability is empirical, not formal" section of the
//! launcher architecture spec at
//! `docs/superpowers/specs/2026-04-30-jackdaw-launcher-architecture-design.md`.

use bevy::ecs::reflect::AppTypeRegistry;
use bevy::ecs::schedule::Schedules;
use bevy::ecs::world::World;
use bevy::prelude::*;

/// FNV-1a-ish const hash combiner. Folds `size_of` and `align_of` of
/// `T` into a 64-bit value. The exact hash function is irrelevant;
/// only equality matters because the loader compares fingerprints
/// with `==`, not range or ordering.
const fn hash_layout<T>() -> u64 {
    let size = core::mem::size_of::<T>() as u64;
    let align = core::mem::align_of::<T>() as u64;
    let mut h: u64 = 0xcbf29ce484222325;
    h = h.wrapping_mul(0x100000001b3) ^ size;
    h = h.wrapping_mul(0x100000001b3) ^ align;
    h
}

/// Compute the editor / cdylib layout fingerprint at compile time.
/// Combines layout of every Bevy type that crosses the FFI boundary.
/// If two compilations produce the same fingerprint, they have
/// identical layouts for all boundary types and dlopen swap is safe.
///
/// The set of types is intentionally a superset of what's strictly
/// needed: any Bevy struct whose layout differing would cause vtable
/// drift or UB on a shared resource is fair game. Adding more types
/// is monotonically safer (false negatives only); removing types
/// risks false positives. Expand as new boundary types appear.
const fn compute_fingerprint() -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    h = h.wrapping_mul(0x100000001b3) ^ hash_layout::<App>();
    h = h.wrapping_mul(0x100000001b3) ^ hash_layout::<World>();
    h = h.wrapping_mul(0x100000001b3) ^ hash_layout::<Schedules>();
    h = h.wrapping_mul(0x100000001b3) ^ hash_layout::<AppTypeRegistry>();
    h
}

/// Layout fingerprint constant. Both the editor binary and the
/// cdylib emit this at compile time; the loader compares them at
/// dlopen.
pub const LAYOUT_FINGERPRINT: u64 = compute_fingerprint();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_nonzero() {
        assert_ne!(LAYOUT_FINGERPRINT, 0);
    }

    #[test]
    fn fingerprint_is_deterministic() {
        const FP1: u64 = compute_fingerprint();
        const FP2: u64 = compute_fingerprint();
        assert_eq!(FP1, FP2);
    }

    #[test]
    fn hash_layout_distinguishes_sizes() {
        const A: u64 = hash_layout::<u8>();
        const B: u64 = hash_layout::<u64>();
        assert_ne!(A, B);
    }
}
