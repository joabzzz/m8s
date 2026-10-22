use m8s_core::queue::{
    Controller, Error, Intent, Policy, Proposal, QueueSample, Reason, Thresholds, Timing,
};

fn timing() -> Timing {
    Timing {
        scale_up_for_seconds: 4,
        scale_down_idle_seconds: 4,
        maximum_sample_gap_seconds: 2,
        freshness_seconds: 2,
    }
}
fn policy(min: u32, max: u32) -> Policy {
    Policy::new(
        min,
        max,
        Thresholds {
            queue_wait_ms_gt: Some(10),
            inflight_gt: Some(2),
        },
        timing(),
    )
    .unwrap()
}
fn sample(at: i64, wait: u64, inflight: u64) -> Option<QueueSample> {
    Some(QueueSample {
        observed_at: at,
        queue_wait_ms: wait,
        inflight,
    })
}
fn observe(
    c: &mut Controller,
    at: i64,
    desired: u32,
    wait: u64,
    inflight: u64,
) -> Option<Proposal> {
    c.observe(at, desired, Intent::Enabled, sample(at, wait, inflight))
        .unwrap()
}
fn up(target: u32) -> Option<Proposal> {
    Some(Proposal {
        target,
        reason: Reason::SustainedPressure,
    })
}

#[test]
fn strict_thresholds_or_duration_and_reset_after_proposal() {
    let mut c = Controller::new(policy(0, 4));
    for at in 0..5 {
        assert_eq!(observe(&mut c, at, 1, 10, 2), None);
    }
    assert_eq!(observe(&mut c, 5, 1, 11, 0), None);
    assert_eq!(observe(&mut c, 7, 1, 0, 3), None); // OR, not AND
    assert_eq!(observe(&mut c, 8, 1, 11, 3), None);
    assert_eq!(observe(&mut c, 9, 1, 11, 0), up(2));
    assert_eq!(observe(&mut c, 10, 1, 11, 0), None);
    assert_eq!(observe(&mut c, 12, 1, 11, 0), None);
    assert_eq!(observe(&mut c, 14, 1, 11, 0), up(2));
}

#[test]
fn gap_and_freshness_equality_and_unknown_resets() {
    let mut c = Controller::new(policy(0, 4));
    for at in [0, 2, 4] {
        let result = c
            .observe(at + 2, 1, Intent::Enabled, sample(at, 11, 0))
            .unwrap();
        assert_eq!(result, if at == 4 { up(2) } else { None });
    }
    assert_eq!(observe(&mut c, 7, 1, 11, 0), None);
    assert_eq!(observe(&mut c, 10, 1, 11, 0), None); // gap > 2
    assert_eq!(observe(&mut c, 12, 1, 11, 0), None);
    assert_eq!(c.observe(13, 1, Intent::Enabled, None), Ok(None));
    assert_eq!(observe(&mut c, 14, 1, 11, 0), None);
    assert_eq!(
        c.observe(18, 1, Intent::Enabled, sample(15, 11, 0)),
        Ok(None)
    ); // stale
    assert_eq!(observe(&mut c, 19, 1, 11, 0), None);
    assert_eq!(observe(&mut c, 21, 1, 11, 0), None);
    assert_eq!(observe(&mut c, 23, 1, 11, 0), up(2));
}

#[test]
fn clock_and_sample_high_watermarks_never_rebase() {
    let mut c = Controller::new(policy(0, 4));
    assert_eq!(observe(&mut c, 10, 1, 11, 0), None);
    for at in [10, 9, 10] {
        assert_eq!(
            c.observe(at, 1, Intent::Enabled, sample(at, 11, 0)),
            Err(Error::InvalidClock)
        );
    }
    assert_eq!(
        c.observe(11, 1, Intent::Enabled, sample(12, 11, 0)),
        Err(Error::InvalidSample)
    );
    assert_eq!(
        c.observe(12, 1, Intent::Enabled, sample(10, 11, 0)),
        Err(Error::InvalidSample)
    );
    assert_eq!(observe(&mut c, 13, 1, 11, 0), None);
    assert_eq!(observe(&mut c, 15, 1, 11, 0), None);
    assert_eq!(observe(&mut c, 17, 1, 11, 0), up(2));
    assert_eq!(
        c.observe(-1, 1, Intent::Enabled, None),
        Err(Error::InvalidClock)
    );
    assert_eq!(
        c.observe(18, 1, Intent::Enabled, sample(-1, 0, 0)),
        Err(Error::InvalidSample)
    );
}

#[test]
fn desired_and_intent_changes_reset_and_paused_never_starts() {
    let mut c = Controller::new(policy(2, 4));
    assert_eq!(observe(&mut c, 0, 0, 11, 0), None);
    assert_eq!(observe(&mut c, 2, 0, 11, 0), None);
    assert_eq!(c.observe(4, 0, Intent::Paused, sample(4, 11, 0)), Ok(None));
    assert_eq!(observe(&mut c, 5, 0, 11, 0), None);
    assert_eq!(observe(&mut c, 7, 0, 11, 0), None);
    assert_eq!(observe(&mut c, 9, 0, 11, 0), up(2));
    assert_eq!(observe(&mut c, 11, 2, 11, 0), None);
    assert_eq!(observe(&mut c, 13, 2, 11, 0), None);
    assert_eq!(observe(&mut c, 15, 3, 11, 0), None); // desired changed
    assert_eq!(
        c.observe(16, 1, Intent::Enabled, None),
        Err(Error::InvalidDesired)
    );
    assert_eq!(
        c.observe(17, 5, Intent::Paused, None),
        Err(Error::InvalidDesired)
    );
    assert_eq!(observe(&mut c, 18, 0, 0, 0), None); // no automatic minimum
}

#[test]
fn idle_requires_both_zero_and_never_crosses_min_or_max() {
    let mut c = Controller::new(policy(1, 3));
    for at in 0..5 {
        assert_eq!(observe(&mut c, at, 3, 11, 0), None);
    }
    for at in 5..10 {
        assert_eq!(observe(&mut c, at, 3, 0, 1), None);
    }
    assert_eq!(observe(&mut c, 10, 3, 0, 0), None);
    assert_eq!(observe(&mut c, 12, 3, 0, 0), None);
    assert_eq!(
        observe(&mut c, 14, 3, 0, 0),
        Some(Proposal {
            target: 2,
            reason: Reason::SustainedIdle
        })
    );
    for at in 15..22 {
        assert_eq!(observe(&mut c, at, 1, 0, 0), None);
    }
    let mut disabled = Controller::new(policy(0, 0));
    for at in 0..8 {
        assert_eq!(observe(&mut disabled, at, 0, 11, 0), None);
    }
    let mut cold = Controller::new(policy(0, 1));
    for at in 0..4 {
        assert_eq!(observe(&mut cold, at, 0, 11, 0), None);
    }
    assert_eq!(observe(&mut cold, 4, 0, 11, 0), up(1));
}

#[test]
fn arithmetic_at_i64_max_and_invalid_policy() {
    let mut c = Controller::new(policy(0, 16));
    assert_eq!(observe(&mut c, i64::MAX - 4, 15, u64::MAX, u64::MAX), None);
    assert_eq!(observe(&mut c, i64::MAX - 2, 15, u64::MAX, u64::MAX), None);
    assert_eq!(observe(&mut c, i64::MAX, 15, u64::MAX, u64::MAX), up(16));
    assert_eq!(
        c.observe(i64::MAX, 15, Intent::Enabled, None),
        Err(Error::InvalidClock)
    );
    for (min, max, wait, inflight) in [
        (2, 1, Some(1), None),
        (0, 17, Some(1), None),
        (0, 1, None, None),
        (0, 1, Some(0), Some(1)),
        (0, 1, None, Some(0)),
    ] {
        assert_eq!(
            Policy::new(
                min,
                max,
                Thresholds {
                    queue_wait_ms_gt: wait,
                    inflight_gt: inflight
                },
                timing()
            ),
            Err(Error::InvalidPolicy)
        );
    }
    for bad in [0, u64::MAX] {
        for field in 0..4 {
            let mut t = timing();
            match field {
                0 => t.scale_up_for_seconds = bad,
                1 => t.scale_down_idle_seconds = bad,
                2 => t.maximum_sample_gap_seconds = bad,
                _ => t.freshness_seconds = bad,
            }
            assert_eq!(
                Policy::new(
                    0,
                    1,
                    Thresholds {
                        queue_wait_ms_gt: Some(1),
                        inflight_gt: None
                    },
                    t
                ),
                Err(Error::InvalidPolicy)
            );
        }
    }
}

#[test]
fn interruptions_reset_each_window_and_unconfigured_metric_is_not_pressure() {
    for interruption in 0..5 {
        let mut c = Controller::new(policy(0, 4));
        assert_eq!(observe(&mut c, 0, 1, 11, 0), None);
        assert_eq!(observe(&mut c, 2, 1, 11, 0), None);
        match interruption {
            0 => assert_eq!(c.observe(3, 1, Intent::Enabled, None), Ok(None)),
            1 => assert_eq!(
                c.observe(3, 1, Intent::Enabled, sample(4, 11, 0)),
                Err(Error::InvalidSample)
            ),
            2 => assert_eq!(
                c.observe(2, 1, Intent::Enabled, sample(2, 11, 0)),
                Err(Error::InvalidClock)
            ),
            3 => assert_eq!(
                c.observe(3, 1, Intent::Enabled, sample(2, 11, 0)),
                Err(Error::InvalidSample)
            ),
            _ => assert_eq!(observe(&mut c, 3, 1, 0, 0), None),
        }
        assert_eq!(observe(&mut c, 4, 1, 11, 0), None);
        assert_eq!(observe(&mut c, 6, 1, 11, 0), None);
        assert_eq!(observe(&mut c, 8, 1, 11, 0), up(2));
    }
    let p = Policy::new(
        0,
        1,
        Thresholds {
            queue_wait_ms_gt: None,
            inflight_gt: Some(2),
        },
        timing(),
    )
    .unwrap();
    let mut c = Controller::new(p);
    for at in 0..6 {
        assert_eq!(observe(&mut c, at, 0, u64::MAX, 2), None);
    }
    // Pressure interrupts idle; only a new full idle window may stop the last replica.
    let mut c = Controller::new(policy(0, 1));
    assert_eq!(observe(&mut c, 0, 1, 0, 0), None);
    assert_eq!(observe(&mut c, 2, 1, 0, 0), None);
    assert_eq!(observe(&mut c, 3, 1, 11, 0), None);
    assert_eq!(observe(&mut c, 4, 1, 0, 0), None);
    assert_eq!(observe(&mut c, 6, 1, 0, 0), None);
    assert_eq!(
        observe(&mut c, 8, 1, 0, 0),
        Some(Proposal {
            target: 0,
            reason: Reason::SustainedIdle
        })
    );
    assert_eq!(observe(&mut c, 9, 1, 0, 0), None);
}

#[test]
fn maximum_duration_is_representable_without_timestamp_addition() {
    let max = i64::MAX as u64;
    let p = Policy::new(
        0,
        1,
        Thresholds {
            queue_wait_ms_gt: Some(1),
            inflight_gt: None,
        },
        Timing {
            scale_up_for_seconds: max,
            scale_down_idle_seconds: max,
            maximum_sample_gap_seconds: max,
            freshness_seconds: max,
        },
    )
    .unwrap();
    let mut c = Controller::new(p);
    assert_eq!(observe(&mut c, 0, 0, 2, 0), None);
    assert_eq!(observe(&mut c, i64::MAX, 0, 2, 0), up(1));
}
