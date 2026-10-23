//! Exact owned instance networking observation, never Ready or inference proof.
use super::*;
use recovery::{RecoveryCandidate, RecoveryIntent};
use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    num::NonZeroU64,
};

pub struct NodeAccess {
    instance: NonZeroU64,
    ssh: SocketAddr,
    control: SocketAddr,
}
impl NodeAccess {
    pub fn instance_id(&self) -> NonZeroU64 {
        self.instance
    }
    pub fn ssh_address(&self) -> SocketAddr {
        self.ssh
    }
    pub fn control_address(&self) -> SocketAddr {
        self.control
    }
}

pub struct OwnedAccessHttps<S> {
    transport: ReadonlyHttps<S>,
    instance: NonZeroU64,
    intent: RecoveryIntent,
}
impl<S: FnMut() -> Result<SecretValue>> OwnedAccessHttps<S> {
    pub fn new(candidate: RecoveryCandidate, intent: RecoveryIntent, supplier: S) -> Result<Self> {
        Ok(Self {
            transport: ReadonlyHttps::new(supplier)?,
            instance: candidate.instance_id(),
            intent,
        })
    }
    pub fn observe(&mut self) -> Result<Option<NodeAccess>> {
        let value = super::destroy::exchange_scoped(
            &mut self.transport.backend,
            &mut self.transport.supplier,
            self.instance,
            reqwest::Method::GET,
        )?;
        project(&value, self.instance, &self.intent)
    }
}

fn public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    // Do not let provider data turn provisioning into local/private-network SSH.
    !ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        && !ip.is_documentation()
        && a != 0
        && a < 224
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 198 && (b == 18 || b == 19))
        && !(a == 192 && b == 0)
}
fn mapped_port(row: &Value, name: &str) -> Result<u16> {
    let mappings = row
        .get("ports")
        .and_then(|v| v.get(name))
        .and_then(Value::as_array)
        .filter(|v| v.len() == 1)
        .ok_or(Error::Unavailable)?;
    let port = mappings[0]
        .get("HostPort")
        .and_then(Value::as_str)
        .ok_or(Error::Unavailable)?;
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::Unavailable);
    }
    port.parse::<u16>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or(Error::Unavailable)
}
fn project(
    value: &Value,
    instance: NonZeroU64,
    intent: &RecoveryIntent,
) -> Result<Option<NodeAccess>> {
    let row = value
        .get("instances")
        .and_then(Value::as_object)
        .ok_or(Error::Unavailable)?;
    if row.get("id").and_then(Value::as_u64) != Some(instance.get())
        || row.get("label").and_then(Value::as_str) != Some(intent.label())
        || row.get("image_uuid").and_then(Value::as_str) != Some(intent.image())
    {
        return Err(Error::Unavailable);
    }
    match row.get("actual_status").and_then(Value::as_str) {
        Some("created" | "loading" | "stopped" | "exited" | "offline") => return Ok(None),
        Some("running") => (),
        _ => return Err(Error::Unavailable),
    }
    let ip: Ipv4Addr = row
        .get("public_ipaddr")
        .and_then(Value::as_str)
        .ok_or(Error::Unavailable)?
        .parse()
        .map_err(|_| Error::Unavailable)?;
    if !public_ipv4(ip) {
        return Err(Error::Unavailable);
    }
    let row = Value::Object(row.clone());
    let ssh = SocketAddr::V4(SocketAddrV4::new(ip, mapped_port(&row, "2222/tcp")?));
    let control = SocketAddr::V4(SocketAddrV4::new(ip, mapped_port(&row, "40100/udp")?));
    Ok(Some(NodeAccess {
        instance,
        ssh,
        control,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn intent() -> RecoveryIntent {
        RecoveryIntent::new(
            "synthetic-owner",
            "synthetic-node",
            "smoke",
            &format!("example.invalid/mesh@sha256:{}", "a".repeat(64)),
        )
        .unwrap()
    }
    fn row() -> Value {
        let intent = intent();
        json!({"instances":{"id":42,"label":intent.label(),"image_uuid":intent.image(),
            "actual_status":"running","public_ipaddr":"8.8.8.8",
            "ports":{"2222/tcp":[{"HostPort":"12000"}],"40100/udp":[{"HostPort":"13000"}]}}})
    }
    #[test]
    fn exact_public_access_is_not_a_readiness_grant() {
        // Pure projection only: this documentation fixture makes no connection.
        let access = project(&row(), NonZeroU64::new(42).unwrap(), &intent())
            .unwrap()
            .unwrap();
        assert_eq!(access.ssh_address().port(), 12000);
        assert_eq!(access.control_address().port(), 13000);
        for state in ["loading", "stopped", "exited", "offline"] {
            let mut value = row();
            value["instances"]["actual_status"] = json!(state);
            assert!(
                project(&value, NonZeroU64::new(42).unwrap(), &intent())
                    .unwrap()
                    .is_none()
            );
        }
    }
    #[test]
    fn private_special_wrong_identity_and_ambiguous_ports_refuse() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.1.2.3",
            "192.0.2.1",
            "198.18.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
        ] {
            let mut value = row();
            value["instances"]["public_ipaddr"] = json!(ip);
            assert!(project(&value, NonZeroU64::new(42).unwrap(), &intent()).is_err());
        }
        for field in ["id", "label", "image_uuid", "ports"] {
            let mut value = row();
            value["instances"][field] = Value::Null;
            assert!(project(&value, NonZeroU64::new(42).unwrap(), &intent()).is_err());
        }
        let mut value = row();
        value["instances"]["ports"]["2222/tcp"] =
            json!([{"HostPort":"12000"},{"HostPort":"12001"}]);
        assert!(project(&value, NonZeroU64::new(42).unwrap(), &intent()).is_err());
    }
}
