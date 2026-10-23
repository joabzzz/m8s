#![cfg(feature = "iroh-transport")]
//! Real TLS protocol tests with a test-only real Ed25519 certificate authority.
//! NOT MP-001 cryptographic admission or provider verification. Explicitly ignored
//! in ordinary CI because BSD netmon AF_ROUTE/RAW is denied in the managed sandbox.
use iroh::{Endpoint, EndpointAddr, endpoint::presets};
use m8s_bootstrap::{transport::*, *};
use std::{net::Ipv4Addr, time::Duration};
use zeroize::Zeroizing;

async fn endpoint() -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .clear_ip_transports().bind_addr("127.0.0.1:0").unwrap()
        .clear_relay_transports().clear_address_lookup()
        .alpns(vec![BOOT_ALPN.to_vec(),CONTROL_ALPN.to_vec()])
        .bind().await.expect("real transport unavailable: requires UDP loopback and BSD AF_ROUTE/RAW netmon capability; do not disable netmon")
}
fn addr(endpoint: &Endpoint) -> EndpointAddr {
    EndpointAddr::new(endpoint.id()).with_ip_addr(*endpoint.bound_sockets().first().unwrap())
}
fn id(endpoint: &Endpoint) -> EndpointId {
    EndpointId(*endpoint.id().as_bytes())
}
mod support;

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires UDP loopback and BSD netmon; new owner-initiated direction, not historical E qualification"]
async fn owner_initiated_bootstrap_preserves_identity_and_single_use() {
    tokio::time::timeout(Duration::from_secs(25), async {
        let owner = endpoint().await;
        let node = endpoint().await;
        let stranger = endpoint().await;
        let binding = LaunchBinding {
            owner: id(&owner),
            instance_id: "synthetic-reverse-i".into(),
            container_id: "synthetic-reverse-c".into(),
            provider_ip: Ipv4Addr::LOCALHOST.into(),
            image_sha256: [3; 32],
        };
        let ledger = Ledger::new(rusqlite::Connection::open_in_memory().unwrap()).unwrap();
        ledger
            .issue(&BootNonce::new([42; 32]), &binding, 100)
            .unwrap();
        let (mut signing, verifier) = support::authority();
        let (_, client_verifier) = support::authority();
        for replay in [false, true] {
            let request = Redemption {
                nonce: BootNonce::new([42; 32]),
                instance_id: binding.instance_id.clone(),
                container_id: binding.container_id.clone(),
                node: id(&node),
                image_sha256: binding.image_sha256,
                self_destruct_present: true,
            };
            let server = async {
                if !replay {
                    // A completed handshake from an unpinned owner receives no
                    // redemption stream, and does not consume the nonce.
                    let wrong = stranger.connect(addr(&node), BOOT_ALPN).await.unwrap();
                    assert!(wrong.accept_bi().await.is_err());
                }
                let mut boot = connect_boot_to_node(&owner, addr(&node)).await.unwrap();
                assert_eq!(boot.peer_endpoint(), id(&node));
                let request = boot.receive().await.unwrap();
                let pending = ledger.redeem(
                    boot.session(),
                    request,
                    &binding,
                    100,
                    1000,
                    Zeroizing::new(vec![9]),
                    &mut signing,
                    &verifier,
                );
                if replay {
                    assert!(matches!(pending, Err(Error::Refused)));
                } else {
                    let pending = pending
                        .unwrap()
                        .with_settings(BundleSettings {
                            model: "synthetic-model".into(),
                            config: Zeroizing::new(vec![]),
                            heartbeat_ttl_seconds: 30,
                        })
                        .unwrap();
                    boot.deliver(pending, 100).await.unwrap();
                }
            };
            let client =
                accept_verified_from_owner(&node, id(&owner), request, &client_verifier, || 100);
            let (_, receipt) = tokio::join!(server, client);
            if replay {
                assert!(receipt.is_err());
            } else {
                assert!(receipt.unwrap().matches_resource(
                    &binding.instance_id,
                    &binding.container_id,
                    &binding.image_sha256,
                ));
            }
        }
        stranger.close().await;
        node.close().await;
        owner.close().await;
    })
    .await
    .expect("bounded owner-initiated TLS qualification");
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires approved UDP + BSD AF_ROUTE/RAW runtime or supported separate runner; test-only Ed25519 certificate encoding, not MP001 admission"]
async fn real_tls_bootstrap_and_single_use_control_frame() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let owner = endpoint().await;
        let node = endpoint().await;
        // Full handshake with an endpoint-supported but wrong application ALPN
        // must not become a boot proof. No synthetic authenticated constructor.
        let reject_server = async {
            assert!(matches!(
                accept_boot(owner.accept().await.unwrap(), id(&owner)).await,
                Err(Error::Refused)
            ));
        };
        let wrong_alpn_client = async {
            let connection = node.connect(addr(&owner), CONTROL_ALPN).await.unwrap();
            connection.closed().await;
        };
        tokio::join!(reject_server, wrong_alpn_client);
        let binding = LaunchBinding {
            owner: id(&owner),
            instance_id: "synthetic-i".into(),
            container_id: "synthetic-c".into(),
            provider_ip: Ipv4Addr::LOCALHOST.into(),
            image_sha256: [3; 32],
        };
        let (mut signing, server_verifier) = support::authority();
        let (_, client_verifier) = support::authority();
        let ledger = Ledger::new(rusqlite::Connection::open_in_memory().unwrap()).unwrap();
        ledger
            .issue(&BootNonce::new([42; 32]), &binding, 100)
            .unwrap();
        let request = Redemption {
            nonce: BootNonce::new([42; 32]),
            instance_id: binding.instance_id.clone(),
            container_id: binding.container_id.clone(),
            node: id(&node),
            image_sha256: [3; 32],
            self_destruct_present: true,
        };
        // An owner-pinned client cannot claim a node key whose private key it
        // does not hold. Refusal occurs before opening a network connection.
        let confused = Redemption {
            nonce: BootNonce::new([42; 32]),
            instance_id: binding.instance_id.clone(),
            container_id: binding.container_id.clone(),
            node: id(&owner),
            image_sha256: binding.image_sha256,
            self_destruct_present: true,
        };
        assert!(matches!(
            redeem_over_iroh(&node, addr(&owner), confused, &client_verifier, || 100).await,
            Err(Error::Refused)
        ));
        let server = async {
            let mut boot = accept_boot(owner.accept().await.unwrap(), id(&owner))
                .await
                .unwrap();
            let request = boot.receive().await.unwrap();
            let pending = ledger
                .redeem(
                    boot.session(),
                    request,
                    &binding,
                    100,
                    1000,
                    Zeroizing::new(vec![9]),
                    &mut signing,
                    &server_verifier,
                )
                .unwrap()
                .with_settings(BundleSettings {
                    model: "synthetic-model".into(),
                    config: Zeroizing::new(vec![8]),
                    heartbeat_ttl_seconds: 60,
                })
                .unwrap();
            boot.deliver(pending, 100).await.unwrap();
        };
        let client = redeem_over_iroh(&node, addr(&owner), request, &client_verifier, || 100);
        let (_, bundle) = tokio::join!(server, client);
        let bundle = bundle.unwrap();
        assert_eq!(bundle.claims.node, id(&node));
        assert_eq!(*bundle.join_token, vec![9]);
        // Reconnect and decrypt another genuine peer-authenticated request, but
        // fail the already consumed nonce before another signing/delivery effect.
        let replay_server = async {
            let mut boot = accept_boot(owner.accept().await.unwrap(), id(&owner))
                .await
                .unwrap();
            let request = boot.receive().await.unwrap();
            assert!(matches!(
                ledger.redeem(
                    boot.session(),
                    request,
                    &binding,
                    101,
                    1000,
                    Zeroizing::new(vec![9]),
                    &mut signing,
                    &server_verifier
                ),
                Err(Error::Refused)
            ));
        };
        let replay_request = Redemption {
            nonce: BootNonce::new([42; 32]),
            instance_id: binding.instance_id.clone(),
            container_id: binding.container_id.clone(),
            node: id(&node),
            image_sha256: binding.image_sha256,
            self_destruct_present: true,
        };
        let replay_client = redeem_over_iroh(
            &node,
            addr(&owner),
            replay_request,
            &client_verifier,
            || 101,
        );
        let (_, replay_result) = tokio::join!(replay_server, replay_client);
        assert!(replay_result.is_err());
        let policy = ControlPolicy::new(id(&owner), [], [], 1000).unwrap();
        let server = async {
            let mut session = accept_control(&node, id(&owner), &policy, 101)
                .await
                .unwrap();
            assert_eq!(session.peer_endpoint(), owner.id().to_string());
            // Only an actual received frame creates evidence. Consume once using
            // its receive timestamp, never a detached cached-session authority.
            let evidence = session.receive_heartbeat(|| 101).await.unwrap();
            assert_eq!(evidence.sequence(), 1);
            assert_eq!(evidence.node_endpoint(), id(&node));
            assert_eq!(evidence.peer_endpoint(), owner.id().to_string());
            let applied = evidence
                .consume(id(&node), 101, |peer, at| (peer.to_owned(), at))
                .unwrap();
            assert_eq!(applied, (owner.id().to_string(), 101));
            let evidence = session.receive_heartbeat(|| 102).await.unwrap();
            assert_eq!(evidence.sequence(), 2);
            assert!(
                evidence
                    .consume(id(&owner), 102, |_, _| panic!(
                        "wrong-node evidence must not invoke bridge"
                    ))
                    .is_err()
            );
            let evidence = session.receive_heartbeat(|| 103).await.unwrap();
            assert_eq!(evidence.sequence(), 3);
            // Revocation updates original shared policy after IO; evidence is
            // still bound to it and cannot substitute a permissive snapshot.
            policy.update([], [id(&owner)], 1000).unwrap();
            assert!(
                evidence
                    .consume(id(&node), 103, |_, _| panic!(
                        "revoked evidence must not invoke bridge"
                    ))
                    .is_err()
            );
        };
        let client = async {
            let connection = owner.connect(addr(&node), CONTROL_ALPN).await.unwrap();
            send_heartbeat(&connection, 1).await.unwrap();
            send_heartbeat(&connection, 2).await.unwrap();
            send_heartbeat(&connection, 3).await.unwrap();
            connection.closed().await;
        };
        tokio::join!(server, client);
        owner.close().await;
        node.close().await;
    })
    .await
    .expect("bounded local TLS test deadline");
}
