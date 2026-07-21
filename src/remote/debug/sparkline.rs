//! Sample buffer for the debugger's live sparklines. The `RingBuffer` is a pure
//! FIFO of recent values; the `UiMaterial` that draws them is added in a later
//! task.

/// A fixed-capacity FIFO of `f32` samples for a sparkline. Pure, no Bevy.
#[derive(Debug, Clone)]
pub struct RingBuffer {
    data: std::collections::VecDeque<f32>,
    cap: usize,
}

impl RingBuffer {
    pub fn new(cap: usize) -> Self {
        Self {
            data: std::collections::VecDeque::with_capacity(cap),
            cap,
        }
    }

    /// Append a sample, evicting the oldest once at capacity.
    pub fn push(&mut self, v: f32) {
        if self.data.len() == self.cap {
            self.data.pop_front();
        }
        self.data.push_back(v);
    }

    /// The current window, oldest first.
    pub fn samples(&self) -> Vec<f32> {
        self.data.iter().copied().collect()
    }

    /// Smallest sample in the window, or `+inf` when empty.
    pub fn min(&self) -> f32 {
        self.data.iter().copied().fold(f32::INFINITY, f32::min)
    }

    /// Largest sample in the window, or `-inf` when empty.
    pub fn max(&self) -> f32 {
        self.data.iter().copied().fold(f32::NEG_INFINITY, f32::max)
    }

    /// The most recent sample, or `0.0` when empty.
    pub fn last(&self) -> f32 {
        self.data.back().copied().unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::RingBuffer;

    #[test]
    fn push_evicts_oldest_at_capacity() {
        let mut b = RingBuffer::new(3);
        b.push(1.0);
        b.push(2.0);
        b.push(3.0);
        b.push(4.0);
        assert_eq!(b.samples(), vec![2.0, 3.0, 4.0]);
        assert_eq!(b.last(), 4.0);
    }

    #[test]
    fn min_max_over_current_window() {
        let mut b = RingBuffer::new(4);
        for v in [5.0, 1.0, 9.0, 3.0] {
            b.push(v);
        }
        assert_eq!(b.min(), 1.0);
        assert_eq!(b.max(), 9.0);
    }

    #[test]
    fn empty_buffer_is_safe() {
        let b = RingBuffer::new(4);
        assert_eq!(b.samples(), Vec::<f32>::new());
        assert_eq!(b.last(), 0.0);
    }
}
