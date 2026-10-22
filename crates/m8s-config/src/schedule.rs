//! UTC-minute event projection from validated intent. No clock, intervals or effects.
use crate::Config;
use chrono::{DateTime, Datelike, Offset, TimeZone, Timelike, Utc};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Context,
    Unsupported,
    Range,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Open,
    Close,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Event {
    pub rule_index: u8,
    pub kind: Kind,
    pub utc_seconds: i64,
    pub owner_local_timestamp: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Preview {
    pub zone: String,
    pub from_exclusive: i64,
    pub through_inclusive: i64,
    pub events: Vec<Event>,
    pub has_more: bool,
    pub truncated: bool,
}
struct Rule {
    fields: [[bool; 60]; 5],
    index: u8,
    kind: Kind,
}
fn compile(text: &str, index: u8, kind: Kind) -> Result<Rule, Error> {
    // Authoritative restricted grammar, unchanged; expansion is NOT a validator.
    super::cron(text).map_err(|_| Error::Unsupported)?;
    let fields: Vec<_> = text.split(' ').collect();
    if fields.len() != 5 || (fields[2] != "*" && fields[4] != "*") {
        return Err(Error::Unsupported);
    }
    let mut sets = [[false; 60]; 5];
    for ((field, (min, max)), set) in fields
        .into_iter()
        .zip([(0, 59), (0, 23), (1, 31), (1, 12), (0, 6)])
        .zip(&mut sets)
    {
        for item in field.split(',') {
            let (range, step) = item
                .split_once('/')
                .map_or((item, 1), |(r, s)| (r, super::cron_number(s).unwrap_or(0)));
            if step == 0 {
                return Err(Error::Unsupported);
            }
            let (start, end) = if range == "*" {
                (min, max)
            } else {
                let (a, b) = range.split_once('-').unwrap_or((range, range));
                (
                    super::cron_number(a).map_err(|_| Error::Unsupported)?,
                    super::cron_number(b).map_err(|_| Error::Unsupported)?,
                )
            };
            for n in start..=end {
                if (n - start) % step == 0 {
                    *set.get_mut(n as usize).ok_or(Error::Unsupported)? = true;
                }
            }
        }
    }
    Ok(Rule {
        fields: sets,
        index,
        kind,
    })
}
fn local(
    at: i64,
    zone: chrono_tz::Tz,
) -> Result<(chrono::NaiveDateTime, chrono::FixedOffset), Error> {
    let utc = DateTime::<Utc>::from_timestamp(at, 0)
        .ok_or(Error::Range)?
        .naive_utc();
    let offset = zone.offset_from_utc_datetime(&utc).fix();
    Ok((utc.checked_add_offset(offset).ok_or(Error::Range)?, offset))
}
/// Matching instants ONLY in (from, from+hours*3600], minute-aligned in UTC.
/// Day-of-month OR day-of-week must be literal '*'; no DOM/DOW policy invented.
/// UTC iteration omits spring gaps and preserves BOTH distinct autumn instants.
/// This does not prescribe live firing, current state, interval pairing or effects.
pub fn preview(
    config: &Config,
    tier: &str,
    from: i64,
    hours: u16,
    limit: u16,
) -> Result<Preview, Error> {
    if from < 0 || !(1..=168).contains(&hours) || !(1..=256).contains(&limit) {
        return Err(Error::Range);
    }
    let end = from
        .checked_add(i64::from(hours) * 3600)
        .ok_or(Error::Range)?;
    let zone: chrono_tz::Tz = config
        .timezone()
        .ok_or(Error::Context)?
        .parse()
        .map_err(|_| Error::Context)?;
    local(from, zone)?;
    local(end, zone)?;
    let tier = config.tier(tier).ok_or(Error::Context)?;
    let mut rules = Vec::new();
    for (i, rule) in tier.schedules().enumerate() {
        let index = u8::try_from(i).map_err(|_| Error::Unsupported)?;
        rules.push(compile(rule.open(), index, Kind::Open)?);
        rules.push(compile(rule.close(), index, Kind::Close)?);
    }
    if rules.is_empty() {
        return Err(Error::Context);
    }
    if rules.len() > 32 {
        return Err(Error::Unsupported);
    }
    let mut events = Vec::new();
    let mut has_more = false;
    let first = (from / 60)
        .checked_add(1)
        .and_then(|n| n.checked_mul(60))
        .ok_or(Error::Range)?;
    let mut at = first;
    while at <= end {
        let (date, offset) = local(at, zone)?;
        let values = [
            date.minute(),
            date.hour(),
            date.day(),
            date.month(),
            date.weekday().num_days_from_sunday(),
        ];
        for rule in &rules {
            if rule
                .fields
                .iter()
                .zip(values)
                .all(|(set, n)| set.get(n as usize) == Some(&true))
            {
                if events.len() < usize::from(limit) {
                    events.push(Event {
                        rule_index: rule.index,
                        kind: rule.kind,
                        utc_seconds: at,
                        owner_local_timestamp: format!(
                            "{}{offset}",
                            date.format("%Y-%m-%dT%H:%M:%S")
                        ),
                    });
                } else {
                    has_more = true;
                }
            }
        }
        at = at.checked_add(60).ok_or(Error::Range)?;
    }
    Ok(Preview {
        zone: zone.name().into(),
        from_exclusive: from,
        through_inclusive: end,
        events,
        has_more,
        truncated: has_more,
    })
}
