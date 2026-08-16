//! Adaptive worker-count controller.
//!
//! `--workers auto` (the default) needs to find a practical concurrency
//! sweet spot for whatever it's pointed at -- anything from a local NVMe
//! drive, where throughput keeps climbing well past `--workers 128`, to a
//! high-latency SMB share, where throughput peaks around a dozen workers
//! and then *falls* as contention and server-side throttling kick in. The
//! controller cannot know in advance which situation it is in, so it
//! measures.
//!
//! This module is a hill-climber: periodically it looks at the throughput
//! and error rate observed over the last sampling window and decides
//! whether to grow, shrink, or hold the worker count (see
//! `docs/adr/0003-adaptive-workers.md` for the full rationale). The
//! decision function ([`AdaptiveController::on_sample`]) is deliberately
//! pure and side-effect free -- it takes a [`WindowStats`] and returns an
//! [`AdaptiveDecision`] -- so it can be validated with synthetic
//! throughput curves in unit tests without needing real storage hardware,
//! which this development environment does not have for every target
//! class (NVMe, HDD, SMB, high-latency SMB) the product needs to tune
//! for. See the `converges_*` tests below for exactly that validation.

use std::time::Duration;

#[derive(Debug, Clone, Copy, Default)]
pub struct WindowStats {
    pub completed: u64,
    pub errors: u64,
    pub wall_elapsed: Duration,
}

impl WindowStats {
    pub fn throughput(&self) -> f64 {
        let secs = self.wall_elapsed.as_secs_f64();
        if secs <= 0.0 {
            0.0
        } else {
            self.completed as f64 / secs
        }
    }

    pub fn error_rate(&self) -> f64 {
        let total = self.completed + self.errors;
        if total == 0 {
            0.0
        } else {
            self.errors as f64 / total as f64
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveDecision {
    Increase(usize),
    Decrease(usize),
    Hold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trend {
    Growing,
    Shrinking,
}

const IMPROVE_THRESHOLD: f64 = 0.05;
const ERROR_RATE_BACKOFF: f64 = 0.15;
const REPROBE_AFTER_HOLDS: u32 = 6;
const MIN_STEP: usize = 1;

pub struct AdaptiveController {
    min: usize,
    max: usize,
    current: usize,
    step: usize,
    trend: Trend,
    last_throughput: Option<f64>,
    holds: u32,
    /// Alternates the direction of periodic re-probes (grow, then shrink,
    /// then grow, ...) once the controller has settled. A re-probe that
    /// always tried growing would introduce a systematic upward drift over
    /// a long-running operation even after the true optimum was found,
    /// since a probe that turns out to hurt is only ever corrected by
    /// shrinking part-way back, not by symmetric exploration in both
    /// directions. Alternating cancels that drift out.
    reprobe_grow_next: bool,
}

impl AdaptiveController {
    pub fn new(min: usize, max: usize, initial: usize) -> Self {
        let max = max.max(min.max(1));
        let initial = initial.clamp(min.max(1), max);
        let default_step = ((max.saturating_sub(min)).max(4) / 4).max(MIN_STEP);
        Self {
            min: min.max(1),
            max,
            current: initial,
            step: default_step,
            trend: Trend::Growing,
            last_throughput: None,
            holds: 0,
            reprobe_grow_next: true,
        }
    }

    pub fn current_workers(&self) -> usize {
        self.current
    }

    /// Feed one sampling window's observations in and get back a decision.
    /// The controller's internal worker-count target is updated to match
    /// the returned decision; callers apply it to the live worker pool.
    pub fn on_sample(&mut self, stats: &WindowStats) -> AdaptiveDecision {
        if stats.completed == 0 {
            return AdaptiveDecision::Hold;
        }

        let throughput = stats.throughput();

        if stats.error_rate() > ERROR_RATE_BACKOFF && self.current > self.min {
            return self.apply_shrink(throughput);
        }

        let Some(last) = self.last_throughput else {
            self.last_throughput = Some(throughput);
            return self.apply_grow(throughput);
        };

        let delta = if last > f64::EPSILON {
            (throughput - last) / last
        } else {
            0.0
        };

        match self.trend {
            Trend::Growing => {
                if delta >= IMPROVE_THRESHOLD {
                    self.grow_step();
                    self.apply_grow(throughput)
                } else if delta <= -IMPROVE_THRESHOLD {
                    self.shrink_step();
                    self.apply_shrink(throughput)
                } else {
                    self.hold_or_reprobe(throughput)
                }
            }
            Trend::Shrinking => {
                if delta >= IMPROVE_THRESHOLD {
                    // Shrinking is still helping: we were oversaturated.
                    self.grow_step();
                    self.apply_shrink(throughput)
                } else if delta <= -IMPROVE_THRESHOLD {
                    // Shrinking hurt: we overcorrected, ease back up.
                    self.shrink_step();
                    self.apply_grow(throughput)
                } else {
                    self.hold_or_reprobe(throughput)
                }
            }
        }
    }

    fn max_step(&self) -> usize {
        ((self.max.saturating_sub(self.min)).max(2) / 2).max(MIN_STEP)
    }

    fn grow_step(&mut self) {
        self.step = (self.step + self.step / 2)
            .max(MIN_STEP)
            .min(self.max_step());
    }

    fn shrink_step(&mut self) {
        self.step = (self.step / 2).max(MIN_STEP);
    }

    fn hold_or_reprobe(&mut self, throughput: f64) -> AdaptiveDecision {
        self.last_throughput = Some(throughput);
        self.holds += 1;
        if self.holds >= REPROBE_AFTER_HOLDS {
            self.holds = 0;
            self.step = MIN_STEP.max(self.step.min(2));
            let grow = self.reprobe_grow_next;
            self.reprobe_grow_next = !self.reprobe_grow_next;
            if grow {
                self.trend = Trend::Growing;
                self.apply_grow(throughput)
            } else {
                self.trend = Trend::Shrinking;
                self.apply_shrink(throughput)
            }
        } else {
            AdaptiveDecision::Hold
        }
    }

    fn apply_grow(&mut self, throughput: f64) -> AdaptiveDecision {
        self.last_throughput = Some(throughput);
        self.trend = Trend::Growing;
        let new = (self.current + self.step).min(self.max);
        if new == self.current {
            AdaptiveDecision::Hold
        } else {
            self.current = new;
            self.holds = 0;
            AdaptiveDecision::Increase(new)
        }
    }

    fn apply_shrink(&mut self, throughput: f64) -> AdaptiveDecision {
        self.last_throughput = Some(throughput);
        self.trend = Trend::Shrinking;
        let new = self.current.saturating_sub(self.step).max(self.min);
        if new == self.current {
            AdaptiveDecision::Hold
        } else {
            self.current = new;
            self.holds = 0;
            AdaptiveDecision::Decrease(new)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(completed: u64, errors: u64, secs: f64) -> WindowStats {
        WindowStats {
            completed,
            errors,
            wall_elapsed: Duration::from_secs_f64(secs),
        }
    }

    #[test]
    fn holds_with_no_completions() {
        let mut c = AdaptiveController::new(1, 128, 4);
        assert_eq!(c.on_sample(&window(0, 0, 1.0)), AdaptiveDecision::Hold);
    }

    #[test]
    fn backs_off_on_high_error_rate() {
        let mut c = AdaptiveController::new(1, 128, 32);
        let decision = c.on_sample(&window(50, 20, 1.0)); // 28% error rate
        assert!(matches!(decision, AdaptiveDecision::Decrease(n) if n < 32));
    }

    #[test]
    fn never_exceeds_configured_max() {
        let mut c = AdaptiveController::new(1, 8, 8);
        // Already at max: growth attempts should hold, not overshoot.
        for i in 0..20 {
            let d = c.on_sample(&window(1000 + i * 50, 0, 1.0));
            assert!(c.current_workers() <= 8, "exceeded max: {:?}", d);
        }
    }

    #[test]
    fn never_goes_below_configured_min() {
        let mut c = AdaptiveController::new(4, 128, 8);
        for _ in 0..20 {
            c.on_sample(&window(10, 50, 1.0)); // sustained high error rate
            assert!(c.current_workers() >= 4);
        }
    }

    /// Simulates a target whose true throughput as a function of worker
    /// count is a concave curve peaking at `peak_workers` -- representative
    /// of a latency-bound target such as SMB, where too few workers
    /// under-utilises the link and too many causes contention/throttling.
    /// The controller should converge to within a small tolerance of the
    /// peak and stay there.
    fn concave_throughput(workers: usize, peak_workers: f64, peak_value: f64) -> f64 {
        let w = workers as f64;
        // A downward parabola centred on peak_workers, floored at a small
        // positive value so throughput never hits exactly zero.
        let spread = peak_workers; // controls how wide the curve is
        let value = peak_value * (1.0 - ((w - peak_workers) / spread).powi(2));
        value.max(peak_value * 0.02)
    }

    #[test]
    fn converges_near_peak_of_concave_throughput_curve() {
        let mut c = AdaptiveController::new(1, 256, 4);
        let peak_workers = 24.0;

        let mut last_workers = c.current_workers();
        for _ in 0..200 {
            let throughput = concave_throughput(last_workers, peak_workers, 5000.0);
            c.on_sample(&window(throughput.round() as u64, 0, 1.0));
            last_workers = c.current_workers();
        }

        let distance = (last_workers as f64 - peak_workers).abs();
        assert!(
            distance <= 6.0,
            "expected convergence near {peak_workers} workers, settled at {last_workers}"
        );
    }

    /// Simulates a target where more workers is (almost) always better up
    /// to the configured ceiling -- representative of local NVMe.
    #[test]
    fn climbs_to_near_max_when_more_workers_always_helps() {
        let mut c = AdaptiveController::new(1, 128, 4);
        let mut last_workers = c.current_workers();
        for _ in 0..200 {
            // Throughput grows with workers but with diminishing returns,
            // never actually decreasing.
            let throughput = 100.0 * (last_workers as f64).sqrt();
            c.on_sample(&window(throughput.round() as u64, 0, 1.0));
            last_workers = c.current_workers();
        }
        assert!(
            last_workers >= 100,
            "expected the controller to climb toward the ceiling, settled at {last_workers}"
        );
    }

    /// Simulates a target where a single worker is already optimal and any
    /// concurrency strictly hurts -- e.g. a single spinning HDD with
    /// seek-bound access patterns.
    #[test]
    fn shrinks_toward_min_when_concurrency_always_hurts() {
        let mut c = AdaptiveController::new(1, 64, 16);
        let mut last_workers = c.current_workers();
        for _ in 0..200 {
            let throughput = 1000.0 / (last_workers as f64);
            c.on_sample(&window(throughput.round().max(1.0) as u64, 0, 1.0));
            last_workers = c.current_workers();
        }
        assert!(
            last_workers <= 4,
            "expected the controller to shrink toward min, settled at {last_workers}"
        );
    }

    #[test]
    fn manual_worker_count_bypasses_the_controller_entirely() {
        // This test documents the product contract rather than exercising
        // new code: `--workers <n>` never constructs an AdaptiveController
        // at all (see crate::options::WorkerPolicy and cursdel-cli's
        // wiring). Enforced structurally, not by a runtime flag here.
    }
}
