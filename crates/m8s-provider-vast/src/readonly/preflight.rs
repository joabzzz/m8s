//! Fresh read-only account inventory and on-demand price qualification.
//! Refuses every existing instance/volume; no adoption, mutation or credential
//! discovery. Public normalized facts only. Prices are observations, not invoices.
use super::*;
use std::num::NonZeroU64;
#[derive(serde::Serialize)]
pub struct DemoPreflight {
    pub account_id: u64,
    pub observed_at: i64,
    pub offer_id: NonZeroU64,
    pub compute_micro_usd_per_hour: u64,
    pub storage_micro_usd_per_hour: u64,
    pub transfer_micro_usd_per_gb: u64,
    pub available_credit_micro_usd: u64,
}
pub struct PreflightHttps<S> {
    transport: ReadonlyHttps<S>,
    stage: &'static str,
}
impl<S: FnMut() -> Result<SecretValue>> PreflightHttps<S> {
    pub fn new(supplier: S) -> Result<Self> {
        Ok(Self {
            transport: ReadonlyHttps::new(supplier)?,
            stage: "vast-client-initialization",
        })
    }
    pub fn last_stage(&self) -> &'static str {
        self.stage
    }
    pub fn transport_failure(&self) -> Option<&'static str> {
        self.transport.backend.1
    }
    fn read(&mut self, path: &str, body: Option<Value>) -> Result<Value> {
        let secret = (self.transport.supplier)()?;
        let bytes = secret.expose();
        if bytes.is_empty() || bytes.len() > 4096 || !bytes.iter().all(|b| (33..=126).contains(b)) {
            return Err(Error::Unavailable);
        }
        let bearer = SecretValue::new([b"Bearer ".as_slice(), bytes].concat());
        let mut authorization =
            HeaderValue::from_bytes(bearer.expose()).map_err(|_| Error::Unavailable)?;
        authorization.set_sensitive(true);
        let method = if body.is_some() {
            reqwest::Method::POST
        } else {
            reqwest::Method::GET
        };
        let reply = self
            .transport
            .backend
            .send(WireRequest {
                method,
                url: format!("{ORIGIN}{path}"),
                authorization,
                body: body
                    .map(|v| serde_json::to_vec(&v).map_err(|_| Error::Unavailable))
                    .transpose()?,
            })
            .map_err(|_| Error::Unavailable)?;
        checked_json(reply, bytes, 256 * 1024)
    }
    pub fn observe(
        &mut self,
        expected_account: u64,
        selected_offer: Option<NonZeroU64>,
        disk_gb: u32,
    ) -> Result<DemoPreflight> {
        if expected_account == 0 || !(1..=20).contains(&disk_gb) {
            return Err(Error::Invalid);
        }
        self.stage = "vast-account-read";
        let account = self.read("/api/v0/users/current/", None)?;
        self.stage = "vast-account-identity";
        let account_id = account
            .get("id")
            .and_then(Value::as_u64)
            .ok_or(Error::Unavailable)?;
        if account_id != expected_account {
            return Err(Error::Unavailable);
        }
        self.stage = "vast-credit-check";
        let credit = number(&account, "credit", 1_000_000, false)?;
        if credit < 6_000_000 {
            return Err(Error::Unavailable);
        }
        self.stage = "vast-complete-inventory";
        let inventory = self.read(
            "/api/v1/instances/?limit=1&select_cols=%5B%22id%22%5D",
            None,
        )?;
        if inventory.get("success") != Some(&Value::Bool(true))
            || inventory
                .get("instances")
                .and_then(Value::as_array)
                .is_none_or(|v| !v.is_empty())
            || inventory.get("next_token") != Some(&Value::Null)
            || inventory.get("instances_found").and_then(Value::as_u64) != Some(0)
            || inventory.get("total_instances").and_then(Value::as_u64) != Some(0)
        {
            return Err(Error::Unavailable);
        }
        self.stage = "vast-volume-inventory";
        let volumes = self.read("/api/v0/volumes/?owner=me&type=all_volume", None)?;
        if volumes
            .get("volumes")
            .and_then(Value::as_array)
            .is_none_or(|v| !v.is_empty())
        {
            return Err(Error::Unavailable);
        }
        let query = json!({"limit":offer_read_limit(selected_offer),"type":"on-demand","verified":{"eq":true},"rentable":{"eq":true},"rented":{"eq":false},"cpu_cores_effective":{"gte":4},"cpu_ram":{"gte":8192},"disk_space":{"gte":disk_gb},"direct_port_count":{"gte":2},"dph_total":{"lte":0.30},"order":[["dph_total","asc"]]});
        // The API's `id` search filter does not address the returned bundle ID:
        // live observation returned no rows for an ID present in the same host
        // search. Keep a bounded catalog read and match the exact bundle locally.
        // Never substitute a different offer when a selected one is unavailable.
        self.stage = "vast-offer-read";
        let offers = self.read("/api/v0/bundles/", Some(query))?;
        self.stage = "vast-offer-price-qualification";
        let rows = offers
            .get("offers")
            .and_then(Value::as_array)
            .filter(|r| r.len() <= offer_read_limit(selected_offer))
            .ok_or(Error::Unavailable)?;
        let mut candidates = Vec::new();
        for row in rows {
            if let Ok(candidate) = qualify(row, disk_gb) {
                candidates.push(candidate);
            }
        }
        candidates.sort_by_key(|v| v.1 + v.2);
        let (offer_id, compute, storage, transfer) = candidates
            .into_iter()
            .find(|v| selected_offer.is_none_or(|id| id == v.0))
            .ok_or(Error::Unavailable)?;
        let observed_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|v| i64::try_from(v.as_secs()).ok())
            .ok_or(Error::Unavailable)?;
        Ok(DemoPreflight {
            account_id,
            observed_at,
            offer_id,
            compute_micro_usd_per_hour: compute,
            storage_micro_usd_per_hour: storage,
            transfer_micro_usd_per_gb: transfer,
            available_credit_micro_usd: credit,
        })
    }
}
fn offer_read_limit(selected: Option<NonZeroU64>) -> usize {
    if selected.is_some() { 100 } else { 20 }
}

// Decimal dollars -> micro-USD, always round costs up. Reject exponent syntax,
// negative values, overflow and non-finite provider values.
fn micros(v: &Value) -> Result<u64> {
    let text = if let Some(s) = v.as_str() {
        s.to_owned()
    } else {
        v.to_string()
    };
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    if whole.is_empty()
        || whole.len() > 9
        || fraction.len() > 18
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(Error::Unavailable);
    }
    let whole: u64 = whole.parse().map_err(|_| Error::Unavailable)?;
    let mut frac = fraction.chars().take(6).collect::<String>();
    while frac.len() < 6 {
        frac.push('0');
    }
    let extra = u64::from(fraction.bytes().skip(6).any(|b| b != b'0'));
    whole
        .checked_mul(1_000_000)
        .and_then(|n| n.checked_add(frac.parse::<u64>().ok()?))
        .and_then(|n| n.checked_add(extra))
        .ok_or(Error::Unavailable)
}
fn qualify(row: &Value, disk: u32) -> Result<(NonZeroU64, u64, u64, u64)> {
    let get = |key| row.get(key).ok_or(Error::Unavailable);
    let id = get("id")?
        .as_u64()
        .and_then(NonZeroU64::new)
        .ok_or(Error::Unavailable)?;
    let compute = micros(get("dph_total")?)?;
    // The CLI describes storage_cost per GB/month while billing docs also
    // describe hourly storage. Reserve the larger hourly interpretation; never
    // divide an ambiguous provider rate and understate the all-in liability.
    let storage = micros(get("storage_cost")?)?
        .checked_mul(disk.into())
        .ok_or(Error::Unavailable)?;
    let transfer = micros(get("inet_up_cost")?)?.max(micros(get("inet_down_cost")?)?);
    if compute == 0
        || compute > 300_000
        || storage > 2_000_000
        || transfer > 100_000
        || get("rentable")?.as_bool() != Some(true)
        || get("rented")?.as_bool() != Some(false)
        || row.get("verification").and_then(Value::as_str) != Some("verified")
    {
        return Err(Error::Unavailable);
    }
    Ok((id, compute, storage, transfer))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_offer_catalog_read_remains_bounded() {
        assert_eq!(offer_read_limit(None), 20);
        assert_eq!(offer_read_limit(NonZeroU64::new(42)), 100);
    }
    #[test]
    fn decimal_cost_rounds_up_without_float_arithmetic() {
        for (s, n) in [
            ("0", 0),
            ("0.0000001", 1),
            ("0.125", 125000),
            ("6.24524799", 6245248),
        ] {
            assert_eq!(micros(&json!(s)).unwrap(), n);
        }
        for s in ["-1", "NaN", "1e-4", "1.2.3", ""] {
            assert!(micros(&json!(s)).is_err());
        }
    }
}
