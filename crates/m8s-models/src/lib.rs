//! Owner-selected model fit arithmetic, not recommendations or runtime guarantees.
//! All capacities are bytes (never implicit GB/GiB). Profiles and KV estimates
//! must be validated against the selected runtime before real deployment.
use m8s_core::Offer;
use serde::{Deserialize, Serialize};

/// Illustrative, untested profiles; no model is selected automatically. Consumers
/// can deserialize this TOML catalog or supply their own ModelProfile values.
pub const EXAMPLE_CATALOG: &str = include_str!("../models/examples.toml");

/// TOML catalog shape: `[[models]]`. Parsing is left to the application.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCatalog {
    pub models: Vec<ModelProfile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelProfile {
    pub id: String,
    /// Owner supplies a pinned source before deployment. Example URIs are not downloads.
    pub source: String,
    pub params_billions: u32,
    pub quant: String,
    pub weights_bytes: u64,
    pub runtime_overhead_bytes_per_replica: u64,
    /// Estimated per-sequence KV bytes per 1024 tokens. Linear approximation.
    pub kv_bytes_per_1024_tokens: u64,
    pub max_context_tokens: u64,
    pub license: String,
    pub license_class: String,
    pub runtime_status: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// (a) As many independent replicas per GPU as fit with requested minimum KV.
    A,
    /// (b) One independent replica per GPU; weights are not shared across GPUs.
    B,
    /// (c) One replica sharded across all GPUs on ONE host, no cross-host split.
    /// Assumes balanced sharding; actual runtime support remains a separate gate.
    C,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanRequest {
    pub layout: Layout,
    pub context_tokens: u64,
    pub minimum_sequences_per_replica: u64,
    /// Driver/runtime reservations in addition to per-replica overhead.
    pub reserved_bytes_per_gpu: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FitPlan {
    pub layout: Layout,
    pub replicas: u64,
    pub gpus_per_replica: u32,
    pub weights_and_overhead_bytes_per_replica: u64,
    pub kv_bytes_per_sequence: u64,
    /// Equal division in (a), full remaining capacity in (b)/(c).
    pub kv_headroom_bytes_per_replica: u64,
    pub concurrent_sequences_per_replica: u64,
    pub total_concurrent_sequences: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanError {
    InvalidProfile,
    InvalidRequest,
    NoCapacity,
    DoesNotFit,
    Overflow,
    InvalidBandwidth,
}
fn narrow(value: u128) -> Result<u64, PlanError> {
    value.try_into().map_err(|_| PlanError::Overflow)
}

pub fn plan(
    profile: &ModelProfile,
    offer: &Offer,
    request: PlanRequest,
) -> Result<FitPlan, PlanError> {
    if profile.weights_bytes == 0
        || profile.kv_bytes_per_1024_tokens == 0
        || profile.max_context_tokens == 0
    {
        return Err(PlanError::InvalidProfile);
    }
    if request.context_tokens == 0
        || request.context_tokens > profile.max_context_tokens
        || request.minimum_sequences_per_replica == 0
    {
        return Err(PlanError::InvalidRequest);
    }
    if offer.gpu_count == 0 {
        return Err(PlanError::NoCapacity);
    }
    let available = offer
        .vram_bytes_per_gpu
        .checked_sub(request.reserved_bytes_per_gpu)
        .filter(|v| *v > 0)
        .ok_or(PlanError::NoCapacity)?;
    let base = narrow(
        u128::from(profile.weights_bytes) + u128::from(profile.runtime_overhead_bytes_per_replica),
    )?;
    let kv = narrow(
        (u128::from(profile.kv_bytes_per_1024_tokens) * u128::from(request.context_tokens))
            .div_ceil(1024),
    )?;
    let minimum =
        u128::from(base) + u128::from(kv) * u128::from(request.minimum_sequences_per_replica);
    let (replicas, gpus_per_replica, headroom) = match request.layout {
        Layout::A => {
            let per_gpu = narrow(u128::from(available) / minimum)?;
            if per_gpu == 0 {
                return Err(PlanError::DoesNotFit);
            }
            (
                narrow(u128::from(per_gpu) * u128::from(offer.gpu_count))?,
                1,
                available / per_gpu - base,
            )
        }
        Layout::B => {
            if minimum > u128::from(available) {
                return Err(PlanError::DoesNotFit);
            }
            (u64::from(offer.gpu_count), 1, available - base)
        }
        Layout::C => {
            // Rounded per-GPU requirement must fit, not just weights on GPU 0.
            if minimum.div_ceil(u128::from(offer.gpu_count)) > u128::from(available) {
                return Err(PlanError::DoesNotFit);
            }
            let total = narrow(u128::from(available) * u128::from(offer.gpu_count))?;
            (1, offer.gpu_count, total - base)
        }
    };
    let sequences = headroom / kv;
    Ok(FitPlan {
        layout: request.layout,
        replicas,
        gpus_per_replica,
        weights_and_overhead_bytes_per_replica: base,
        kv_bytes_per_sequence: kv,
        kv_headroom_bytes_per_replica: headroom,
        concurrent_sequences_per_replica: sequences,
        total_concurrent_sequences: narrow(u128::from(sequences) * u128::from(replicas))?,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DownloadEstimate {
    pub seconds: u64,
    /// Compute rental estimate only; excludes storage, transfer, image and load.
    pub compute_micro_usd: u64,
}
/// One shared host download, independent of replica count. Efficiency is explicit
/// basis points in 1..=10000 (e.g. 7000 for 70% of advertised Mbps). Seconds and
/// compute cost round UP. Does not initiate downloads or claim measured speed.
pub fn estimate_download(
    bytes: u64,
    advertised_mbps: u64,
    efficiency_bps: u16,
    hourly_micro_usd: u64,
) -> Result<DownloadEstimate, PlanError> {
    if advertised_mbps == 0 || efficiency_bps == 0 || efficiency_bps > 10000 {
        return Err(PlanError::InvalidBandwidth);
    }
    let bits_scaled = u128::from(bytes) * 8 * 10000;
    let rate = u128::from(advertised_mbps) * 1_000_000 * u128::from(efficiency_bps);
    let seconds = narrow(bits_scaled.div_ceil(rate))?;
    let cost = narrow((u128::from(hourly_micro_usd) * u128::from(seconds)).div_ceil(3600))?;
    Ok(DownloadEstimate {
        seconds,
        compute_micro_usd: cost,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile() -> ModelProfile {
        ModelProfile {
            id: "synthetic".into(),
            source: "fixture://only".into(),
            params_billions: 27,
            quant: "example".into(),
            weights_bytes: 16,
            runtime_overhead_bytes_per_replica: 2,
            kv_bytes_per_1024_tokens: 2,
            max_context_tokens: 8192,
            license: "unknown".into(),
            license_class: "unknown".into(),
            runtime_status: "untested".into(),
        }
    }
    fn offer() -> Offer {
        Offer {
            id: "fixture".into(),
            country: Some("US".into()),
            region: None,
            inet_down_mbps: 1000,
            gpu_count: 2,
            vram_bytes_per_gpu: 80,
            hourly_micro_usd: 3600,
            interruptible: false,
            reliability_bps: 9900,
        }
    }
    fn request(layout: Layout) -> PlanRequest {
        PlanRequest {
            layout,
            context_tokens: 1024,
            minimum_sequences_per_replica: 1,
            reserved_bytes_per_gpu: 0,
        }
    }
    #[test]
    fn layouts_and_kv() {
        let p = profile();
        let o = offer();
        let a = plan(&p, &o, request(Layout::A)).unwrap();
        assert_eq!(
            (
                a.replicas,
                a.kv_headroom_bytes_per_replica,
                a.total_concurrent_sequences
            ),
            (8, 2, 8)
        );
        let b = plan(&p, &o, request(Layout::B)).unwrap();
        assert_eq!(
            (
                b.replicas,
                b.kv_headroom_bytes_per_replica,
                b.total_concurrent_sequences
            ),
            (2, 62, 62)
        );
        let c = plan(&p, &o, request(Layout::C)).unwrap();
        assert_eq!(
            (
                c.replicas,
                c.gpus_per_replica,
                c.kv_headroom_bytes_per_replica,
                c.total_concurrent_sequences
            ),
            (1, 2, 142, 71)
        );
    }
    #[test]
    fn exact_fit_and_one_byte_short() {
        let p = profile();
        let mut o = offer();
        o.vram_bytes_per_gpu = 20;
        for layout in [Layout::A, Layout::B] {
            assert!(plan(&p, &o, request(layout)).is_ok());
            o.vram_bytes_per_gpu = 19;
            assert_eq!(plan(&p, &o, request(layout)), Err(PlanError::DoesNotFit));
            o.vram_bytes_per_gpu = 20;
        }
        o.vram_bytes_per_gpu = 10;
        assert!(plan(&p, &o, request(Layout::C)).is_ok());
        o.vram_bytes_per_gpu = 9;
        assert_eq!(plan(&p, &o, request(Layout::C)), Err(PlanError::DoesNotFit));
    }
    #[test]
    fn kv_rounding_context_and_reservations() {
        let p = profile();
        let o = offer();
        let mut r = request(Layout::B);
        r.context_tokens = 1025;
        assert_eq!(plan(&p, &o, r).unwrap().kv_bytes_per_sequence, 3);
        r.context_tokens = 8192;
        assert!(plan(&p, &o, r).is_ok());
        r.context_tokens = 8193;
        assert_eq!(plan(&p, &o, r), Err(PlanError::InvalidRequest));
        r.context_tokens = 0;
        assert_eq!(plan(&p, &o, r), Err(PlanError::InvalidRequest));
        r.context_tokens = 1024;
        r.minimum_sequences_per_replica = 0;
        assert_eq!(plan(&p, &o, r), Err(PlanError::InvalidRequest));
        r.minimum_sequences_per_replica = 1;
        r.reserved_bytes_per_gpu = 61;
        assert_eq!(plan(&p, &o, r), Err(PlanError::DoesNotFit));
        r.reserved_bytes_per_gpu = 81;
        assert_eq!(plan(&p, &o, r), Err(PlanError::NoCapacity));
    }
    #[test]
    fn invalid_and_extreme_inputs() {
        let mut p = profile();
        let mut o = offer();
        o.gpu_count = 0;
        assert_eq!(plan(&p, &o, request(Layout::A)), Err(PlanError::NoCapacity));
        o.gpu_count = 2;
        p.weights_bytes = 0;
        assert_eq!(
            plan(&p, &o, request(Layout::A)),
            Err(PlanError::InvalidProfile)
        );
        p.weights_bytes = u64::MAX;
        assert_eq!(plan(&p, &o, request(Layout::A)), Err(PlanError::Overflow));
        p = profile();
        o.vram_bytes_per_gpu = u64::MAX;
        assert_eq!(plan(&p, &o, request(Layout::C)), Err(PlanError::Overflow));
        p.weights_bytes = 1;
        p.runtime_overhead_bytes_per_replica = 0;
        p.kv_bytes_per_1024_tokens = 1;
        o.gpu_count = u32::MAX;
        assert_eq!(plan(&p, &o, request(Layout::A)), Err(PlanError::Overflow));
    }
    #[test]
    fn example_classes_are_fit_math_not_selection() {
        let mut p = profile();
        let mut o = offer();
        p.weights_bytes = 17_000_000_000;
        p.runtime_overhead_bytes_per_replica = 2_000_000_000;
        p.kv_bytes_per_1024_tokens = 64_000_000;
        o.vram_bytes_per_gpu = 24_000_000_000;
        let mut r = request(Layout::B);
        r.context_tokens = 8192;
        let b = plan(&p, &o, r).unwrap();
        assert_eq!((b.replicas, b.concurrent_sequences_per_replica), (2, 9));
        p.weights_bytes = 42_000_000_000;
        p.kv_bytes_per_1024_tokens = 320_000_000;
        assert_eq!(plan(&p, &o, r), Err(PlanError::DoesNotFit));
        o.vram_bytes_per_gpu = 48_000_000_000;
        let b = plan(&p, &o, r).unwrap();
        assert_eq!((b.replicas, b.concurrent_sequences_per_replica), (2, 1));
        o.gpu_count = 1;
        o.vram_bytes_per_gpu = 80_000_000_000;
        r.layout = Layout::C;
        assert_eq!(
            plan(&p, &o, r).unwrap().concurrent_sequences_per_replica,
            14
        );
    }
    #[test]
    fn download_math_and_boundaries() {
        assert_eq!(
            estimate_download(125_000_000, 1000, 10000, 3600),
            Ok(DownloadEstimate {
                seconds: 1,
                compute_micro_usd: 1
            })
        );
        assert_eq!(
            estimate_download(125_000_001, 1000, 10000, 3600)
                .unwrap()
                .seconds,
            2
        );
        assert_eq!(
            estimate_download(125_000_000, 1000, 7000, 1).unwrap(),
            DownloadEstimate {
                seconds: 2,
                compute_micro_usd: 1
            }
        );
        assert_eq!(
            estimate_download(0, 1, 1, u64::MAX),
            Ok(DownloadEstimate {
                seconds: 0,
                compute_micro_usd: 0
            })
        );
        for (rate, eff) in [(0, 10000), (1, 0), (1, 10001)] {
            assert_eq!(
                estimate_download(1, rate, eff, 1),
                Err(PlanError::InvalidBandwidth)
            );
        }
        assert!(estimate_download(u64::MAX, u64::MAX, 10000, 1).is_ok());
        assert_eq!(
            estimate_download(u64::MAX, 1, 1, u64::MAX),
            Err(PlanError::Overflow)
        );
    }
}
