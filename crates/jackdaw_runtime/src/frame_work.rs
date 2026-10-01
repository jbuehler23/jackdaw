//! How much of a long job one frame takes on.
//!
//! A job that has to touch the world cannot leave the main thread, so it is
//! handed out a batch at a time and the window draws in between. Some of that
//! work costs nothing where it is handed out and everything later in the same
//! frame -- inserting a component that registers a scene instance is one -- so
//! the loop cannot time itself as it goes and the caller reports what the
//! batch before it ended up costing. [`FramePace`] takes fewer items after a
//! batch that ran past its budget and more after one that left room to spare.

use core::time::Duration;

/// How many items of a long job to take on this frame.
pub struct FramePace {
    batch: usize,
    least: usize,
    most: usize,
}

impl FramePace {
    /// A pace that takes between `least` and `most` items a frame, opening at
    /// `least` so the first frame of a job is never the expensive one.
    pub fn new(least: usize, most: usize) -> Self {
        Self {
            batch: least.max(1),
            least: least.max(1),
            most: most.max(least.max(1)),
        }
    }

    /// How many of `pending` items to take now, given what the batch before
    /// this one cost and the most this job may cost a frame.
    pub fn take(&mut self, spent: Duration, budget: Duration, pending: usize) -> usize {
        if spent > budget {
            self.batch = (self.batch / 2).max(self.least);
        } else if spent * 2 < budget {
            self.batch = (self.batch * 2).min(self.most);
        }
        self.batch.min(pending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUDGET: Duration = Duration::from_millis(50);

    #[test]
    fn a_batch_that_ran_long_is_smaller_the_next_time_round() {
        let mut pace = FramePace::new(1, 256);
        let opening = pace.take(Duration::ZERO, BUDGET, 1000);
        let after_overrun = pace.take(BUDGET * 4, BUDGET, 1000);

        assert!(
            after_overrun < opening,
            "a frame that cost {BUDGET:?} four times over asks for less, not {after_overrun}"
        );
    }

    #[test]
    fn a_batch_that_left_room_to_spare_grows_up_to_the_ceiling() {
        let mut pace = FramePace::new(1, 16);
        for _ in 0..10 {
            pace.take(Duration::ZERO, BUDGET, 1000);
        }

        assert_eq!(
            pace.take(Duration::ZERO, BUDGET, 1000),
            16,
            "cheap frames climb to the ceiling and stop there"
        );
    }

    #[test]
    fn a_pace_still_takes_an_item_when_every_batch_runs_long() {
        let mut pace = FramePace::new(1, 256);
        for _ in 0..20 {
            pace.take(BUDGET * 10, BUDGET, 1000);
        }

        assert_eq!(
            pace.take(BUDGET * 10, BUDGET, 1000),
            1,
            "a job whose items each cost more than a whole frame still finishes"
        );
    }

    #[test]
    fn a_paced_job_settles_on_frames_that_fit_the_budget() {
        // Items cost a tenth of the budget each, so five a frame is the most
        // the pace can hold and still leave the frame room to draw.
        const ITEM: Duration = Duration::from_millis(5);
        let mut pace = FramePace::new(1, 256);
        let mut pending = 1000usize;
        let mut last_frame = Duration::ZERO;
        let mut frames = 0;

        while pending > 0 && frames < 1000 {
            let taken = pace.take(last_frame, BUDGET, pending);
            pending -= taken;
            last_frame = ITEM * taken as u32;
            frames += 1;
        }

        assert_eq!(pending, 0, "the job finishes");
        assert!(
            last_frame <= BUDGET * 2,
            "the pace settles near the budget rather than running away: {last_frame:?}"
        );
    }
}
