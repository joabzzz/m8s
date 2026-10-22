//! Private shared field validators; no public authority surface.
use crate::ConfigTierBounds;
use serde::Deserialize;
pub(super) fn config_status_error<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}
pub(super) fn valid_bounds(tiers: &[ConfigTierBounds]) -> bool {
    if tiers.len() > 32 {
        return false;
    }
    let mut previous = "";
    for tier in tiers {
        if tier.id.is_empty()
            || tier.id.len() > 48
            || tier.id.as_str() <= previous
            || !tier
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || tier.min > tier.max
            || tier.max > 16
        {
            return false;
        }
        previous = &tier.id;
    }
    true
}

pub(super) fn diagnostic_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
