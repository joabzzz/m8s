# m8s-config — bounded schema1 DRAFT

Owner intent parser/validation library, **not** a scheduler, autoscaler, router,
provider client, permission grant or deployment/readiness check. No network,
environment interpolation/discovery, keychain, credentials, model discovery or
downloads. No daemon/CLI integration in this slice.

```rust
let catalog: m8s_core::models::ModelCatalog =
    toml::from_str(m8s_core::models::EXAMPLE_CATALOG)?; // explicit supplied example data
let limits = m8s_config::ValidationLimits::default(); // trusted caller envelope
let config = m8s_config::Config::parse(m8s_config::EXAMPLE.as_bytes(), &catalog, &limits)?;
let normalized_ttl_seconds = config.dms().ttl_seconds;
let canonical_toml = config.to_toml()?;
```

`Config` and tier/queue/schedule views are immutable and not independently
Deserialize-able. They do not retain the catalog or its sources. Errors have only
fixed categories; no raw TOML error/input/path or user strings in error chains.
`to_toml` serializes the validated **configuration**, never credentials/profiles;
accepted inputs also have a bounded canonical round trip. Raw TOML is never logged.

`Config::from_path(path, catalog, limits)` opens only the explicitly supplied
bounded regular file. On Unix final symlinks/special files are refused using
O_NOFOLLOW/O_NONBLOCK plus metadata and bounded reads. Parent path resolution is
caller-trusted, not a malicious-filesystem boundary. Other platforms use byte
parsing; the optional file-loader path fails closed there.

## Version1 schema / defaults

See `examples/m8s.toml` (illustration only). `schema = 1` and nonempty `[tier.<id>]`
are mandatory. Duplicates, unknown fields and type errors fail closed at every
level. 64KiB input/canonical-output cap;32 tiers;256 explicit catalog profiles;
16 schedule pairs per tier. Tier IDs max48/model IDs max96 ASCII bytes, start
lowercase letter and contain only lowercase letters/digits/`-_.`, no path syntax.

- `[budget]`: integer `monthly_micro_usd` (default20M),
  `max_total_hourly_micro_usd` (3M), `max_instance_hourly_micro_usd` (1M, <=total).
  **No TOML float money** or USD-field aliases. Separate string-only
  `usd_limit_to_micro_usd` floors and `usd_cost_to_micro_usd` ceils exact decimal
  USD, including fractional-microUSD values; no binary float or exponent syntax.
- `ValidationLimits` is an explicit trusted caller-injected administrative envelope,
  default20M monthly /3M total-hourly /3M instance-hourly ceilings. Not universal
  immutable product policy; other authorized environments can inject different
  limits. Out-of-envelope config is refused, never clamped/raised. A caller's
  limits are NOT proof of Admin approval. Parsing monthly config never grants
  unattended Vast test permission or replaces the separate initial all-in $2 gate.
- `[dead_mans_switch]`: `ttl="1h"`, `max_lifetime="12h"`. Positive integer `s`,
  `m`, `h`, `d` suffix only; checked multiply and <=31d, max>=ttl. No off bypass,
  fractional/compound duration or implicit units. This describes timers only.
- `[policy]`: `minimum_down_mbps=1000`, may raise up to1M, never lower. Core's
  location denylist, unknown-location rejection, and verified signed m8s image
  requirement remain mandatory **requirements**, never verification evidence.
  Lowering/removing denylist or security exceptions is not supported in bounded
  schema1. Future ADR0005 configurability is not silently redefined or removed.
- `[owner] timezone`: explicit chrono-tz/IANA name required if any schedules exist;
  no OS/environment/local-time fallback. Optional otherwise.
- Tiers require explicit `model` reference into the supplied catalog. No selected
  default model/recommendation. Catalog IDs unique, profiles bounded, positive
  weights/KV/context/parameters, weights+overhead checked, known runtime/license
  class metadata. `unknown` license class supports current examples; runtime
  `certified` is descriptive data, NOT verified certification or Ready evidence.
- `provider` defaults `["vast-secure","vast-community"]`; only these unique known
  values, preference order retained. `replicas={min=0,max=1}`, both0..16,min<=max.
  `gpu={count=1,min_vram_bytes=1}` defaults mean only a one-byte additional floor,
  NOT a model-fit assertion; actual planner must still check capacity/runtime.
  GPU count1..16, byte floor1..1TB. `interruptible=false`, explicit opt-in accepted.
- `cold={mode="wait",timeout="15m"}` default; `reject` accepts no extra fields;
  `fallback` requires only target `tier`. Missing targets/self/multi-tier cycles
  fail. Fallback is a hint to client/harness for another tier/model, not routing.
- `manual=true` descriptive. Optional `queue` typed integer thresholds
  `queue_wait_ms_gt` (1..86400000) and/or `inflight_gt` (1..1M), required
  `scale_up_for` and `scale_down_idle` positive durations. No freeform trigger DSL.
- `[[tier.<id>.schedule]]` requires distinct `open`/`close` numeric five-field
  cron descriptions, exactly one ASCII space separator. Fields minute0..59,
  hour0..23,day1..31,month1..12,weekday0..6 (Sunday0). Lists/ranges and positive
  steps on `*` or ranges supported; names/macros/seconds/single-number steps and
  arbitrary syntax refused. Only grammar/ranges are checked: calendar feasibility,
  DST ambiguity, next fire times, pairing and execution are NOT implemented.

## ADR sketch refinement / scope

ADR0009 was a sketch, not a finished public contract. Draft schema1 explicitly
uses integer microUSD vs float USD, byte GPU capacities vs ambiguous GB, typed
cold/queue tables vs DSL strings, explicit timezone and supplied actual
`m8s_core::models::ModelCatalog` shape instead of discovering files. No scale-up axis,
cron execution, queue metric observation, provider calls, admission or Ready/Gone
inference. Budget limits remain injected owner policy; no new Admin/security
exception mechanism. Root reviews this bounded draft before final public schema.

Primary docs: [TOML](https://docs.rs/toml/latest/toml/),
[Serde unknown fields](https://serde.rs/container-attrs.html),
[chrono-tz](https://docs.rs/chrono-tz/latest/chrono_tz/).
Design/evidence: [Knowledge](agor://kb/m8s/designs/config-schema-v1-draft.md).

## Pure calendar projection

`schedule::preview(&validated_config, tier, from, hours, limit)` is an offline
matching-event projection, not a scheduler or authority. Explicit nonnegative Unix
seconds, hours1..168, limit1..256; scan UTC-minute instants in
`(from, from+hours*3600]`, bounded10080minutes x32 expressions. Whole-window checked
UTC/local representability; no clock/filesystem/effects. Uses actual chrono-tz
owner IANA offsets, including historical second offsets. Events retain UTC order,
then zero-based rule index/open-before-close; gaps omit matches and autumn repeats
preserve distinct UTC instants, not a live firing policy. No current-state or
interval/conflict/desired-state inference.

Existing Config cron validation/serialization acceptance is unchanged. Expansion
first reuses its private validator. Conservative preview subset requires DOM OR
DOW literal `*` for every open/close expression; both nonliteral refuses the whole
preview, including semantically equivalent `*/1`. Lists union, calendar fields
intersect, Sunday0; wildcard/range steps anchor at field/range minimum. Inputs are
validated intent, not authenticated/approved invocations. Returned limit truncation
is explicit; replace neither schedule execution nor durable coordinator policy.
