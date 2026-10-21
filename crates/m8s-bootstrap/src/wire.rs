//! Private, bounded codec. No Debug/Serialize on secret-bearing messages or buffers.
use crate::*;

pub(crate) const REQUEST_LIMIT: usize = 16 * 1024;
pub(crate) const BUNDLE_LIMIT: usize = 1024 * 1024;
const ID_LIMIT: usize = 256;
const TOKEN_LIMIT: usize = 128 * 1024;
const CONFIG_LIMIT: usize = 512 * 1024;
const CERT_LIMIT: usize = 128 * 1024;
const ACL_LIMIT: usize = 32;

pub(crate) struct Bundle {
    pub claims: CertificateClaims,
    pub certificate: NodeCertificate,
    pub join_token: Zeroizing<Vec<u8>>,
    pub acl: ControlAcl,
    pub settings: BundleSettings,
}
struct Writer(Zeroizing<Vec<u8>>);
impl Writer {
    fn new(tag: u8) -> Self {
        let capacity = match tag {
            1 => REQUEST_LIMIT,
            2 => BUNDLE_LIMIT,
            _ => 13,
        };
        let mut bytes = Zeroizing::new(Vec::with_capacity(capacity));
        bytes.extend_from_slice(&[b'M', b'8', b'B', 1, tag]);
        Self(bytes)
    }
    fn bytes(&mut self, value: &[u8], max: usize) -> Result<()> {
        if value.len() > max {
            return Err(Error::Refused);
        }
        self.0
            .extend_from_slice(&(value.len() as u32).to_be_bytes());
        self.0.extend_from_slice(value);
        Ok(())
    }
    fn text(&mut self, value: &str) -> Result<()> {
        if value.is_empty() || value.contains('\0') {
            return Err(Error::Refused);
        }
        self.bytes(value.as_bytes(), ID_LIMIT)
    }
    fn fixed(&mut self, value: &[u8]) {
        self.0.extend_from_slice(value);
    }
    fn finish(self, max: usize) -> Result<Zeroizing<Vec<u8>>> {
        if self.0.len() > max {
            Err(Error::Refused)
        } else {
            Ok(self.0)
        }
    }
}
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn new(data: &'a [u8], tag: u8, max: usize) -> Result<Self> {
        if data.len() > max || data.len() < 5 || data[..5] != [b'M', b'8', b'B', 1, tag] {
            return Err(Error::Refused);
        }
        Ok(Self(&data[5..]))
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(Error::Refused);
        }
        let (value, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(value)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.take(N)?.try_into().map_err(|_| Error::Refused)
    }
    fn bytes(&mut self, max: usize) -> Result<&'a [u8]> {
        let n = u32::from_be_bytes(self.array()?) as usize;
        if n > max {
            return Err(Error::Refused);
        }
        self.take(n)
    }
    fn text(&mut self) -> Result<String> {
        let s = std::str::from_utf8(self.bytes(ID_LIMIT)?).map_err(|_| Error::Refused)?;
        if s.is_empty() || s.contains('\0') {
            return Err(Error::Refused);
        }
        Ok(s.to_owned())
    }
    fn finish(self) -> Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(Error::Refused)
        }
    }
}
pub(crate) fn encode_request(request: &Redemption) -> Result<Zeroizing<Vec<u8>>> {
    let mut w = Writer::new(1);
    w.fixed(&request.nonce.0[..]);
    w.text(&request.instance_id)?;
    w.text(&request.container_id)?;
    w.fixed(&request.node.0);
    w.fixed(&request.image_sha256);
    w.fixed(&[u8::from(request.self_destruct_present)]);
    w.finish(REQUEST_LIMIT)
}
pub(crate) fn decode_request(data: &[u8]) -> Result<Redemption> {
    let mut r = Reader::new(data, 1, REQUEST_LIMIT)?;
    let nonce = BootNonce::new(r.array()?);
    let instance_id = r.text()?;
    let container_id = r.text()?;
    let node = EndpointId(r.array()?);
    let image_sha256 = r.array()?;
    let self_destruct_present = match r.array::<1>()? {
        [0] => false,
        [1] => true,
        _ => return Err(Error::Refused),
    };
    r.finish()?;
    Ok(Redemption {
        nonce,
        instance_id,
        container_id,
        node,
        image_sha256,
        self_destruct_present,
    })
}
pub(crate) fn encode_bundle(pending: &PendingDelivery) -> Result<Zeroizing<Vec<u8>>> {
    let settings = pending.settings.as_ref().ok_or(Error::Refused)?;
    if settings.heartbeat_ttl_seconds == 0
        || pending.join_token.is_empty()
        || pending.certificate.0.is_empty()
        || pending.acl.endpoints.len() > ACL_LIMIT
    {
        return Err(Error::Refused);
    }
    let mut w = Writer::new(2);
    w.fixed(&pending.claims.owner.0);
    w.fixed(&pending.claims.node.0);
    w.fixed(&pending.claims.issued_at.to_be_bytes());
    w.fixed(&pending.claims.expires_at.to_be_bytes());
    w.bytes(&pending.certificate.0, CERT_LIMIT)?;
    w.bytes(&pending.join_token, TOKEN_LIMIT)?;
    w.fixed(&(pending.acl.endpoints.len() as u32).to_be_bytes());
    for endpoint in &pending.acl.endpoints {
        w.fixed(&endpoint.0);
    }
    w.text(&settings.model)?;
    w.bytes(&settings.config, CONFIG_LIMIT)?;
    w.fixed(&settings.heartbeat_ttl_seconds.to_be_bytes());
    w.finish(BUNDLE_LIMIT)
}
pub(crate) fn decode_bundle(data: &[u8]) -> Result<Bundle> {
    let mut r = Reader::new(data, 2, BUNDLE_LIMIT)?;
    let owner = EndpointId(r.array()?);
    let node = EndpointId(r.array()?);
    let issued_at = i64::from_be_bytes(r.array()?);
    let expires_at = i64::from_be_bytes(r.array()?);
    let certificate = NodeCertificate(r.bytes(CERT_LIMIT)?.to_vec());
    let join_token = Zeroizing::new(r.bytes(TOKEN_LIMIT)?.to_vec());
    let count = u32::from_be_bytes(r.array()?) as usize;
    if count == 0 || count > ACL_LIMIT {
        return Err(Error::Refused);
    }
    let mut endpoints = BTreeSet::new();
    for _ in 0..count {
        if !endpoints.insert(EndpointId(r.array()?)) {
            return Err(Error::Refused);
        }
    }
    if !endpoints.contains(&owner) {
        return Err(Error::Refused);
    }
    let model = r.text()?;
    let config = Zeroizing::new(r.bytes(CONFIG_LIMIT)?.to_vec());
    let heartbeat_ttl_seconds = u64::from_be_bytes(r.array()?);
    r.finish()?;
    if heartbeat_ttl_seconds == 0
        || issued_at < 0
        || expires_at <= issued_at
        || certificate.0.is_empty()
        || join_token.is_empty()
    {
        return Err(Error::Refused);
    }
    Ok(Bundle {
        claims: CertificateClaims {
            owner,
            node,
            issued_at,
            expires_at,
        },
        certificate,
        join_token,
        acl: ControlAcl { owner, endpoints },
        settings: BundleSettings {
            model,
            config,
            heartbeat_ttl_seconds,
        },
    })
}
pub(crate) fn encode_heartbeat(sequence: u64) -> Zeroizing<Vec<u8>> {
    let mut w = Writer::new(3);
    w.fixed(&sequence.to_be_bytes());
    w.0
}
pub(crate) fn decode_heartbeat(data: &[u8]) -> Result<u64> {
    let mut r = Reader::new(data, 3, 13)?;
    let sequence = u64::from_be_bytes(r.array()?);
    r.finish()?;
    Ok(sequence)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_roundtrip_and_all_truncations() {
        let request = Redemption {
            nonce: BootNonce::new([42; 32]),
            instance_id: "synthetic-i".into(),
            container_id: "synthetic-c".into(),
            node: EndpointId([2; 32]),
            image_sha256: [3; 32],
            self_destruct_present: true,
        };
        let encoded = encode_request(&request).unwrap();
        let decoded = decode_request(&encoded).unwrap();
        assert_eq!(decoded.nonce.hash(), request.nonce.hash());
        assert_eq!(decoded.node, request.node);
        for n in 0..encoded.len() {
            assert!(decode_request(&encoded[..n]).is_err());
        }
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(decode_request(&trailing).is_err());
        let mut invalid_bool = encoded.clone();
        *invalid_bool.last_mut().unwrap() = 2;
        assert!(decode_request(&invalid_bool).is_err());
    }
    #[test]
    fn bundle_roundtrip_limits_and_fields() {
        let pending = PendingDelivery {
            delivery_token: None,
            claims: CertificateClaims {
                owner: EndpointId([1; 32]),
                node: EndpointId([2; 32]),
                issued_at: 1,
                expires_at: 100,
            },
            certificate: NodeCertificate(vec![4]),
            join_token: Zeroizing::new(vec![9]),
            acl: ControlAcl::owner_only(EndpointId([1; 32])),
            settings: Some(BundleSettings {
                model: "synthetic-model".into(),
                config: Zeroizing::new(vec![8]),
                heartbeat_ttl_seconds: 60,
            }),
        };
        let encoded = encode_bundle(&pending).unwrap();
        let b = decode_bundle(&encoded).unwrap();
        assert_eq!(b.claims, pending.claims);
        assert_eq!(*b.join_token, vec![9]);
        assert_eq!(b.certificate.0, vec![4]);
        assert!(b.acl.endpoints.contains(&pending.claims.owner));
        assert_eq!(b.settings.model, "synthetic-model");
        for n in 0..encoded.len() {
            assert!(decode_bundle(&encoded[..n]).is_err());
        }
        assert!(decode_bundle(&vec![0; BUNDLE_LIMIT + 1]).is_err());
        let count_offset =
            5 + 32 + 32 + 8 + 8 + 4 + pending.certificate.0.len() + 4 + pending.join_token.len();
        let mut malformed = encoded.clone();
        malformed[count_offset..count_offset + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(decode_bundle(&malformed).is_err());
        let mut missing_owner = encoded.clone();
        missing_owner[count_offset + 4..count_offset + 36].fill(7);
        assert!(decode_bundle(&missing_owner).is_err());
        let mut duplicate = encoded.clone();
        duplicate[count_offset..count_offset + 4].copy_from_slice(&2u32.to_be_bytes());
        duplicate.splice(count_offset + 36..count_offset + 36, pending.claims.owner.0);
        assert!(decode_bundle(&duplicate).is_err());
        let mut pending = pending;
        pending.settings.as_mut().unwrap().config = Zeroizing::new(vec![0; CONFIG_LIMIT + 1]);
        assert!(encode_bundle(&pending).is_err());
    }
    #[test]
    fn unknown_version_lengths_and_heartbeat_are_closed() {
        assert!(decode_request(b"M8B\x021").is_err());
        let mut w = Writer::new(1);
        w.fixed(&[0; 32]);
        w.fixed(&u32::MAX.to_be_bytes());
        assert!(decode_request(&w.0).is_err());
        let frame = encode_heartbeat(7);
        assert_eq!(decode_heartbeat(&frame).unwrap(), 7);
        for n in 0..frame.len() {
            assert!(decode_heartbeat(&frame[..n]).is_err());
        }
        assert!(decode_heartbeat(&[0; 14]).is_err());
    }
}
