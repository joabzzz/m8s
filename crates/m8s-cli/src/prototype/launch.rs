//! Single paid attempt. Durable intent and independent cleanup precede create.
use super::owner::{self, OwnerClient, private_file, utc};
use m8s_bootstrap::{
    BundleSettings, EndpointId, LaunchBinding,
    mp001::{Mp001CertificateVerifier, Mp001TrustPin, signing::Mp001NodeSigner},
};
use m8s_provider_vast::{
    provision::{ProvisionPacket, ProvisioningSession},
    readonly::{access::NodeAccess, launch::LaunchRequest, recovery::RecoveryIntent},
    watchdog::ArmedCleanup,
};
use m8s_secrets::SecretValue;
use m8sd::{
    Store,
    bootstrap_delivery::{
        DeliveryPlan,
        session::{BootstrapDirection, OwnerSessionPlan},
    },
    budget::*,
    nonce::{LaunchIdentity, ObservationStatus},
    operation::vast::{DispatchOutcome, VastExecutor},
};
use std::{
    fs,
    io::Read,
    num::NonZeroU64,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;
type Result<T> = std::result::Result<T, &'static str>;
const ERROR: &str = "prototype-launch-refused";
const MODEL: &str = "qwen2.5-0.5b-instruct-q4_k_m";
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    schema: String,
    directory: PathBuf,
    mesh: PathBuf,
    credential_file: PathBuf,
    image: String,
    offer_id: Option<NonZeroU64>,
    expected_account_id: u64,
    disk_gb: u32,
    owner: String,
    node_uid: String,
    tier: String,
    policy: AdminBudget,
    request: BudgetRequest,
}
fn load(path: &Path) -> Result<Plan> {
    let bytes = super::super::models::load_bytes(path).map_err(|_| ERROR)?;
    if bytes.len() > 32768 {
        return Err(ERROR);
    }
    let p: Plan = serde_json::from_slice(&bytes).map_err(|_| ERROR)?;
    if p.schema != "m8s.vast-demo/v1"
        || p.owner != p.policy.owner
        || p.owner != p.request.owner
        || p.node_uid != p.request.node_uid
        || p.policy.trial_all_in > 6_000_000
        || p.policy.max_instances != 1
        || p.request
            .quote
            .runtime_seconds
            .checked_add(p.request.quote.cleanup_seconds)
            .is_none_or(|v| v > 1800)
        || !p.directory.is_absolute()
        || !p.credential_file.is_absolute()
    {
        return Err(ERROR);
    }
    Ok(p)
}
// Explicitly selected dotenv file only. No environment search or shell evaluation.
fn credential(path: &Path) -> Result<SecretValue> {
    let f = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| ERROR)?;
    let m = f.metadata().map_err(|_| ERROR)?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o022 != 0
        || m.len() > 65536
        || m.nlink() != 1
    {
        return Err(ERROR);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    f.take(65537).read_to_end(&mut bytes).map_err(|_| ERROR)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| ERROR)?;
    let mut found = None;
    for line in text.lines() {
        let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
        if let Some(value) = line.strip_prefix("VAST_API_KEY=") {
            if found.is_some() {
                return Err(ERROR);
            }
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            if value.is_empty()
                || value.len() > 4096
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            {
                return Err(ERROR);
            }
            found = Some(SecretValue::new(value.as_bytes().to_vec()));
        }
    }
    found.ok_or(ERROR)
}
fn observation<'a>(
    p: &'a Plan,
    binding: &'a LaunchBinding,
    observed_at: i64,
) -> LaunchIdentity<'a> {
    LaunchIdentity {
        owner: &p.owner,
        node_uid: &p.node_uid,
        binding,
        observed_at,
        status: ObservationStatus::Complete,
    }
}
fn ssh_key(root: &Path) -> Result<String> {
    let key = root.join("provision-key");
    let status = Command::new("/usr/bin/ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "m8s-provision", "-f"])
        .arg(&key)
        .env_clear()
        .env("HOME", root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| ERROR)?;
    if !status.success() {
        return Err(ERROR);
    }
    let public = fs::read_to_string(root.join("provision-key.pub")).map_err(|_| ERROR)?;
    Ok(public
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" "))
}
pub(super) fn run(args: &[String]) -> Result<()> {
    if args.len() != 2 || args[0] != "--plan" {
        return Err("invalid-prototype-launch-arguments");
    }
    let mut p = load(Path::new(&args[1]))?;
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&p.directory)
        .map_err(|_| ERROR)?;
    if p.directory.canonicalize().map_err(|_| ERROR)? != p.directory {
        return Err(ERROR);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| ERROR)?;
    let signer_key = mesh_llm_identity::OwnerKeypair::generate();
    let owner_key = owner::fresh_key()?;
    let node_key = owner::fresh_key()?;
    let a = std::net::TcpListener::bind("127.0.0.1:0").map_err(|_| ERROR)?;
    let b = std::net::TcpListener::bind("127.0.0.1:0").map_err(|_| ERROR)?;
    let ports = (
        a.local_addr().map_err(|_| ERROR)?.port(),
        b.local_addr().map_err(|_| ERROR)?.port(),
    );
    drop((a, b));
    let mut client = {
        let _enter = runtime.enter();
        OwnerClient::start(
            &p.mesh,
            &p.directory.join("owner-client"),
            &signer_key,
            ports.0,
            ports.1,
            utc()? + 1800,
        )?
    };
    let result = run_with_client(
        &mut p,
        &runtime,
        &signer_key,
        owner_key,
        node_key,
        &mut client,
    );
    let stopped = runtime.block_on(client.stop());
    let _ = fs::remove_file(p.directory.join("provision-key"));
    let evidence = result?;
    stopped?;
    private_file(
        &p.directory.join("demo-evidence.json"),
        &serde_json::to_vec_pretty(&evidence).map_err(|_| ERROR)?,
    )?;
    println!("{}", serde_json::to_string(&evidence).map_err(|_| ERROR)?);
    Ok(())
}
fn run_with_client(
    p: &mut Plan,
    runtime: &tokio::runtime::Runtime,
    signer_key: &mesh_llm_identity::OwnerKeypair,
    owner_key: iroh::SecretKey,
    node_key: iroh::SecretKey,
    client: &mut OwnerClient,
) -> Result<serde_json::Value> {
    let invite = runtime.block_on(client.invite())?;
    eprintln!("prototype stage: certified-owner-client-ready");
    let ssh_public = ssh_key(&p.directory)?;
    let key = credential(&p.credential_file)?;
    let mut preflight = m8s_provider_vast::readonly::preflight::PreflightHttps::new(|| {
        Ok(SecretValue::new(key.expose().to_vec()))
    })
    .map_err(|_| ERROR)?;
    let fresh = preflight
        .observe(p.expected_account_id, p.offer_id, p.disk_gb)
        .map_err(|_| {
            eprintln!("preflight stage: {}", preflight.last_stage());
            preflight
                .transport_failure()
                .unwrap_or(preflight.last_stage())
        })?;
    // Only a fresh full empty inventory can refresh baseline currentness. Never
    // reset cumulative spent/reserved fields or silently extend approval dates.
    if p.policy.baseline.external_liability != ExternalLiability::NoneConfirmed {
        return Err(ERROR);
    }
    p.policy.baseline.observed_at = fresh.observed_at;
    p.policy.baseline.status = EvidenceStatus::Complete;
    p.policy.baseline.provenance = "vast-complete-empty-account-inventory".into();
    p.request.quote.observed_at = fresh.observed_at;
    p.request.quote.valid_until = fresh.observed_at
        + (p.request.quote.runtime_seconds + p.request.quote.cleanup_seconds) as i64
        + 30;
    p.request.quote.status = EvidenceStatus::Complete;
    p.request.quote.compute_unit = RateUnit::MicroUsdPerHour;
    p.request.quote.storage_unit = RateUnit::MicroUsdPerHour;
    p.request.quote.totals_unit = TotalUnit::MicroUsd;
    p.request.quote.compute_hourly = fresh.compute_micro_usd_per_hour;
    p.request.quote.storage_hourly = fresh.storage_micro_usd_per_hour;
    p.request.quote.transfer_total = fresh
        .transfer_micro_usd_per_gb
        .checked_mul(10)
        .ok_or(ERROR)?;
    p.request.quote.provenance = "vast-on-demand-price-observed-10gb-transfer-reserve".into();
    private_file(
        &p.directory.join("preflight-evidence.json"),
        &serde_json::to_vec_pretty(&fresh).map_err(|_| ERROR)?,
    )?;
    let identity =
        RecoveryIntent::new(&p.owner, &p.node_uid, &p.tier, &p.image).map_err(|_| ERROR)?;
    let request = LaunchRequest::new(fresh.offer_id, identity.clone(), ssh_public, p.disk_gb)
        .map_err(|_| ERROR)?;
    let mut executor =
        VastExecutor::new(request, &p.owner, &p.node_uid, &p.tier).map_err(|_| ERROR)?;
    let store = Store::open(p.directory.join("state.db"), &p.owner).map_err(|_| ERROR)?;
    store.scale(&p.tier, 1).map_err(|_| ERROR)?;
    let now = utc()?;
    let reserved = store
        .reserve_budget(
            &p.policy,
            &p.policy.baseline.initial_period,
            &p.request,
            now,
        )
        .map_err(|_| "prototype-budget-refused")?;
    let intent = executor
        .intent(&p.request.run_id, &p.request.request_id, "single-create")
        .map_err(|_| ERROR)?;
    executor
        .prepare_managed(&store, &intent)
        .map_err(|_| ERROR)?;
    let serving_at = i64::try_from(
        executor
            .cleanup_starts_at(&store, &intent)
            .map_err(|_| ERROR)?,
    )
    .map_err(|_| ERROR)?;
    let plan = serde_json::json!({"schema":"m8s.vast-watchdog/v1","owner":p.owner,"node_uid":p.node_uid,"tier":p.tier,"image":p.image,"created_at":now,"destroy_at":serving_at});
    let plan_path = p.directory.join("watchdog-plan.json");
    private_file(&plan_path, &serde_json::to_vec(&plan).map_err(|_| ERROR)?)?;
    let mut cleanup =
        ArmedCleanup::arm_cli(&plan_path, key).map_err(|_| "prototype-cleanup-arm-failed")?;
    // Check the exact guard BEFORE consuming the durable one-use claim.
    if !cleanup
        .guards(&identity, u64::try_from(serving_at).map_err(|_| ERROR)?)
        .map_err(|_| ERROR)?
    {
        return Err("prototype-cleanup-deadline-mismatch-before-claim");
    }
    eprintln!("prototype stage: cleanup-armed");
    let claim = executor.claim(&store, "single-create").map_err(|_| ERROR)?;
    match executor
        .execute(&store, claim, &mut cleanup)
        .map_err(|_| ERROR)?
    {
        DispatchOutcome::NotInvoked => return Err("prototype-create-not-invoked"),
        DispatchOutcome::Candidate(_) | DispatchOutcome::Ambiguous => (),
    }
    let access = loop {
        if utc()? >= serving_at {
            return Err("prototype-create-unresolved-cleanup-retained");
        }
        if let Ok(Some(access)) = cleanup.observe_owned() {
            break access;
        }
        std::thread::sleep(Duration::from_secs(3));
    };
    eprintln!("prototype stage: exact-instance-network-observed");
    // Once positively bound, every subsequent outcome passes through independent
    // provider-confirmed cleanup before reporting success or inference content.
    let work = (|| {
        executor
            .bind_observed(&store, &intent, &access)
            .map_err(|_| ERROR)?;
        // The adapter itself bounds asynchronous log generation. Do not retry
        // an authorization refusal, malformed host key, or unknown log route.
        let host_key = cleanup
            .provisioning_host_key(&access)
            .map_err(|_| "prototype-host-key-unavailable")?;
        eprintln!("prototype stage: provisioning-host-key-pinned");
        let fresh_access = cleanup.observe_owned().map_err(|_| ERROR)?.ok_or(ERROR)?;
        if fresh_access.instance_id() != access.instance_id() {
            return Err(ERROR);
        }
        runtime.block_on(bootstrap(
            p,
            &store,
            signer_key,
            owner_key,
            node_key,
            client,
            invite,
            &fresh_access,
            &host_key,
            serving_at,
        ))
    })();
    let _ = store.down();
    eprintln!("prototype stage: cleanup-requested");
    cleanup.request_cleanup();
    let deadline = Instant::now() + Duration::from_secs(180);
    let absent = loop {
        if let Ok(Some(receipt)) = cleanup.confirm_owned_absent(&access) {
            break receipt;
        }
        if Instant::now() >= deadline {
            return Err("prototype-removal-unresolved-retain-liability");
        }
        std::thread::sleep(Duration::from_secs(3));
    };
    private_file(&p.directory.join("cleanup-evidence.json"), &serde_json::to_vec_pretty(&serde_json::json!({"schema":"m8s.vast-removal/v1","instance_id":absent.instance_id().get(),"image":p.image,"creator_account_id":p.expected_account_id,"provider_confirmed_absent":true,"observed_at":utc()?})).map_err(|_| ERROR)?)?;
    eprintln!("prototype stage: exact-instance-absence-confirmed");
    let binding = store
        .budget_reservations()
        .map_err(|_| ERROR)?
        .into_iter()
        .find(|r| r.request.request_id == p.request.request_id)
        .and_then(|r| r.binding)
        .ok_or(ERROR)?;
    let now = utc()?;
    store
        .settle_budget(
            &p.request.request_id,
            &reserved.period,
            &SettlementEvidence {
                binding,
                observed_at: now,
                status: EvidenceStatus::Complete,
                resource: ResourceOutcome::OwnedAbsent,
                volumes: VolumeOutcome::BoundVolumesAbsent,
                billing: FinalBilling::Unknown,
                provenance: "creator-account-exact-get-null".into(),
            },
            now,
        )
        .map_err(|_| ERROR)?;
    let completion = work?;
    Ok(
        serde_json::json!({"schema":"m8s.vast-demo-evidence/v1","instance_id":absent.instance_id().get(),"image":p.image,"model":MODEL,"certificate_verified":true,"owner_routed_inference":true,"provider_confirmed_absent":true,"reserved_micro_usd":reserved.reserved_all_in,"final_billing":"unknown-liability-retained","completion":completion}),
    )
}
#[allow(clippy::too_many_arguments)]
async fn bootstrap(
    p: &Plan,
    store: &Store,
    signer_key: &mesh_llm_identity::OwnerKeypair,
    owner_key: iroh::SecretKey,
    node_key: iroh::SecretKey,
    client: &OwnerClient,
    invite: Zeroizing<Vec<u8>>,
    access: &NodeAccess,
    host_key: &m8s_provider_vast::readonly::provisioning_log::HostPublicKey,
    expires: i64,
) -> Result<serde_json::Value> {
    let owner = EndpointId(*owner_key.public().as_bytes());
    let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
        .secret_key(owner_key)
        .clear_ip_transports()
        .bind_addr(
            "0.0.0.0:0"
                .parse::<std::net::SocketAddr>()
                .map_err(|_| ERROR)?,
        )
        .map_err(|_| ERROR)?
        .clear_relay_transports()
        .clear_address_lookup()
        .alpns(vec![
            m8s_bootstrap::BOOT_ALPN.to_vec(),
            m8s_bootstrap::CONTROL_ALPN.to_vec(),
        ])
        .bind()
        .await
        .map_err(|_| ERROR)?;
    let digest = p.image.rsplit_once("@sha256:").ok_or(ERROR)?.1;
    let image_sha256: [u8; 32] = hex::decode(digest)
        .map_err(|_| ERROR)?
        .try_into()
        .map_err(|_| ERROR)?;
    let binding = LaunchBinding {
        owner,
        instance_id: access.instance_id().to_string(),
        container_id: access.instance_id().to_string(),
        provider_ip: access.control_address().ip(),
        image_sha256,
    };
    let mut nonce = Zeroizing::new([0; 32]);
    getrandom::fill(nonce.as_mut()).map_err(|_| ERROR)?;
    store
        .issue_bootstrap_nonce(
            &nonce,
            observation(p, &binding, access.observed_at()),
            utc()?,
        )
        .map_err(|_| ERROR)?;
    let config = serde_json::json!({"schema":"m8s.vast-node/v1","instance_id":access.instance_id().get(),"container_id":access.instance_id().to_string(),"image_sha256":digest,"owner_endpoint":hex::encode(owner.0),"owner_signing_key":hex::encode(signer_key.verifying_key().as_bytes()),"mesh_owner_id":signer_key.owner_id(),"owner_address":"127.0.0.1:1","bind_address":"0.0.0.0:40100","trust":mesh_llm_identity::TrustStore::default(),"program":"/opt/mesh/mesh-llm","gguf":"/models/qwen2.5-0.5b-instruct-q4_k_m.gguf","material_root":"/var/lib/m8s/material","model":MODEL,"heartbeat_ttl_seconds":30,"max_lifetime_seconds":1800,"port":9337,"console_port":3131});
    let packet = ProvisionPacket::new(
        &config,
        SecretValue::new(nonce.to_vec()),
        SecretValue::new(node_key.to_bytes().to_vec()),
    )
    .map_err(|_| ERROR)?;
    let mut provision = ProvisioningSession::start(
        access,
        host_key,
        &p.directory,
        &p.directory.join("provision-key"),
        packet,
    )
    .await
    .map_err(|_| "prototype-ssh-provision-failed")?;
    eprintln!("prototype stage: ssh-provision-sent");
    let pin =
        Mp001TrustPin::new(owner, *signer_key.verifying_key().as_bytes()).map_err(|_| ERROR)?;
    let mut signer = Mp001NodeSigner::new(signer_key, &pin, 1800, 5).map_err(|_| ERROR)?;
    let verifier = Mp001CertificateVerifier::new(
        Mp001TrustPin::new(owner, *signer_key.verifying_key().as_bytes()).map_err(|_| ERROR)?,
        mesh_llm_identity::TrustStore::default(),
    )
    .map_err(|_| ERROR)?;
    let plan = OwnerSessionPlan {
        node_address: iroh::EndpointAddr::new(node_key.public())
            .with_ip_addr(access.control_address()),
        direction: BootstrapDirection::OwnerInitiates,
        delivery: DeliveryPlan {
            observation: observation(p, &binding, access.observed_at()),
            cert_expires_at: expires,
            join_token: invite,
            settings: BundleSettings {
                model: MODEL.into(),
                config: Zeroizing::new(Vec::new()),
                heartbeat_ttl_seconds: 30,
            },
        },
        bootstrap_timeout: Duration::from_secs(60),
    };
    let port = client.port;
    let remaining = expires
        .checked_sub(utc()?)
        .filter(|v| *v > 0)
        .ok_or(ERROR)?;
    let work_deadline = Instant::now() + Duration::from_secs(remaining as u64);
    let result = store.with_bootstrapped_node(&endpoint,plan,&mut signer,&verifier,|_| async move {
        eprintln!("prototype stage: certificate-delivered-control-established");
        tokio::task::spawn_blocking(move || {
            super::model_wait::wait(port,MODEL,(Instant::now()+Duration::from_secs(120)).min(work_deadline)).map_err(|_| m8s_bootstrap::Error::Unavailable)?;
            eprintln!("prototype stage: owner-routed-model-observed");
            let body = serde_json::to_vec(&serde_json::json!({"model":MODEL,"messages":[{"role":"user","content":"Reply with a short sentence explaining what a mesh network is."}],"stream":false,"max_tokens":48})).map_err(|_| m8s_bootstrap::Error::Unavailable)?;
            let bytes = super::http::exchange(port,&body,(Instant::now()+Duration::from_secs(60)).min(work_deadline)).map_err(|_| m8s_bootstrap::Error::Unavailable)?;
            let value = super::project(&bytes,MODEL).map_err(|_| m8s_bootstrap::Error::Unavailable)?;
            eprintln!("prototype stage: owner-routed-inference-completed");
            Ok(serde_json::json!({"content":value["content"],"finish_reason":value["finish_reason"]}))
        }).await.map_err(|_| m8s_bootstrap::Error::Unavailable)?
    },std::future::pending(),|| utc().unwrap_or(-1)).await.map_err(|error| { eprintln!("prototype bootstrap category: {error}"); "prototype-bootstrap-or-inference-failed" });
    let _ = provision.stop().await;
    endpoint.close().await;
    result
}

pub(super) fn preflight(args: &[String]) -> Result<()> {
    if args.len() != 6
        || args[0] != "--credential-file"
        || args[2] != "--account"
        || args[4] != "--disk-gb"
    {
        return Err("invalid-preflight-arguments");
    }
    let key = credential(Path::new(&args[1]))?;
    let account = args[3].parse().map_err(|_| ERROR)?;
    let disk = args[5].parse().map_err(|_| ERROR)?;
    let mut client = m8s_provider_vast::readonly::preflight::PreflightHttps::new(|| {
        Ok(SecretValue::new(key.expose().to_vec()))
    })
    .map_err(|_| "vast-client-initialization")?;
    let proof = client.observe(account, None, disk).map_err(|_| {
        eprintln!("preflight stage: {}", client.last_stage());
        client.transport_failure().unwrap_or(client.last_stage())
    })?;
    println!("{}", serde_json::to_string(&proof).map_err(|_| ERROR)?);
    Ok(())
}

pub(super) fn audit(args: &[String]) -> Result<()> {
    if args.len() != 4 || args[0] != "--directory" || args[2] != "--owner" {
        return Err("invalid-prototype-audit-arguments");
    }
    let path = Path::new(&args[1]).join("state.db");
    if !fs::symlink_metadata(&path).map_err(|_| ERROR)?.is_file() {
        return Err(ERROR);
    }
    let store = Store::open(path, &args[3]).map_err(|_| "prototype-audit-unavailable")?;
    let operations = store.create_operations().map_err(|_| ERROR)?;
    let records: Vec<_> = operations.iter().map(|r|serde_json::json!({"operation_id":r.intent.operation_id,"state":r.state,"invocation_started_at":r.invocation_started_at,"provider_result_recorded":r.result.is_some(),"uncertainty_observed":r.uncertainty_observed})).collect();
    let budgets: Vec<_> = store.budget_reservations().map_err(|_| ERROR)?.into_iter().map(|r|serde_json::json!({"request_id":r.request.request_id,"state":r.state,"reserved_micro_usd":r.reserved_all_in,"reserved_at":r.reserved_at})).collect();
    println!(
        "{}",
        serde_json::json!({"schema":"m8s.prototype-attempt-audit/v1","operations":records,"budgets":budgets,"provider_calls":false})
    );
    Ok(())
}
