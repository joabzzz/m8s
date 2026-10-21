use super::*;

fn binding() -> LaunchBinding {
    LaunchBinding {
        owner: EndpointId([1; 32]),
        instance_id: "synthetic-instance".into(),
        container_id: "synthetic-container".into(),
        provider_ip: "203.0.113.1".parse().unwrap(),
        image_sha256: [3; 32],
    }
}
fn session(alpn: &[u8], peer: u8) -> IrohSession {
    IrohSession {
        owner: binding().owner,
        peer: EndpointId([peer; 32]),
        source_ip: binding().provider_ip,
        alpn: alpn.to_vec(),
    }
}
fn request() -> Redemption {
    Redemption {
        nonce: BootNonce::new([42; 32]),
        instance_id: binding().instance_id,
        container_id: binding().container_id,
        node: EndpointId([2; 32]),
        image_sha256: [3; 32],
        self_destruct_present: true,
    }
}
// Synthetic signer/verifier only. These tests are NOT cryptographic admission tests.
struct TestAuthority {
    calls: usize,
    fail_sign: bool,
    fail_verify: bool,
}
impl NodeSigner for TestAuthority {
    fn sign(&mut self, _: &CertificateClaims) -> Result<NodeCertificate> {
        self.calls += 1;
        if self.fail_sign {
            Err(Error::Unavailable)
        } else {
            Ok(NodeCertificate(vec![0]))
        }
    }
}
impl CertificateVerifier for TestAuthority {
    fn verify(&self, _: &NodeCertificate, _: &CertificateClaims) -> Result<()> {
        if self.fail_verify {
            Err(Error::Certificate)
        } else {
            Ok(())
        }
    }
}
fn authority() -> TestAuthority {
    TestAuthority {
        calls: 0,
        fail_sign: false,
        fail_verify: false,
    }
}
fn ledger() -> Ledger {
    let l = Ledger::new(Connection::open_in_memory().unwrap()).unwrap();
    l.issue(&request().nonce, &binding(), 100).unwrap();
    l
}
fn redeem(
    l: &Ledger,
    s: &IrohSession,
    r: Redemption,
    now: i64,
    signer: &mut TestAuthority,
    verifier: &TestAuthority,
) -> Result<PendingDelivery> {
    l.redeem(
        s,
        r,
        &binding(),
        now,
        2000,
        Zeroizing::new(vec![9]),
        signer,
        verifier,
    )
}
#[test]
fn ttl_future_and_replay_fail_closed() {
    let l = ledger();
    let mut a = authority();
    let v = authority();
    for now in [99, 1000, i64::MAX] {
        assert!(redeem(&l, &session(BOOT_ALPN, 2), request(), now, &mut a, &v).is_err());
    }
    let b = redeem(&l, &session(BOOT_ALPN, 2), request(), 999, &mut a, &v).unwrap();
    assert_eq!(b.claims().node, EndpointId([2; 32]));
    assert!(redeem(&l, &session(BOOT_ALPN, 2), request(), 999, &mut a, &v).is_err());
    assert_eq!(a.calls, 1);
}
#[test]
fn all_bindings_checked_before_signing() {
    let l = ledger();
    let mut a = authority();
    let v = authority();
    for change in 0..8 {
        let mut r = request();
        let mut s = session(BOOT_ALPN, 2);
        match change {
            0 => r.instance_id.push('x'),
            1 => r.container_id.push('x'),
            2 => r.image_sha256[0] ^= 1,
            3 => r.node = EndpointId([4; 32]),
            4 => s.source_ip = "203.0.113.2".parse().unwrap(),
            5 => r.self_destruct_present = false,
            6 => s.owner = EndpointId([5; 32]),
            _ => s.alpn = CONTROL_ALPN.to_vec(),
        }
        assert!(redeem(&l, &s, r, 101, &mut a, &v).is_err());
    }
    assert_eq!(a.calls, 0);
    assert!(redeem(&l, &session(BOOT_ALPN, 2), request(), 101, &mut a, &v).is_ok());
}
#[test]
fn ledger_binding_cannot_be_replaced_by_caller() {
    let l = ledger();
    let mut expected = binding();
    expected.container_id.push('x');
    let mut r = request();
    r.container_id = expected.container_id.clone();
    let mut a = authority();
    assert!(
        l.redeem(
            &session(BOOT_ALPN, 2),
            r,
            &expected,
            101,
            2000,
            Zeroizing::new(vec![9]),
            &mut a,
            &authority()
        )
        .is_err()
    );
    assert_eq!(a.calls, 0);
}
#[test]
fn signing_and_verification_failure_burn_nonce() {
    for fail_sign in [true, false] {
        let l = ledger();
        let mut a = authority();
        a.fail_sign = fail_sign;
        let mut v = authority();
        v.fail_verify = !fail_sign;
        assert!(matches!(
            redeem(&l, &session(BOOT_ALPN, 2), request(), 101, &mut a, &v),
            Err(Error::Certificate)
        ));
        a.fail_sign = false;
        assert!(
            redeem(
                &l,
                &session(BOOT_ALPN, 2),
                request(),
                102,
                &mut a,
                &authority()
            )
            .is_err()
        );
        assert_eq!(a.calls, 1);
    }
}
#[test]
fn durable_burn_and_concurrent_replay() {
    // Shared SQLite memory DB with two independent connections; no filesystem secrets.
    let uri = "file:m8s-bootstrap-replay?mode=memory&cache=shared";
    let l = Ledger::new(Connection::open(uri).unwrap()).unwrap();
    l.issue(&request().nonce, &binding(), 100).unwrap();
    let l2 = Ledger::new(Connection::open(uri).unwrap()).unwrap();
    let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = [l, l2]
        .into_iter()
        .map(|ledger| {
            let gate = gate.clone();
            std::thread::spawn(move || {
                gate.wait();
                redeem(
                    &ledger,
                    &session(BOOT_ALPN, 2),
                    request(),
                    101,
                    &mut authority(),
                    &authority(),
                )
                .is_ok()
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|ok| *ok)
            .count(),
        1
    );
}
#[test]
fn restart_preserves_consumption() {
    let path = std::env::temp_dir().join(format!(
        "m8s-bootstrap-synthetic-restart-{}.sqlite",
        std::process::id()
    ));
    let l = Ledger::new(Connection::open(&path).unwrap()).unwrap();
    l.issue(&request().nonce, &binding(), 100).unwrap();
    redeem(
        &l,
        &session(BOOT_ALPN, 2),
        request(),
        101,
        &mut authority(),
        &authority(),
    )
    .unwrap();
    drop(l);
    let l = Ledger::new(Connection::open(&path).unwrap()).unwrap();
    assert!(
        redeem(
            &l,
            &session(BOOT_ALPN, 2),
            request(),
            102,
            &mut authority(),
            &authority()
        )
        .is_err()
    );
    drop(l);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn acl_is_explicit_and_alpn_scoped() {
    let acl = ControlAcl::owner_only(binding().owner);
    assert!(acl.authorize(&session(CONTROL_ALPN, 1)).is_ok());
    assert!(acl.authorize(&session(CONTROL_ALPN, 2)).is_err());
    assert!(acl.authorize(&session(BOOT_ALPN, 1)).is_err());
    let acl = ControlAcl::with_delegates(binding().owner, [EndpointId([4; 32])]);
    assert!(acl.authorize(&session(CONTROL_ALPN, 4)).is_ok());
}
#[test]
fn heartbeats_bind_identity_expiry_and_monotonic_sequence() {
    let claims = CertificateClaims {
        owner: binding().owner,
        node: EndpointId([2; 32]),
        issued_at: 100,
        expires_at: 200,
    };
    let mut guard = HeartbeatGuard::new(&claims, 60).unwrap();
    assert!(guard.receive(&session(CONTROL_ALPN, 3), 1, 101).is_err());
    assert!(guard.receive(&session(BOOT_ALPN, 2), 1, 101).is_err());
    guard.receive(&session(CONTROL_ALPN, 2), 1, 101).unwrap();
    assert!(guard.receive(&session(CONTROL_ALPN, 2), 1, 102).is_err());
    assert!(guard.receive(&session(CONTROL_ALPN, 2), 2, 100).is_err());
    assert!(!guard.expired(160));
    assert!(guard.expired(161));
    assert!(guard.receive(&session(CONTROL_ALPN, 2), 2, 161).is_err());
    assert!(guard.receive(&session(CONTROL_ALPN, 2), 2, 200).is_err());
}
#[cfg(not(feature = "iroh-transport"))]
#[test]
fn transport_and_delivery_remain_unavailable() {
    assert!(matches!(accept_iroh(), Err(Error::Unavailable)));
    let bundle = redeem(
        &ledger(),
        &session(BOOT_ALPN, 2),
        request(),
        101,
        &mut authority(),
        &authority(),
    )
    .unwrap();
    assert!(matches!(
        bundle.deliver(&session(BOOT_ALPN, 2)),
        Err(Error::Unavailable)
    ));
}

#[test]
fn outer_transactions_and_duplicate_issuance_are_refused() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("BEGIN").unwrap();
    assert!(matches!(Ledger::new(connection), Err(Error::Refused)));
    let ledger = ledger();
    assert!(ledger.issue(&request().nonce, &binding(), 100).is_err());
    let mut signer = authority();
    let mut r = request();
    r.nonce = BootNonce::new([43; 32]);
    assert!(
        redeem(
            &ledger,
            &session(BOOT_ALPN, 2),
            r,
            101,
            &mut signer,
            &authority()
        )
        .is_err()
    );
    assert_eq!(signer.calls, 0);
}

#[test]
fn cancelled_delivery_does_not_restore_nonce() {
    let ledger = ledger();
    let delivery = redeem(
        &ledger,
        &session(BOOT_ALPN, 2),
        request(),
        101,
        &mut authority(),
        &authority(),
    )
    .unwrap();
    drop(delivery); // Equivalent to abandoning delivery after synchronous consume/sign.
    assert!(
        redeem(
            &ledger,
            &session(BOOT_ALPN, 2),
            request(),
            102,
            &mut authority(),
            &authority()
        )
        .is_err()
    );
}

#[test]
fn signer_unwind_does_not_restore_nonce() {
    struct InterruptedSigner;
    impl NodeSigner for InterruptedSigner {
        fn sign(&mut self, _: &CertificateClaims) -> Result<NodeCertificate> {
            panic!("synthetic signer interruption");
        }
    }
    let ledger = ledger();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ledger.redeem(
            &session(BOOT_ALPN, 2),
            request(),
            &binding(),
            101,
            2000,
            Zeroizing::new(vec![9]),
            &mut InterruptedSigner,
            &authority(),
        )
    }));
    assert!(outcome.is_err());
    assert!(
        redeem(
            &ledger,
            &session(BOOT_ALPN, 2),
            request(),
            102,
            &mut authority(),
            &authority()
        )
        .is_err()
    );
}

mod actual_crypto {
    use super::*;
    // The integration helper imports the public library name; mirror the fixture
    // below to avoid exposing internal test constructors or changing production API.
    use ed25519_dalek::{Signature, Signer, SigningKey};
    struct Signing(SigningKey);
    struct Verification(ed25519_dalek::VerifyingKey);
    fn bytes(c: &CertificateClaims) -> Vec<u8> {
        let mut b = b"m8s-test-only-certificate-v1\0".to_vec();
        b.extend_from_slice(&c.owner.0);
        b.extend_from_slice(&c.node.0);
        b.extend_from_slice(&c.issued_at.to_be_bytes());
        b.extend_from_slice(&c.expires_at.to_be_bytes());
        b
    }
    impl NodeSigner for Signing {
        fn sign(&mut self, c: &CertificateClaims) -> Result<NodeCertificate> {
            Ok(NodeCertificate(self.0.sign(&bytes(c)).to_bytes().to_vec()))
        }
    }
    impl CertificateVerifier for Verification {
        fn verify(&self, c: &NodeCertificate, e: &CertificateClaims) -> Result<()> {
            let s = Signature::from_slice(&c.0).map_err(|_| Error::Certificate)?;
            self.0
                .verify_strict(&bytes(e), &s)
                .map_err(|_| Error::Certificate)
        }
    }
    #[test]
    fn real_signature_claim_confusion_and_tampering_fail_closed() {
        let key = SigningKey::from_bytes(&[17; 32]);
        let verify = Verification(key.verifying_key());
        let mut signer = Signing(key);
        let claims = CertificateClaims {
            owner: binding().owner,
            node: EndpointId([2; 32]),
            issued_at: 100,
            expires_at: 200,
        };
        let mut certificate = signer.sign(&claims).unwrap();
        verify.verify(&certificate, &claims).unwrap();
        for field in 0..4 {
            let mut confused = claims.clone();
            match field {
                0 => confused.owner = EndpointId([7; 32]),
                1 => confused.node = EndpointId([7; 32]),
                2 => confused.issued_at += 1,
                _ => confused.expires_at += 1,
            };
            assert!(verify.verify(&certificate, &confused).is_err());
        }
        certificate.0[0] ^= 1;
        assert!(verify.verify(&certificate, &claims).is_err());
        certificate.0.truncate(63);
        assert!(verify.verify(&certificate, &claims).is_err());
        let ledger = ledger();
        let key = SigningKey::from_bytes(&[17; 32]);
        let mut signer = Signing(key);
        assert!(
            ledger
                .redeem(
                    &session(BOOT_ALPN, 2),
                    request(),
                    &binding(),
                    101,
                    2000,
                    Zeroizing::new(vec![9]),
                    &mut signer,
                    &verify
                )
                .is_ok()
        );
        assert!(
            ledger
                .redeem(
                    &session(BOOT_ALPN, 2),
                    request(),
                    &binding(),
                    102,
                    2000,
                    Zeroizing::new(vec![9]),
                    &mut signer,
                    &verify
                )
                .is_err()
        );
    }
    #[test]
    fn real_signature_wrong_signer_burns_before_verification_failure() {
        let verify = Verification(SigningKey::from_bytes(&[17; 32]).verifying_key());
        let mut wrong = Signing(SigningKey::from_bytes(&[18; 32]));
        let ledger = ledger();
        assert!(matches!(
            ledger.redeem(
                &session(BOOT_ALPN, 2),
                request(),
                &binding(),
                101,
                2000,
                Zeroizing::new(vec![9]),
                &mut wrong,
                &verify
            ),
            Err(Error::Certificate)
        ));
        let mut correct = Signing(SigningKey::from_bytes(&[17; 32]));
        assert!(matches!(
            ledger.redeem(
                &session(BOOT_ALPN, 2),
                request(),
                &binding(),
                102,
                2000,
                Zeroizing::new(vec![9]),
                &mut correct,
                &verify
            ),
            Err(Error::Refused)
        ));
    }
}
