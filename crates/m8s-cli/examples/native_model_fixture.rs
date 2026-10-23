//! Explicit offline native-model fixture. No provider calls or owner-key files.
use mesh_llm_identity::{
    OwnerKeypair,
    exact_node_signing::{ExactNodeSigningRequest, ExactSigningPolicy, sign_node_ownership_exact},
};
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("expected fresh absolute fixture directory")?;
    let root = Path::new(&path);
    if !root.is_absolute() {
        return Err("absolute directory required".into());
    }
    fs::DirBuilder::new().mode(0o700).create(root)?;
    if root.canonicalize()? != root {
        return Err("canonical directory required".into());
    }
    let owner = OwnerKeypair::generate();
    let mut seed = Zeroizing::new([0u8; 32]);
    getrandom::fill(seed.as_mut()).map_err(|_| "randomness unavailable")?;
    let node = iroh::SecretKey::from_bytes(&seed);
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() * 1000;
    let request = ExactNodeSigningRequest::new(*node.public().as_bytes(), now, now + 1_800_000)?;
    let cert =
        sign_node_ownership_exact(&owner, &request, &ExactSigningPolicy::new(1_800_000, 5000)?)?;
    let write = |name: &str, bytes: &[u8]| -> std::io::Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join(name))?;
        f.write_all(bytes)?;
        f.sync_all()
    };
    let encoded = Zeroizing::new(hex::encode(seed.as_slice()));
    write("node-key", encoded.as_bytes())?;
    write("node-ownership.json", &serde_json::to_vec(&cert)?)?;
    write("owner-id", owner.owner_id().as_bytes())?;
    println!("Offline fixture created; owner signing key was not persisted.");
    Ok(())
}
