//! Actual Ed25519 test signer/verifier, NOT MP-001 certificate encoding or a
//! production signer. Synthetic deterministic key never used outside tests.
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use m8s_bootstrap::{
    CertificateClaims, CertificateVerifier, Error, NodeCertificate, NodeSigner, Result,
};

pub struct TestSigner(SigningKey);
pub struct TestVerifier(VerifyingKey);

pub fn authority() -> (TestSigner, TestVerifier) {
    let signing = SigningKey::from_bytes(&[17; 32]);
    let verifier = TestVerifier(signing.verifying_key());
    (TestSigner(signing), verifier)
}
fn claim_bytes(claims: &CertificateClaims) -> Vec<u8> {
    // Public fixed-width, domain-separated claims. No private key material.
    let mut bytes = b"m8s-test-only-certificate-v1\0".to_vec();
    bytes.extend_from_slice(&claims.owner.0);
    bytes.extend_from_slice(&claims.node.0);
    bytes.extend_from_slice(&claims.issued_at.to_be_bytes());
    bytes.extend_from_slice(&claims.expires_at.to_be_bytes());
    bytes
}
impl NodeSigner for TestSigner {
    fn sign(&mut self, claims: &CertificateClaims) -> Result<NodeCertificate> {
        Ok(NodeCertificate(
            self.0.sign(&claim_bytes(claims)).to_bytes().to_vec(),
        ))
    }
}
impl CertificateVerifier for TestVerifier {
    fn verify(&self, certificate: &NodeCertificate, expected: &CertificateClaims) -> Result<()> {
        let signature = Signature::from_slice(&certificate.0).map_err(|_| Error::Certificate)?;
        self.0
            .verify_strict(&claim_bytes(expected), &signature)
            .map_err(|_| Error::Certificate)
    }
}
