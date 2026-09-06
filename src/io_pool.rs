//! Builds Bevy's IO task pool with a stack large enough for nested glTF loads.
//!
//! `bevy_gltf::load_gltf` blocks its own IO thread inside `TaskPool::scope`,
//! and the executor keeps nesting further loads on the same stack, overflowing
//! the default 2 MiB on scenes with hundreds of models. `TaskPoolOptions` has
//! no stack-size knob, so [`init`] claims the pool before `TaskPoolPlugin` runs.

use bevy::tasks::{IoTaskPool, TaskPoolBuilder, available_parallelism};

/// Stack for each IO thread. Reserved address space, committed page by page,
/// so the untouched remainder costs no resident memory.
const IO_STACK_SIZE: usize = 64 * 1024 * 1024;

/// Half the cores, at least four and no more than eight. Bevy's default of a
/// quarter clamped to `1..=4` leaves a scene open bounded by the pool rather
/// than the disk; the ceiling keeps these threads from crowding out the render
/// and compute pools.
const IO_PERCENT: f32 = 0.5;
const IO_MIN_THREADS: usize = 4;
const IO_MAX_THREADS: usize = 8;

/// How many IO threads the pool asks for on a machine with `total_threads`
/// cores.
pub fn io_thread_count(total_threads: usize) -> usize {
    let proportion = total_threads as f32 * IO_PERCENT;
    let mut desired = proportion as usize;
    if proportion - desired as f32 >= 0.5 {
        desired += 1;
    }
    desired.clamp(IO_MIN_THREADS, IO_MAX_THREADS)
}

/// Claims the IO task pool with a large-stacked one. Call before the app is
/// built; once `TaskPoolPlugin` has run this is a no-op.
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
        // 273 levels at ~8 KiB overflowed 2 MiB, by a wide margin.
        const OBSERVED_LEVELS: usize = 273;
        const BYTES_PER_LEVEL: usize = 8 * 1024;
        const { assert!(IO_STACK_SIZE >= OBSERVED_LEVELS * BYTES_PER_LEVEL * 8) };
    }
}
