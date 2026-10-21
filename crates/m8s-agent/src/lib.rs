//! Deterministic DMS and local subprocess supervision. Secure transport and provider
//! self-destruction are intentionally unavailable; local process exit is not absence.
//! The caller must supply an iroh-authenticated endpoint identity, never a claimed wire field.
use m8s_core::Timestamp;
pub mod supervisor;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Drain,
    Wipe,
    Destroy,
    ConfirmAbsent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Failure,
    ConfirmedAbsent,
}
pub struct DeadManSwitch {
    acl: Vec<String>,
    started: Timestamp,
    last_heartbeat: Timestamp,
    last_tick: Timestamp,
    ttl: i64,
    max_lifetime: i64,
    triggered: bool,
    drained: bool,
    wiped: bool,
    gone: bool,
}
impl DeadManSwitch {
    pub fn new(
        owner: String,
        mut delegates: Vec<String>,
        now: Timestamp,
        ttl: i64,
        max_lifetime: i64,
    ) -> Result<Self, &'static str> {
        if owner.is_empty() || ttl <= 0 || max_lifetime <= 0 {
            return Err("invalid DMS policy");
        }
        delegates.push(owner);
        Ok(Self {
            acl: delegates,
            started: now,
            last_heartbeat: now,
            last_tick: now,
            ttl,
            max_lifetime,
            triggered: false,
            drained: false,
            wiped: false,
            gone: false,
        })
    }
    /// Latch teardown on shutdown or child failure; it cannot be undone by heartbeats.
    pub fn trigger(&mut self) {
        self.triggered = true;
    }
    pub fn heartbeat(&mut self, authenticated_peer: &str, now: Timestamp) -> bool {
        if self.triggered
            || now < self.last_tick
            || now < self.last_heartbeat
            || !self.acl.iter().any(|p| p == authenticated_peer)
        {
            return false;
        }
        // Heartbeats arriving at/after the deadline cannot revive an expired lease.
        if now.saturating_sub(self.last_heartbeat) >= self.ttl
            || now.saturating_sub(self.started) >= self.max_lifetime
        {
            self.triggered = true;
            return false;
        }
        self.last_heartbeat = now;
        true
    }
    pub fn tick(&mut self, now: Timestamp) -> Vec<Effect> {
        self.last_tick = self.last_tick.max(now);
        let now = self.last_tick;
        self.triggered |= now.saturating_sub(self.last_heartbeat) >= self.ttl
            || now.saturating_sub(self.started) >= self.max_lifetime;
        if !self.triggered || self.gone {
            return vec![];
        }
        let mut effects = vec![];
        if !self.drained {
            effects.push(Effect::Drain);
        }
        if !self.wiped {
            effects.push(Effect::Wipe);
        }
        // Failure to drain or wipe must not postpone spend containment.
        effects.extend([Effect::Destroy, Effect::ConfirmAbsent]);
        effects
    }
    pub fn complete(&mut self, effect: Effect, outcome: Outcome) {
        if !self.triggered {
            return;
        }
        match (effect, outcome) {
            (Effect::Drain, Outcome::Success) => self.drained = true,
            (Effect::Wipe, Outcome::Success) => self.wiped = true,
            (Effect::ConfirmAbsent, Outcome::ConfirmedAbsent) => self.gone = true,
            _ => {}
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn acl_only_heartbeats_and_deadline_latches() {
        let mut d = DeadManSwitch::new("owner".into(), vec![], 0, 3600, 43200).unwrap();
        assert!(!d.heartbeat("inference-peer", 3599));
        assert!(d.tick(3599).is_empty());
        assert_eq!(
            d.tick(3600),
            vec![
                Effect::Drain,
                Effect::Wipe,
                Effect::Destroy,
                Effect::ConfirmAbsent
            ]
        );
        assert!(!d.heartbeat("owner", 3601));
        assert!(!d.tick(3602).is_empty());
    }
    #[test]
    fn hard_lifetime_and_retry_independent_of_cleanup() {
        let mut d =
            DeadManSwitch::new("owner".into(), vec!["delegate".into()], 0, 100, 200).unwrap();
        assert!(d.heartbeat("delegate", 99));
        assert!(d.heartbeat("owner", 198));
        assert!(!d.heartbeat("owner", 200));
        let effects = d.tick(200);
        assert!(effects.contains(&Effect::Destroy));
        d.complete(Effect::Drain, Outcome::Failure);
        d.complete(Effect::Wipe, Outcome::Failure);
        d.complete(Effect::Destroy, Outcome::Success);
        assert_eq!(d.tick(201), effects);
        d.complete(Effect::Drain, Outcome::Success);
        d.complete(Effect::Wipe, Outcome::Success);
        assert_eq!(d.tick(202), vec![Effect::Destroy, Effect::ConfirmAbsent]);
        d.complete(Effect::ConfirmAbsent, Outcome::ConfirmedAbsent);
        assert!(d.tick(203).is_empty());
    }
    #[test]
    fn backwards_clock_cannot_extend_lease() {
        let mut d = DeadManSwitch::new("owner".into(), vec![], 10, 10, 100).unwrap();
        assert!(d.tick(19).is_empty());
        assert!(!d.heartbeat("owner", 18));
        assert!(!d.tick(20).is_empty());
    }
}
