//! Fixed demo profiles. Native download selection and served identity are distinct.
#[derive(Clone, Copy, Default, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum DemoModel {
    #[default]
    Qwen,
    Flint,
    #[serde(rename = "qwen3.8-27b")]
    Qwen38,
}
impl DemoModel {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Qwen => "qwen2.5-0.5b-instruct-q4_k_m",
            Self::Flint => m8s_provider::vast::provision::FLINT_MODEL_REF,
            Self::Qwen38 => m8s_provider::vast::provision::QWEN38_MODEL_REF,
        }
    }
    pub(super) fn gguf(self) -> Option<String> {
        match self {
            Self::Qwen => Some(format!("/models/{}.gguf", self.label())),
            Self::Flint | Self::Qwen38 => None,
        }
    }
    pub(super) fn served(self) -> &'static str {
        match self {
            Self::Qwen => {
                "local-gguf/sha256-74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db"
            }
            Self::Qwen38 => {
                "local-gguf/sha256-322e194ff79741c7baa497c240f677f54b201b0efab44ca8e50f122b39123482"
            }
            Self::Flint => {
                "local-gguf/sha256-97cc8009ff9b132d5cf3c488e6767962bb5a5dc158a90ed661bbbddf876b8bde"
            }
        }
    }
    pub(super) fn completion_request(self) -> serde_json::Value {
        let mut request = serde_json::json!({
            "model": self.served(),
            "messages": [{"role": "user", "content": "Reply with a short sentence explaining what a mesh network is."}],
            "stream": false,
            "max_tokens": 48
        });
        if matches!(self, Self::Qwen38) {
            request["chat_template_kwargs"] = serde_json::json!({"enable_thinking": false});
        }
        request
    }
    pub(super) fn accepts_disk(self, disk_gb: u32) -> bool {
        !matches!(self, Self::Qwen38) || disk_gb >= 64
    }
    pub(super) fn transfer_reserve_gb(self, disk_gb: u32) -> u64 {
        match self {
            // Cover native model/cache traffic, metadata, image pull and
            // transfer framing separately from the disk allocation. The pinned
            // non-sharded artifact has one 16.465 GB file. 64 GB covers two
            // full download paths plus the <13 GB unpacked image and >18 GB headroom;
            // native retries and draft-model downloads are disabled.
            Self::Qwen38 => 64,
            Self::Qwen | Self::Flint => u64::from(disk_gb),
        }
    }
    pub(super) fn requirements(self) -> m8s_provider::vast::readonly::preflight::Requirements {
        match self {
            Self::Qwen => Default::default(),
            Self::Qwen38 => m8s_provider::vast::readonly::preflight::Requirements {
                max_compute_hourly: 2_000_000,
                max_transfer_micro_usd_per_gb: 4000,
                min_gpu_memory_mib: 48000,
                min_download_mbps: 500,
                blackwell_sm120: true,
            },
            Self::Flint => m8s_provider::vast::readonly::preflight::Requirements {
                max_compute_hourly: 3_000_000,
                max_transfer_micro_usd_per_gb: 100_000,
                min_gpu_memory_mib: 48000,
                min_download_mbps: 4000,
                blackwell_sm120: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_model_is_opt_in_and_never_the_historical_fixture() {
        let model: DemoModel = serde_json::from_str("\"qwen3.8-27b\"").unwrap();
        assert_eq!(
            model.label(),
            m8s_provider::vast::provision::QWEN38_MODEL_REF
        );
        assert!(model.gguf().is_none());
        assert_eq!(
            model.served(),
            "local-gguf/sha256-322e194ff79741c7baa497c240f677f54b201b0efab44ca8e50f122b39123482"
        );
        assert_ne!(model.served(), DemoModel::default().served());
        assert!(DemoModel::default().gguf().is_some());
        for invalid in ["qwen27", "qwen3.5-27b", "qwen3.8", "Qwen/Qwen3.8-27B"] {
            assert!(serde_json::from_value::<DemoModel>(serde_json::json!(invalid)).is_err());
        }
    }

    #[test]
    fn download_profile_requires_disk_and_single_gpu_headroom() {
        let model = DemoModel::Qwen38;
        assert!(!model.accepts_disk(10));
        assert!(!model.accepts_disk(63));
        assert!(model.accepts_disk(64));
        assert_eq!(model.transfer_reserve_gb(64), 64);
        let requirements = model.requirements();
        assert_eq!(requirements.min_gpu_memory_mib, 48000);
        assert_eq!(requirements.max_compute_hourly, 2_000_000);
        assert_eq!(requirements.max_transfer_micro_usd_per_gb, 4000);
        assert_eq!(requirements.min_download_mbps, 500);
        assert!(requirements.blackwell_sm120);
        assert_eq!(
            model.completion_request()["chat_template_kwargs"]["enable_thinking"],
            false
        );
        assert!(
            DemoModel::Qwen
                .completion_request()
                .get("chat_template_kwargs")
                .is_none()
        );
    }
}
