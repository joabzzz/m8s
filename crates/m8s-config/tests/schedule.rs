use m8s_config::{
    Config, ValidationLimits,
    schedule::{self, Error, Kind},
};
fn config(zone: &str, rules: &[(&str, &str)]) -> Config {
    let mut text =
        format!("schema=1\n[owner]\ntimezone='{zone}'\n[tier.t]\nmodel='example-27b-q4km'\n");
    for (open, close) in rules {
        text += &format!("[[tier.t.schedule]]\nopen='{open}'\nclose='{close}'\n");
    }
    let catalog = toml::from_str(m8s_core::models::EXAMPLE_CATALOG).unwrap();
    Config::parse(text.as_bytes(), &catalog, &ValidationLimits::default()).unwrap()
}
fn ts(text: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(text)
        .unwrap()
        .timestamp()
}
#[test]
fn weekday_zone_midminute_exclusion_and_inclusive_end() {
    let c = config("America/New_York", &[("0 8 * * 1-5", "0 18 * * 1-5")]);
    let p = schedule::preview(&c, "t", ts("2024-03-04T12:00:00Z"), 1, 256).unwrap();
    assert_eq!(p.events.len(), 1);
    assert_eq!(p.events[0].utc_seconds, ts("2024-03-04T13:00:00Z"));
    assert_eq!(
        p.events[0].owner_local_timestamp,
        "2024-03-04T08:00:00-05:00"
    );
    assert!(
        schedule::preview(&c, "t", ts("2024-03-04T13:00:00Z"), 1, 256)
            .unwrap()
            .events
            .is_empty()
    );
    assert!(
        schedule::preview(&c, "t", ts("2024-03-03T12:00:30Z"), 1, 256)
            .unwrap()
            .events
            .is_empty()
    );
    let c = config("UTC", &[("* * * * *", "59 23 * * *")]);
    let p = schedule::preview(&c, "t", 30, 1, 256).unwrap();
    assert_eq!(p.events[0].utc_seconds, 60);
    assert_eq!(p.events.last().unwrap().utc_seconds, 3600);
    assert_eq!(p.events.len(), 60);
}
#[test]
fn lists_ranges_steps_sunday_and_calendar_boundaries() {
    for (open, minutes) in [
        ("0,10-20/5 0 * * 0", vec![10, 15, 20]),
        ("*/20 0 * * *", vec![20, 40]),
        ("5-15/5 0 * * *", vec![5, 10, 15]),
    ] {
        let c = config("UTC", &[(open, "1 1 * * *")]);
        let from = ts("2024-01-07T00:00:00Z");
        assert_eq!(
            schedule::preview(&c, "t", from, 1, 256)
                .unwrap()
                .events
                .iter()
                .map(|e| (e.utc_seconds - from) / 60)
                .collect::<Vec<_>>(),
            minutes
        );
    }
    let c = config("UTC", &[("0 0 */2 2-4/2 *", "1 1 * * *")]);
    let p = schedule::preview(&c, "t", ts("2024-02-28T23:00:00Z"), 1, 256).unwrap();
    assert_eq!(p.events.len(), 1);
    assert_eq!(
        p.events[0].owner_local_timestamp,
        "2024-02-29T00:00:00+00:00"
    );
    assert!(
        schedule::preview(&c, "t", ts("2023-02-28T23:00:00Z"), 1, 256)
            .unwrap()
            .events
            .is_empty()
    );
    let weekday = config("UTC", &[("0 0 * * 1-5/2", "1 1 * * *")]);
    assert_eq!(
        schedule::preview(&weekday, "t", ts("2024-03-03T23:00:00Z"), 1, 256)
            .unwrap()
            .events
            .len(),
        1
    );
    assert!(
        schedule::preview(&weekday, "t", ts("2024-03-04T23:00:00Z"), 1, 256)
            .unwrap()
            .events
            .is_empty()
    );
    let c = config("UTC", &[("0 0 1 1 *", "1 1 * * *")]);
    let p = schedule::preview(&c, "t", ts("2024-12-31T23:00:00Z"), 1, 256).unwrap();
    assert_eq!(
        p.events[0].owner_local_timestamp,
        "2025-01-01T00:00:00+00:00"
    );
}
#[test]
fn spring_gap_and_autumn_repeat_are_distinct_utc_events_not_live_policy() {
    let c = config("America/New_York", &[("30 2 * * *", "59 23 * * *")]);
    assert!(
        schedule::preview(&c, "t", ts("2024-03-10T06:00:00Z"), 3, 256)
            .unwrap()
            .events
            .is_empty()
    );
    let c = config("America/New_York", &[("30 1 * * *", "59 23 * * *")]);
    let p = schedule::preview(&c, "t", ts("2024-11-03T04:00:00Z"), 3, 256).unwrap();
    assert_eq!(p.events.len(), 2);
    assert_eq!(p.events[0].utc_seconds, ts("2024-11-03T05:30:00Z"));
    assert_eq!(p.events[1].utc_seconds, ts("2024-11-03T06:30:00Z"));
    assert_eq!(
        p.events[0].owner_local_timestamp,
        "2024-11-03T01:30:00-04:00"
    );
    assert_eq!(
        p.events[1].owner_local_timestamp,
        "2024-11-03T01:30:00-05:00"
    );
}
#[test]
fn deterministic_ties_preserve_rules_and_limit_truncates_without_pairing() {
    let c = config(
        "UTC",
        &[("* * * * *", "*/1 * * * *"), ("*/1 * * * *", "* * * * *")],
    );
    let p = schedule::preview(&c, "t", 0, 1, 5).unwrap();
    assert_eq!(
        p.events
            .iter()
            .map(|e| (e.utc_seconds, e.rule_index, e.kind))
            .collect::<Vec<_>>(),
        vec![
            (60, 0, Kind::Open),
            (60, 0, Kind::Close),
            (60, 1, Kind::Open),
            (60, 1, Kind::Close),
            (120, 0, Kind::Open)
        ]
    );
    assert!(p.has_more && p.truncated);
    let c = config("UTC", &[("0 1 * * *", "0 2 * * *")]);
    let p = schedule::preview(&c, "t", 0, 1, 1).unwrap();
    assert_eq!(p.events.len(), 1);
    assert!(!p.has_more && !p.truncated);
    let c = config("UTC", &vec![("* * * * *", "*/1 * * * *"); 16]);
    let p = schedule::preview(&c, "t", 0, 168, 256).unwrap();
    assert_eq!(p.events.len(), 256);
    assert_eq!(p.through_inclusive, 604800);
    assert!(p.has_more);
}
#[test]
fn invalid_context_unsupported_day_policy_and_ranges_fail_closed() {
    for open in ["0 0 1 * 1", "0 0 */1 * 1", "0 0 1 * */1"] {
        let c = config("UTC", &[(open, "59 23 * * *")]);
        assert_eq!(schedule::preview(&c, "t", 0, 1, 1), Err(Error::Unsupported));
    }
    let c = config("UTC", &[("* * * * *", "59 23 * * *")]);
    for (from, hours, limit) in [
        (-1, 1, 1),
        (i64::MAX, 1, 1),
        (i64::MAX - 3600, 1, 1),
        (0, 0, 1),
        (0, 169, 1),
        (0, 1, 0),
        (0, 1, 257),
    ] {
        assert_eq!(
            schedule::preview(&c, "t", from, hours, limit),
            Err(Error::Range)
        );
    }
    let max = chrono::NaiveDate::MAX
        .and_hms_opt(23, 59, 59)
        .unwrap()
        .and_utc()
        .timestamp();
    assert_eq!(
        schedule::preview(&c, "t", max - 3599, 1, 1),
        Err(Error::Range)
    );
    let east = config("Pacific/Kiritimati", &[("* * * * *", "59 23 * * *")]);
    assert_eq!(
        schedule::preview(&east, "t", max - 7200, 1, 1),
        Err(Error::Range)
    );
    let unsupported_close = config("UTC", &[("0 0 * * *", "0 1 1 * 1")]);
    assert_eq!(
        schedule::preview(&unsupported_close, "t", 0, 1, 1),
        Err(Error::Unsupported)
    );
    assert_eq!(
        schedule::preview(&c, "unknown", 0, 1, 1),
        Err(Error::Context)
    );
    let c = config("UTC", &[]);
    assert_eq!(schedule::preview(&c, "t", 0, 1, 1), Err(Error::Context));
}
