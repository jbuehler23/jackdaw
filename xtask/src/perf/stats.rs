//! Summaries of timing samples.

/// Median, 95th percentile and worst of a set of samples, in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub count: usize,
    pub median: f64,
    pub p95: f64,
    pub max: f64,
}

impl Summary {
    /// None for no samples. Percentiles use the nearest rank.
    pub fn of(samples: &[f64]) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let rank = |p: f64| {
            let index = (p * sorted.len() as f64).ceil() as usize;
            sorted[index.clamp(1, sorted.len()) - 1]
        };
        Some(Self {
            count: sorted.len(),
            median: rank(0.5),
            p95: rank(0.95),
            max: sorted[sorted.len() - 1],
        })
    }
}

/// A frame slower than this counts as a stall when settling.
pub const STALL_MS: f64 = 150.0;

/// How an operation settled: when the editor last stalled after it, and its
/// slowest frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settle {
    /// The call's own round trip.
    pub call: f64,
    /// From the call to the end of the last stalled frame after it, or the
    /// call alone when nothing stalled.
    pub settled: f64,
    /// The slowest status round trip after the call.
    pub worst_frame: f64,
}

impl Settle {
    /// `samples` holds each status round trip after the call as its start,
    /// measured from the call's start, and its length.
    pub fn of(call: f64, samples: &[(f64, f64)]) -> Self {
        let settled = samples
            .iter()
            .filter(|(_, rtt)| *rtt > STALL_MS)
            .map(|(start, rtt)| start + rtt)
            .fold(call, f64::max);
        let worst_frame = samples.iter().map(|(_, rtt)| *rtt).fold(0.0, f64::max);
        Self {
            call,
            settled,
            worst_frame,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_samples_have_no_summary() {
        assert_eq!(Summary::of(&[]), None);
    }

    #[test]
    fn median_and_p95_take_the_nearest_rank() {
        let samples: Vec<f64> = (1..=100).rev().map(f64::from).collect();
        let summary = Summary::of(&samples).unwrap();
        assert_eq!(summary.count, 100);
        assert_eq!(summary.median, 50.0);
        assert_eq!(summary.p95, 95.0);
        assert_eq!(summary.max, 100.0);
    }

    #[test]
    fn one_sample_is_every_statistic() {
        let summary = Summary::of(&[16.6]).unwrap();
        assert_eq!(
            (summary.median, summary.p95, summary.max),
            (16.6, 16.6, 16.6)
        );
    }

    #[test]
    fn a_quiet_operation_settles_when_its_call_returns() {
        let settle = Settle::of(12.0, &[(12.0, 17.0), (29.0, 16.0)]);
        assert_eq!(settle.settled, 12.0);
        assert_eq!(settle.worst_frame, 17.0);
    }

    #[test]
    fn an_operation_settles_at_the_end_of_its_last_stall() {
        let samples = [
            (10.0, 17.0),
            (27.0, 4000.0),
            (4027.0, 20.0),
            (4047.0, 900.0),
            (4947.0, 16.0),
        ];
        let settle = Settle::of(10.0, &samples);
        assert_eq!(settle.settled, 4947.0);
        assert_eq!(settle.worst_frame, 4000.0);
    }
}
