//! Pure admission and lifecycle policy. Callers persist launch intent and reserve
//! budget/rate-limit capacity atomically before performing any provider effect.
use crate::{MicroUsd, Offer, Phase, Timestamp};

pub const DEFAULT_LAUNCHES_PER_HOUR: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyError {
    UnknownLocation,
    DeniedLocation,
    Bandwidth,
    Interruptible,
    InstanceBudget,
    HourlyBudget,
    MonthlyBudget,
    InvalidHorizon,
    LaunchLimit,
    InvalidLaunchHistory,
    UnsignedImage,
}

/// All amounts include existing reservations as well as active rentals.
#[derive(Clone, Copy, Debug)]
pub struct PlacementInputs {
    pub minimum_down_mbps: u64,
    pub allow_interruptible: bool,
    pub signed_m8s_image_verified: bool,
    pub max_instance_hourly: MicroUsd,
    pub max_total_hourly: MicroUsd,
    pub committed_hourly: MicroUsd,
    pub monthly_limit: MicroUsd,
    pub monthly_spent_and_reserved: MicroUsd,
    /// Positive bounded rental horizon, enforced separately by lifecycle timers.
    pub reserve_seconds: u64,
}

/// Fail closed on unrecognised country values; adapters must supply ISO alpha-2.
pub fn check_location(offer: &Offer) -> Result<(), PolicyError> {
    let country = offer
        .country
        .as_deref()
        .ok_or(PolicyError::UnknownLocation)?;
    if matches!(country, "CN" | "CU" | "IR" | "KP" | "SY" | "RU" | "BY") {
        return Err(PolicyError::DeniedLocation);
    }
    // Assigned ISO 3166-1 alpha-2 codes, not arbitrary two-letter strings.
    const COUNTRIES: &str = "AD AE AF AG AI AL AM AO AQ AR AS AT AU AW AX AZ BA BB BD BE BF BG BH BI BJ BL BM BN BO BQ BR BS BT BV BW BZ CA CC CD CF CG CH CI CK CL CM CO CR CV CW CX CY CZ DE DJ DK DM DO DZ EC EE EG EH ER ES ET FI FJ FK FM FO FR GA GB GD GE GF GG GH GI GL GM GN GP GQ GR GS GT GU GW GY HK HM HN HR HT HU ID IE IL IM IN IO IQ IS IT JE JM JO JP KE KG KH KI KM KN KR KW KY KZ LA LB LC LI LK LR LS LT LU LV LY MA MC MD ME MF MG MH MK ML MM MN MO MP MQ MR MS MT MU MV MW MX MY MZ NA NC NE NF NG NI NL NO NP NR NU NZ OM PA PE PF PG PH PK PL PM PN PR PS PT PW PY QA RE RO RS RW SA SB SC SD SE SG SH SI SJ SK SL SM SN SO SR SS ST SV SX SZ TC TD TF TG TH TJ TK TL TM TN TO TR TT TV TW TZ UA UG UM US UY UZ VA VC VE VG VI VN VU WF WS YE YT ZA ZM ZW";
    if !COUNTRIES.split_ascii_whitespace().any(|c| c == country) {
        return Err(PolicyError::UnknownLocation);
    }
    if country == "UA" {
        // Conservative subdivision allowlist: partially occupied oblasts are
        // excluded too. No fuzzy matching of provider free-text names.
        match offer.region.as_deref() {
            Some("UA-43" | "UA-14" | "UA-09" | "UA-23" | "UA-65" | "UA-40") => {
                return Err(PolicyError::DeniedLocation);
            }
            Some(
                "UA-05" | "UA-07" | "UA-12" | "UA-18" | "UA-21" | "UA-26" | "UA-30" | "UA-32"
                | "UA-35" | "UA-46" | "UA-48" | "UA-51" | "UA-53" | "UA-56" | "UA-59" | "UA-61"
                | "UA-63" | "UA-68" | "UA-71" | "UA-74" | "UA-77",
            ) => {}
            _ => return Err(PolicyError::UnknownLocation),
        }
    }
    Ok(())
}

pub fn check_placement(offer: &Offer, p: &PlacementInputs) -> Result<(), PolicyError> {
    check_location(offer)?;
    if !p.signed_m8s_image_verified {
        return Err(PolicyError::UnsignedImage);
    }
    if offer.inet_down_mbps < p.minimum_down_mbps.max(1000) {
        return Err(PolicyError::Bandwidth);
    }
    if offer.interruptible && !p.allow_interruptible {
        return Err(PolicyError::Interruptible);
    }
    if offer.hourly_micro_usd > p.max_instance_hourly {
        return Err(PolicyError::InstanceBudget);
    }
    if u128::from(p.committed_hourly) + u128::from(offer.hourly_micro_usd)
        > u128::from(p.max_total_hourly)
    {
        return Err(PolicyError::HourlyBudget);
    }
    if p.reserve_seconds == 0 {
        return Err(PolicyError::InvalidHorizon);
    }
    let cost = (u128::from(offer.hourly_micro_usd) * u128::from(p.reserve_seconds)).div_ceil(3600);
    if u128::from(p.monthly_spent_and_reserved) + cost > u128::from(p.monthly_limit) {
        return Err(PolicyError::MonthlyBudget);
    }
    Ok(())
}

/// History must contain this tier's committed launch intents (including uncertain
/// outcomes). Sliding window (now-3600, now]; future history fails closed.
pub fn check_launch_limit(
    now: Timestamp,
    tier_launches: &[Timestamp],
    limit: usize,
) -> Result<(), PolicyError> {
    let mut count = 0usize;
    for &t in tier_launches {
        if t > now {
            return Err(PolicyError::InvalidLaunchHistory);
        }
        if i128::from(now) - i128::from(t) < 3600 {
            count += 1;
        }
    }
    if count >= limit {
        Err(PolicyError::LaunchLimit)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LifecycleEvidence {
    pub certificate_verified: bool,
    pub model_served: bool,
    pub owner_routed_canary_passed: bool,
    /// Only an explicit provider absence observation sets this. Not stop/exit,
    /// timeout, transport failure, or successful submission of destroy.
    pub provider_confirmed_absent: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionError {
    InvalidEdge,
    ReadinessUnproven,
    AbsenceUnproven,
}

pub fn transition(from: Phase, to: Phase, e: LifecycleEvidence) -> Result<Phase, TransitionError> {
    use Phase::*;
    if to == Ready && !(e.certificate_verified && e.model_served && e.owner_routed_canary_passed) {
        return Err(TransitionError::ReadinessUnproven);
    }
    if to == Gone && !e.provider_confirmed_absent {
        return Err(TransitionError::AbsenceUnproven);
    }
    let valid = from == to
        || matches!(
            (from, to),
            (Pending, Searching)
                | (Searching, Launching)
                | (Launching, Booting)
                | (Booting, Bootstrapped)
                | (Bootstrapped, Loading)
                | (Loading, Ready)
                | (Ready, Draining)
                | (Draining, Destroying)
                | (Destroying, Gone)
                | (Searching, Paused)
                | (Paused, Searching)
                | (
                    Pending
                        | Searching
                        | Launching
                        | Booting
                        | Bootstrapped
                        | Loading
                        | Ready
                        | Draining,
                    Failed
                )
                | (
                    Pending
                        | Searching
                        | Launching
                        | Booting
                        | Bootstrapped
                        | Loading
                        | Ready
                        | Draining
                        | Failed
                        | Paused,
                    Orphaned
                )
                | (
                    Pending
                        | Searching
                        | Launching
                        | Booting
                        | Bootstrapped
                        | Loading
                        | Ready
                        | Failed
                        | Orphaned
                        | Paused,
                    Destroying
                )
        );
    if valid {
        Ok(to)
    } else {
        Err(TransitionError::InvalidEdge)
    }
}

/// Row deletion still needs fresh absence evidence, even for an existing Gone row.
pub fn may_delete(phase: Phase, e: LifecycleEvidence) -> bool {
    phase == Phase::Gone && e.provider_confirmed_absent
}

#[cfg(test)]
mod tests {
    use super::*;
    fn offer() -> Offer {
        Offer {
            id: "fixture".into(),
            country: Some("US".into()),
            region: None,
            inet_down_mbps: 1000,
            gpu_count: 1,
            vram_bytes_per_gpu: 24_000_000_000,
            hourly_micro_usd: 100,
            interruptible: false,
            reliability_bps: 9900,
        }
    }
    fn inputs() -> PlacementInputs {
        PlacementInputs {
            minimum_down_mbps: 1000,
            allow_interruptible: false,
            signed_m8s_image_verified: true,
            max_instance_hourly: 100,
            max_total_hourly: 200,
            committed_hourly: 100,
            monthly_limit: 1000,
            monthly_spent_and_reserved: 900,
            reserve_seconds: 3600,
        }
    }
    #[test]
    fn countries_fail_closed() {
        let mut o = offer();
        for c in [None, Some(""), Some("ZZ"), Some("us"), Some(" USA ")] {
            o.country = c.map(str::to_owned);
            assert_eq!(check_location(&o), Err(PolicyError::UnknownLocation));
        }
        for c in ["CN", "CU", "IR", "KP", "SY", "RU", "BY"] {
            o.country = Some(c.into());
            assert_eq!(check_location(&o), Err(PolicyError::DeniedLocation));
        }
        o.country = Some("UA".into());
        for r in [None, Some("Kyiv"), Some("UA-99")] {
            o.region = r.map(str::to_owned);
            assert_eq!(check_location(&o), Err(PolicyError::UnknownLocation));
        }
        for r in ["UA-43", "UA-14", "UA-09", "UA-23", "UA-65", "UA-40"] {
            o.region = Some(r.into());
            assert_eq!(check_location(&o), Err(PolicyError::DeniedLocation));
        }
        o.region = Some("UA-30".into());
        assert_eq!(check_location(&o), Ok(()));
    }
    #[test]
    fn placement_boundaries() {
        let mut o = offer();
        let mut p = inputs();
        assert_eq!(check_placement(&o, &p), Ok(()));
        o.inet_down_mbps = 999;
        assert_eq!(check_placement(&o, &p), Err(PolicyError::Bandwidth));
        p.minimum_down_mbps = 0;
        assert_eq!(check_placement(&o, &p), Err(PolicyError::Bandwidth));
        o.inet_down_mbps = 1000;
        o.interruptible = true;
        assert_eq!(check_placement(&o, &p), Err(PolicyError::Interruptible));
        p.allow_interruptible = true;
        assert_eq!(check_placement(&o, &p), Ok(()));
        p.signed_m8s_image_verified = false;
        assert_eq!(check_placement(&o, &p), Err(PolicyError::UnsignedImage));
    }
    #[test]
    fn budget_rounding_and_overflow() {
        let mut o = offer();
        let mut p = inputs();
        o.hourly_micro_usd = 101;
        assert_eq!(check_placement(&o, &p), Err(PolicyError::InstanceBudget));
        o.hourly_micro_usd = 100;
        p.committed_hourly = 101;
        assert_eq!(check_placement(&o, &p), Err(PolicyError::HourlyBudget));
        p.committed_hourly = 100;
        p.reserve_seconds = 3601;
        assert_eq!(check_placement(&o, &p), Err(PolicyError::MonthlyBudget));
        p.reserve_seconds = 0;
        assert_eq!(check_placement(&o, &p), Err(PolicyError::InvalidHorizon));
        o.hourly_micro_usd = u64::MAX;
        p.max_instance_hourly = u64::MAX;
        p.max_total_hourly = u64::MAX;
        p.committed_hourly = 1;
        assert_eq!(check_placement(&o, &p), Err(PolicyError::HourlyBudget));
        p.committed_hourly = 0;
        p.reserve_seconds = u64::MAX;
        p.monthly_limit = u64::MAX;
        assert_eq!(check_placement(&o, &p), Err(PolicyError::MonthlyBudget));
        o.hourly_micro_usd = 1;
        p.reserve_seconds = 1;
        p.monthly_limit = 901;
        assert_eq!(check_placement(&o, &p), Ok(()));
    }
    #[test]
    fn tier_hour_boundaries() {
        assert_eq!(check_launch_limit(3600, &[0, 1, 2], 3), Ok(()));
        assert_eq!(
            check_launch_limit(3600, &[1, 2, 3600], 3),
            Err(PolicyError::LaunchLimit)
        );
        assert_eq!(
            check_launch_limit(0, &[1], 3),
            Err(PolicyError::InvalidLaunchHistory)
        );
        assert_eq!(check_launch_limit(i64::MAX, &[i64::MIN], 3), Ok(()));
        assert_eq!(check_launch_limit(0, &[], 0), Err(PolicyError::LaunchLimit));
    }
    #[test]
    fn evidence_and_edges() {
        let mut e = LifecycleEvidence::default();
        for mask in 0..7 {
            e.certificate_verified = mask & 1 != 0;
            e.model_served = mask & 2 != 0;
            e.owner_routed_canary_passed = mask & 4 != 0;
            assert_eq!(
                transition(Phase::Loading, Phase::Ready, e),
                Err(TransitionError::ReadinessUnproven)
            );
        }
        e.certificate_verified = true;
        e.model_served = true;
        e.owner_routed_canary_passed = true;
        assert_eq!(
            transition(Phase::Loading, Phase::Ready, e),
            Ok(Phase::Ready)
        );
        assert_eq!(
            transition(Phase::Pending, Phase::Ready, e),
            Err(TransitionError::InvalidEdge)
        );
        assert_eq!(
            transition(Phase::Destroying, Phase::Gone, e),
            Err(TransitionError::AbsenceUnproven)
        );
        assert!(!may_delete(Phase::Gone, e));
        e.provider_confirmed_absent = true;
        assert_eq!(
            transition(Phase::Destroying, Phase::Gone, e),
            Ok(Phase::Gone)
        );
        assert!(may_delete(Phase::Gone, e));
        assert!(!may_delete(Phase::Destroying, e));
        assert_eq!(
            transition(Phase::Gone, Phase::Searching, e),
            Err(TransitionError::InvalidEdge)
        );
    }
    #[test]
    fn normal_lifecycle_and_cleanup() {
        use Phase::*;
        let phases = [
            Pending,
            Searching,
            Launching,
            Booting,
            Bootstrapped,
            Loading,
            Ready,
            Draining,
            Destroying,
            Gone,
        ];
        let e = LifecycleEvidence {
            certificate_verified: true,
            model_served: true,
            owner_routed_canary_passed: true,
            provider_confirmed_absent: true,
        };
        for pair in phases.windows(2) {
            assert_eq!(transition(pair[0], pair[1], e), Ok(pair[1]));
        }
        for p in [Failed, Orphaned, Paused] {
            assert_eq!(transition(p, Destroying, e), Ok(Destroying));
        }
    }
}

#[cfg(test)]
mod exhaustive_invariants {
    use super::*;
    #[test]
    fn no_phase_pair_can_bypass_readiness_or_absence_evidence() {
        use Phase::*;
        let phases = [
            Pending,
            Searching,
            Launching,
            Booting,
            Bootstrapped,
            Loading,
            Ready,
            Draining,
            Destroying,
            Gone,
            Failed,
            Orphaned,
            Paused,
        ];
        for from in phases {
            for to in phases {
                let e = LifecycleEvidence::default();
                let result = transition(from, to, e);
                if to == Ready {
                    assert_eq!(result, Err(TransitionError::ReadinessUnproven));
                }
                if to == Gone {
                    assert_eq!(result, Err(TransitionError::AbsenceUnproven));
                }
                assert!(!may_delete(from, e));
                if from == Gone && to != Gone {
                    assert!(result.is_err());
                }
            }
        }
    }
}
