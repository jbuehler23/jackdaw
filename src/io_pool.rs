//! Builds Bevy's IO task pool with a stack large enough for nested
//! glTF loads.
//!
//! `bevy_gltf::load_gltf` runs its image loads through
//! `TaskPool::scope`, which blocks the IO thread it is already on.
//! While that thread is parked in `block_on`, the executor keeps
//! draining the global queue on the same stack, so it picks up
//! another queued glTF load and blocks again. Loads therefore nest
//! one inside the next for as long as the queue has work, and each
//! level costs roughly 8 KiB of stack.
//!
//! Opening a scene that names a few hundred models is enough to run
//! that off the end of the default 2 MiB thread stack. A capture of
//! `assets/zone1.bsn` (527 `GltfSource` instances) aborted with 4222
//! frames on `IO Task Pool (0)`, 273 nested `block_on` levels deep,
//! with no jackdaw frame anywhere in the trace.
//!
//! The nesting itself is Bevy's to fix (see `docs/jackdaw-upstream.md`
//! entry 13); nothing in the editor sits between the queue and the
//! blocking scope. What the editor can do is give the pool a stack
//! that the nesting fits in.
//!
//! `TaskPoolOptions` has no stack-size knob, so the pool cannot be
//! configured through `TaskPoolPlugin`. It can be replaced, though:
//! `create_default_pools` reaches the pool through
//! `IoTaskPool::get_or_init`, so a pool built before `TaskPoolPlugin`
//! runs is the one the engine adopts, and the plugin's own builder is
//! never called. [`init`] does that, mirroring the thread count Bevy
//! would have chosen so the only difference is the stack.

use bevy::tasks::{IoTaskPool, TaskPoolBuilder, available_parallelism};

/// Stack for each IO thread.
///
/// At ~8 KiB per nested load this clears about 8000 levels, against
/// the 273 that aborted the editor on a 527-model scene. Thread
/// stacks are reserved address space that is committed page by page
/// as it is touched, so the untouched remainder of a 64 MiB stack
/// costs no resident memory.
const IO_STACK_SIZE: usize = 64 * 1024 * 1024;

/// Half the cores, at least four and no more than eight.
///
/// Bevy's default is a quarter of the cores clamped into `1..=4`, sized
/// for a game streaming a few assets while it plays. An editor opening a
/// scene does the opposite: it decodes hundreds of models and their
/// textures at once with nothing else to do until they arrive, and that
/// decode runs on the IO threads rather than the compute pool. Bevy's
/// policy gives three of them on a twelve-core machine, and a capture of
/// `assets/zone1.bsn` had all three saturated -- 6.0s of thread CPU
/// inside a 2.4s load -- so the load was bounded by the pool and not by
/// the disk.
///
/// The floor is four so a small machine still overlaps decodes with the
/// stalls the nesting causes: a load blocked inside `TaskPool::scope`
/// keeps its thread parked (see the module docs). The ceiling is eight
/// because these threads compete with the render and compute pools for
/// the same cores, and past that the editor's own frame starts paying
/// for the scene it is opening.
const IO_PERCENT: f32 = 0.5;
const IO_MIN_THREADS: usize = 4;
const IO_MAX_THREADS: usize = 8;

/// How many IO threads the pool asks for on a machine with
/// `total_threads` cores.
///
/// Split out from [`init`] because the pool is a process-wide global
/// that can only be built once, which a test cannot exercise.
pub fn io_thread_count(total_threads: usize) -> usize {
    let proportion = total_threads as f32 * IO_PERCENT;
    let mut desired = proportion as usize;
    if proportion - desired as f32 >= 0.5 {
        desired += 1;
    }
    desired.clamp(IO_MIN_THREADS, IO_MAX_THREADS)
}

/// Claims the IO task pool with a large-stacked one.
///
/// Call before the app is built. Once `TaskPoolPlugin` has run the
/// pool exists and `get_or_init` keeps it, so a later call is a no-op
/// and the default 2 MiB stack stands.
pub fn init() {
    IoTaskPool::get_or_init(|| {
        TaskPoolBuilder::default()
            .num_threads(io_thread_count(available_parallelism()))
            .stack_size(IO_STACK_SIZE)
            .thread_name("IO Task Pool".to_string())
            .build()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Half the cores, floored at four and capped at eight. The
    /// twelve-core case is the one the measurement was taken on: three
    /// threads under Bevy's policy, six under this one.
    #[test]
    fn asks_for_half_the_cores_within_the_four_to_eight_band() {
        assert_eq!(io_thread_count(1), 4);
        assert_eq!(io_thread_count(4), 4);
        assert_eq!(io_thread_count(8), 4);
        assert_eq!(io_thread_count(10), 5);
        assert_eq!(io_thread_count(12), 6);
        assert_eq!(io_thread_count(16), 8);
        assert_eq!(io_thread_count(64), 8);
    }

    #[test]
    fn holds_the_nesting_that_aborted_a_real_scene() {
        // 273 levels at ~8 KiB overflowed 2 MiB; the replacement has
        // to clear that by a wide margin, not by a hair.
        const OBSERVED_LEVELS: usize = 273;
        const BYTES_PER_LEVEL: usize = 8 * 1024;
        const { assert!(IO_STACK_SIZE >= OBSERVED_LEVELS * BYTES_PER_LEVEL * 8) };
    }
}
