//! Unwired, deterministic queue-based replica proposals; never execution permission.

/// Caller-chosen engineering inputs, not an approved live scaling profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    min: u32,
    max: u32,
    queue_wait_ms_gt: Option<u64>,
    inflight_gt: Option<u64>,
    up_seconds: u64,
    idle_seconds: u64,
    gap_seconds: u64,
    freshness_seconds: u64,
}

/// At least one positive threshold is required. Configured metrics combine with OR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Thresholds {
    pub queue_wait_ms_gt: Option<u64>,
    pub inflight_gt: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub scale_up_for_seconds: u64,
    pub scale_down_idle_seconds: u64,
    pub maximum_sample_gap_seconds: u64,
    pub freshness_seconds: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidPolicy,
    InvalidClock,
    InvalidDesired,
    InvalidSample,
}

impl Policy {
    /// All durations must be positive and representable as nonnegative Unix seconds.
    pub fn new(min: u32, max: u32, thresholds: Thresholds, timing: Timing) -> Result<Self, Error> {
        let Thresholds {
            queue_wait_ms_gt,
            inflight_gt,
        } = thresholds;
        let Timing {
            scale_up_for_seconds: up_seconds,
            scale_down_idle_seconds: idle_seconds,
            maximum_sample_gap_seconds: gap_seconds,
            freshness_seconds,
        } = timing;
        if min > max
            || max > 16
            || (queue_wait_ms_gt.is_none() && inflight_gt.is_none())
            || queue_wait_ms_gt == Some(0)
            || inflight_gt == Some(0)
            || [up_seconds, idle_seconds, gap_seconds, freshness_seconds]
                .iter()
                .any(|&v| v == 0 || v > i64::MAX as u64)
        {
            return Err(Error::InvalidPolicy);
        }
        Ok(Self {
            min,
            max,
            queue_wait_ms_gt,
            inflight_gt,
            up_seconds,
            idle_seconds,
            gap_seconds,
            freshness_seconds,
        })
    }
}

/// Input intent only: Enabled is NOT authorization or a durable manual-stop latch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Enabled,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueSample {
    pub observed_at: i64,
    pub queue_wait_ms: u64,
    pub inflight: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    SustainedPressure,
    SustainedIdle,
}

/// Absolute proposed desired count, not a mutation, claim, or effect guarantee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proposal {
    pub target: u32,
    pub reason: Reason,
}

/// Replace the controller to replace its immutable policy. No clocks or effects are owned.
pub struct Controller {
    policy: Policy,
    now_high: Option<i64>,
    sample_high: Option<i64>,
    previous: Option<(u32, Intent)>,
    last_sample: Option<i64>,
    pressure_since: Option<i64>,
    idle_since: Option<i64>,
}

impl Controller {
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            now_high: None,
            sample_high: None,
            previous: None,
            last_sample: None,
            pressure_since: None,
            idle_since: None,
        }
    }

    fn reset(&mut self) {
        self.last_sample = None;
        self.pressure_since = None;
        self.idle_since = None;
    }

    /// Strictly increasing now and sample timestamps; equality is not a new sample.
    /// Freshness and sample-gap equality are accepted. Windows use sample time,
    /// not receipt time; no continuity between observations is proven.
    pub fn observe(
        &mut self,
        now: i64,
        desired: u32,
        intent: Intent,
        sample: Option<QueueSample>,
    ) -> Result<Option<Proposal>, Error> {
        if now < 0 || self.now_high.is_some_and(|high| now <= high) {
            self.reset();
            return Err(Error::InvalidClock);
        }
        self.now_high = Some(now);
        if self.previous != Some((desired, intent)) {
            self.reset();
            self.previous = Some((desired, intent));
        }
        let p = self.policy;
        if desired != 0 && (desired < p.min || desired > p.max) {
            self.reset();
            return Err(Error::InvalidDesired);
        }
        if intent == Intent::Paused {
            self.reset();
            return Ok(None);
        }
        let Some(s) = sample else {
            self.reset();
            return Ok(None);
        };
        if s.observed_at < 0
            || s.observed_at > now
            || self.sample_high.is_some_and(|high| s.observed_at <= high)
        {
            self.reset();
            return Err(Error::InvalidSample);
        }
        self.sample_high = Some(s.observed_at);
        if i128::from(now) - i128::from(s.observed_at) > i128::from(p.freshness_seconds) {
            self.reset();
            return Ok(None);
        }
        if self.last_sample.is_some_and(|last| {
            i128::from(s.observed_at) - i128::from(last) > i128::from(p.gap_seconds)
        }) {
            self.reset();
        }
        self.last_sample = Some(s.observed_at);
        let pressure = p.queue_wait_ms_gt.is_some_and(|v| s.queue_wait_ms > v)
            || p.inflight_gt.is_some_and(|v| s.inflight > v);
        let idle = s.queue_wait_ms == 0 && s.inflight == 0;
        let proposal = if pressure {
            self.idle_since = None;
            let since = *self.pressure_since.get_or_insert(s.observed_at);
            if i128::from(s.observed_at) - i128::from(since) >= i128::from(p.up_seconds)
                && desired < p.max
            {
                Some(Proposal {
                    target: if desired == 0 {
                        p.min.max(1)
                    } else {
                        desired + 1
                    },
                    reason: Reason::SustainedPressure,
                })
            } else {
                None
            }
        } else if idle {
            self.pressure_since = None;
            let since = *self.idle_since.get_or_insert(s.observed_at);
            if i128::from(s.observed_at) - i128::from(since) >= i128::from(p.idle_seconds)
                && desired > p.min
            {
                Some(Proposal {
                    target: desired - 1,
                    reason: Reason::SustainedIdle,
                })
            } else {
                None
            }
        } else {
            self.pressure_since = None;
            self.idle_since = None;
            None
        };
        if proposal.is_some() {
            self.reset();
        }
        Ok(proposal)
    }
}
