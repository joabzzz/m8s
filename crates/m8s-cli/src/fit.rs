//! Offline arithmetic over explicit synthetic capacity, never a live offer.
use m8s_models::{FitPlan, Layout, PlanError, PlanRequest};
use serde_json::{Value, json};
use std::{io::Write, path::PathBuf};
const CAP: usize = 16 * 1024;
const SCHEMA: &str = "m8s.model-fit/v1";
const HELP: &str = concat!(
    "m8s models fit --catalog ABS --model ID --gpu-count N --vram-bytes-per-gpu N --layout a|b|c --context-tokens N --minimum-sequences N --reserved-bytes-per-gpu N [--json]\n",
    "Every sizing flag is required once; decimal integers, reserved may be 0. Minimum sequences is per replica. Flags may appear in any order.\n",
    "a: many independent replicas per GPU; b: one independent replica per GPU; c: one balanced-sharded replica across all GPUs on ONE host (runtime support unverified).\n",
    "Capacity estimate only; model unmeasured, runtime not verified, provider policy not assessed, NOT authorized or executable.\n",
);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Error {
    Arguments,
    Catalog,
    Model,
    NoFit,
    Overflow,
    Output,
}
impl Error {
    fn category(self) -> &'static str {
        match self {
            Self::Arguments => "invalid-model-fit-arguments",
            Self::Catalog => "invalid-model-fit-catalog",
            Self::Model => "unknown-model-fit-id",
            Self::NoFit => "model-does-not-fit",
            Self::Overflow => "model-fit-overflow",
            Self::Output => "model-fit-output-unavailable",
        }
    }
}
struct Options {
    catalog: PathBuf,
    model: String,
    gpu_count: u32,
    vram_bytes_per_gpu: u64,
    request: PlanRequest,
    json: bool,
}
fn decimal(raw: &str) -> Result<u64, Error> {
    if raw.is_empty() || raw.len() > 20 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Arguments);
    }
    raw.parse().map_err(|_| Error::Arguments)
}
impl Options {
    fn parse(args: &[String]) -> Result<Self, Error> {
        const FLAGS: [&str; 8] = [
            "--catalog",
            "--model",
            "--gpu-count",
            "--vram-bytes-per-gpu",
            "--layout",
            "--context-tokens",
            "--minimum-sequences",
            "--reserved-bytes-per-gpu",
        ];
        let mut values = [None; 8];
        let mut json = false;
        let mut rest = args;
        while let Some((arg, tail)) = rest.split_first() {
            rest = tail;
            if arg == "--json" && !json {
                json = true;
                continue;
            }
            let slot = FLAGS
                .iter()
                .position(|flag| flag == arg)
                .ok_or(Error::Arguments)?;
            if values[slot].is_some() {
                return Err(Error::Arguments);
            }
            let (value, tail) = rest.split_first().ok_or(Error::Arguments)?;
            values[slot] = Some(value.as_str());
            rest = tail;
        }
        let [
            catalog,
            model,
            gpu,
            vram,
            layout,
            context,
            minimum,
            reserved,
        ] = values.map(|value| value.ok_or(Error::Arguments));
        let catalog = catalog?;
        let model = model?;
        let path = PathBuf::from(catalog);
        if !path.is_absolute()
            || catalog.len() > 4096
            || model.is_empty()
            || model.len() > 96
            || !model.as_bytes()[0].is_ascii_lowercase()
            || !model
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b))
        {
            return Err(Error::Arguments);
        }
        let gpu_count = u32::try_from(decimal(gpu?)?).map_err(|_| Error::Arguments)?;
        let vram_bytes_per_gpu = decimal(vram?)?;
        let request = PlanRequest {
            layout: match layout? {
                "a" => Layout::A,
                "b" => Layout::B,
                "c" => Layout::C,
                _ => return Err(Error::Arguments),
            },
            context_tokens: decimal(context?)?,
            minimum_sequences_per_replica: decimal(minimum?)?,
            reserved_bytes_per_gpu: decimal(reserved?)?,
        };
        if gpu_count == 0
            || vram_bytes_per_gpu == 0
            || request.context_tokens == 0
            || request.minimum_sequences_per_replica == 0
        {
            return Err(Error::Arguments);
        }
        Ok(Self {
            catalog: path,
            model: model.into(),
            gpu_count,
            vram_bytes_per_gpu,
            request,
            json,
        })
    }
}
fn layout_name(layout: Layout) -> &'static str {
    match layout {
        Layout::A => "a",
        Layout::B => "b",
        Layout::C => "c",
    }
}
fn project(options: &Options, fit: FitPlan) -> Value {
    let assumption = match fit.layout {
        Layout::A => {
            "Independent replicas per GPU; remaining capacity divided equally among replicas."
        }
        Layout::B => "One independent replica per GPU; weights are not shared between GPUs.",
        Layout::C => {
            "One replica balanced-sharded across all GPUs on ONE host, never across hosts; runtime support unverified."
        }
    };
    json!({"schema":SCHEMA,"status":"capacity-estimate-only","model":options.model,
        "model_evidence":"model-unmeasured","runtime":"runtime-not-verified",
        "provider_policy":"provider-policy-not-assessed","authorization":"not-authorized","execution":"not-executable",
        "sizing":{"gpu_count":options.gpu_count,"vram_bytes_per_gpu":options.vram_bytes_per_gpu,
            "layout":layout_name(options.request.layout),"context_tokens":options.request.context_tokens,
            "minimum_sequences":options.request.minimum_sequences_per_replica,"reserved_bytes_per_gpu":options.request.reserved_bytes_per_gpu},
        "fit":{"layout":layout_name(fit.layout),"replicas":fit.replicas,"gpus_per_replica":fit.gpus_per_replica,
            "weights_and_overhead_bytes_per_replica":fit.weights_and_overhead_bytes_per_replica,
            "kv_bytes_per_sequence":fit.kv_bytes_per_sequence,"kv_headroom_bytes_per_replica":fit.kv_headroom_bytes_per_replica,
            "concurrent_sequences_per_replica":fit.concurrent_sequences_per_replica,"total_concurrent_sequences":fit.total_concurrent_sequences},
        "assumption":assumption})
}
fn render(value: &Value, json: bool) -> Result<Vec<u8>, Error> {
    let mut bytes = if json {
        serde_json::to_vec(value)
    } else {
        serde_json::to_vec_pretty(value)
    }
    .map_err(|_| Error::Output)?;
    if !json {
        let mut text = b"Offline capacity estimate only: NOT authorized or executable.\n".to_vec();
        text.extend(bytes);
        bytes = text;
    }
    bytes.push(b'\n');
    if bytes.len() > CAP {
        return Err(Error::Output);
    }
    Ok(bytes)
}
fn execute(options: &Options) -> Result<Vec<u8>, Error> {
    let catalog = super::models::load_catalog(&options.catalog).map_err(|()| Error::Catalog)?;
    m8s_config::validate_catalog(&catalog).map_err(|_| Error::Catalog)?;
    let model = catalog
        .models
        .iter()
        .find(|model| model.id == options.model)
        .ok_or(Error::Model)?;
    // plan() reads ONLY gpu_count/vram. This deliberately non-admissible dummy
    // is never output, searched, priced or passed to provider/policy admission.
    let capacity = m8s_core::Offer {
        id: "SYNTHETIC-CAPACITY-ONLY-NOT-A-LIVE-OFFER".into(),
        country: None,
        region: None,
        inet_down_mbps: 0,
        gpu_count: options.gpu_count,
        vram_bytes_per_gpu: options.vram_bytes_per_gpu,
        hourly_micro_usd: 0,
        interruptible: false,
        reliability_bps: 0,
    };
    let fit = m8s_models::plan(model, &capacity, options.request).map_err(|error| match error {
        PlanError::NoCapacity | PlanError::DoesNotFit => Error::NoFit,
        PlanError::Overflow => Error::Overflow,
        PlanError::InvalidProfile => Error::Catalog,
        PlanError::InvalidRequest | PlanError::InvalidBandwidth => Error::Arguments,
    })?;
    render(&project(options, fit), options.json)
}
fn write(bytes: &[u8]) -> Result<(), Error> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(bytes)
        .and_then(|()| stdout.flush())
        .map_err(|_| Error::Output)
}
pub(super) fn run(args: &[String]) -> Result<(), &'static str> {
    if args == ["--help"] {
        return write(HELP.as_bytes()).map_err(Error::category);
    }
    match Options::parse(args).and_then(|options| execute(&options)) {
        Ok(bytes) => write(&bytes).map_err(Error::category),
        Err(error) => {
            if args.iter().any(|arg| arg == "--json") {
                write(
                    &render(
                        &json!({"schema":SCHEMA,"ok":false,"error":error.category()}),
                        true,
                    )
                    .map_err(Error::category)?,
                )
                .map_err(Error::category)?;
            }
            Err(error.category())
        }
    }
}
