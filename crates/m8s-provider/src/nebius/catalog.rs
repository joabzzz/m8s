//! Conservative catalog snapshot: USD PAYG GPU-bundled presets, 2026-10-23.
//! Excludes L40S (separately charged CPU/RAM), spot, discounts, taxes and storage.
use super::{Config, component, wire};
use crate::{Error, Result};
use m8s_core::Offer;
use std::collections::{BTreeMap, BTreeSet};

struct Family {
    platform: &'static str,
    regions: &'static [&'static str],
    presets: [&'static str; 2],
    vram_gb: u64,
    per_gpu_micro_usd: u64,
}
const FAMILIES: &[Family] = &[
    Family {
        platform: "gpu-h100-sxm",
        regions: &["eu-north1"],
        presets: ["1gpu-16vcpu-200gb", "8gpu-128vcpu-1600gb"],
        vram_gb: 80,
        per_gpu_micro_usd: 4_500_000,
    },
    Family {
        platform: "gpu-h200-sxm",
        regions: &["eu-north1", "eu-north2", "eu-west1", "us-central1"],
        presets: ["1gpu-16vcpu-200gb", "8gpu-128vcpu-1600gb"],
        vram_gb: 141,
        per_gpu_micro_usd: 5_400_000,
    },
    Family {
        platform: "gpu-b200-sxm",
        regions: &["us-central1"],
        presets: ["1gpu-20vcpu-224gb", "8gpu-160vcpu-1792gb"],
        vram_gb: 180,
        per_gpu_micro_usd: 8_500_000,
    },
    Family {
        platform: "gpu-b200-sxm-a",
        regions: &["me-west1"],
        presets: ["1gpu-20vcpu-224gb", "8gpu-160vcpu-1792gb"],
        vram_gb: 180,
        per_gpu_micro_usd: 8_500_000,
    },
    Family {
        platform: "gpu-b300-sxm",
        regions: &["uk-south1", "eu-west2", "us-north1"],
        presets: ["1gpu-24vcpu-346gb", "8gpu-192vcpu-2768gb"],
        vram_gb: 288,
        per_gpu_micro_usd: 9_500_000,
    },
    Family {
        platform: "gpu-rtx6000",
        regions: &["us-central1"],
        presets: ["1gpu-24vcpu-218gb", "8gpu-192vcpu-1744gb"],
        vram_gb: 96,
        per_gpu_micro_usd: 1_800_000,
    },
    Family {
        platform: "gpu-rtx6000-a",
        regions: &["uk-south2", "eu-south1"],
        presets: ["1gpu-24vcpu-218gb", "8gpu-192vcpu-1744gb"],
        vram_gb: 96,
        per_gpu_micro_usd: 1_800_000,
    },
];
pub(super) fn country(region: &str) -> Result<&'static str> {
    match region {
        "eu-north1" => Ok("FI"),
        "eu-north2" => Ok("IS"),
        "eu-west1" | "eu-west2" => Ok("FR"),
        "eu-south1" => Ok("ES"),
        "us-central1" | "us-north1" => Ok("US"),
        "me-west1" => Ok("IL"),
        "uk-south1" | "uk-south2" => Ok("GB"),
        _ => Err(Error::Invalid),
    }
}
fn make_offer(region: &str, family: &Family, index: usize) -> Result<Offer> {
    let count = if index == 0 { 1 } else { 8 };
    Ok(Offer {
        id: format!("{region}/{}/{}", family.platform, family.presets[index]),
        country: Some(country(region)?.into()),
        region: Some(region.into()),
        // NIC line rate isn't measured Internet throughput; unknown is zero.
        inet_down_mbps: 0,
        reliability_bps: 0,
        gpu_count: count,
        vram_bytes_per_gpu: family.vram_gb * 1_000_000_000,
        hourly_micro_usd: family.per_gpu_micro_usd * u64::from(count),
        interruptible: false,
    })
}
pub(super) fn resources<'a>(region: &str, id: &'a str) -> Result<(&'a str, &'a str)> {
    let parts: Vec<_> = id.split('/').collect();
    if parts.len() != 3 || parts[0] != region || !component(parts[1]) || !component(parts[2]) {
        return Err(Error::Invalid);
    }
    Ok((parts[1], parts[2]))
}
pub(super) fn validate_offer(config: &Config, offer: &Offer) -> Result<()> {
    for family in FAMILIES
        .iter()
        .filter(|f| f.regions.contains(&config.region.as_str()))
    {
        for index in 0..2 {
            if make_offer(&config.region, family, index)? == *offer {
                return Ok(());
            }
        }
    }
    Err(Error::Invalid)
}
pub(super) fn offers(
    config: &Config,
    platforms: Vec<wire::Platform>,
    quotas: Vec<wire::Quota>,
) -> Result<Vec<Offer>> {
    let mut available = BTreeMap::new();
    for quota in quotas {
        if quota.metadata.parent_id != config.project {
            return Err(Error::Invalid);
        }
        if quota.spec.region != config.region {
            continue;
        }
        let remaining = if quota.status.state == "STATE_ACTIVE"
            && matches!(
                quota.status.usage_state.as_str(),
                "USAGE_STATE_USED" | "USAGE_STATE_NOT_USED"
            ) {
            let limit = quota
                .spec
                .limit
                .parse::<u64>()
                .map_err(|_| Error::Invalid)?;
            let usage = quota
                .status
                .usage
                .parse::<u64>()
                .map_err(|_| Error::Invalid)?;
            limit.saturating_sub(usage)
        } else {
            0
        };
        if available.insert(quota.metadata.name, remaining).is_some() {
            return Err(Error::Invalid);
        }
    }
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for platform in platforms {
        if platform.metadata.parent_id != config.project
            || !seen.insert(platform.metadata.name.clone())
        {
            return Err(Error::Invalid);
        }
        let Some(family) = FAMILIES.iter().find(|f| {
            f.platform == platform.metadata.name && f.regions.contains(&config.region.as_str())
        }) else {
            continue;
        };
        // The API supplies the quota name; do not guess a GPU model -> quota mapping.
        let remaining = available
            .get(&platform.spec.gpu_count_quota_type)
            .copied()
            .unwrap_or(0);
        let mut presets = BTreeSet::new();
        for preset in platform.spec.presets {
            if !presets.insert(preset.name.clone()) {
                return Err(Error::Invalid);
            }
            let Some(index) = family.presets.iter().position(|p| *p == preset.name) else {
                continue;
            };
            let offer = make_offer(&config.region, family, index)?;
            if offer.gpu_count != preset.resources.gpu_count {
                return Err(Error::Invalid);
            }
            if u64::from(offer.gpu_count) <= remaining {
                m8s_core::policy::check_location(&offer).map_err(|_| Error::Invalid)?;
                result.push(offer);
            }
        }
    }
    result.sort_by(|a, b| {
        a.hourly_micro_usd
            .cmp(&b.hourly_micro_usd)
            .then(a.id.cmp(&b.id))
    });
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_catalog_region_has_policy_checked_country_and_exact_multi_gpu_price() {
        for family in FAMILIES {
            for region in family.regions {
                for index in 0..2 {
                    let offer = make_offer(region, family, index).unwrap();
                    assert!(m8s_core::policy::check_location(&offer).is_ok());
                    assert_eq!(
                        offer.hourly_micro_usd,
                        family.per_gpu_micro_usd * u64::from(offer.gpu_count)
                    );
                    assert_eq!(
                        resources(region, &offer.id).unwrap(),
                        (family.platform, family.presets[index])
                    );
                    let config = Config {
                        project: "project-fixture".into(),
                        owner: "owner-fixture".into(),
                        region: (*region).into(),
                    };
                    assert!(validate_offer(&config, &offer).is_ok());
                }
            }
        }
        for (region, expected) in [
            ("eu-north1", "FI"),
            ("eu-north2", "IS"),
            ("eu-west1", "FR"),
            ("eu-south1", "ES"),
            ("me-west1", "IL"),
            ("us-central1", "US"),
            ("uk-south1", "GB"),
        ] {
            assert_eq!(country(region).unwrap(), expected);
        }
        assert!(country("eu-unknown").is_err());
    }
}
