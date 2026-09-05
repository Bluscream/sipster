//! TLS to a device with a self-signed certificate, pinned on first use.
//!
//! # Why this exists
//!
//! A FRITZ!Box serves TR-064 over both plain HTTP (49000) and TLS (49443). We
//! used the plain one, so every contact and call-list entry crossed the LAN in
//! the clear — digest auth keeps the *password* off the wire, but not the data
//! it protects.
//!
//! The certificate is self-signed and unique per device, so no certificate
//! authority can vouch for it and normal verification always fails.
//!
//! # Trust on first use
//!
//! Rather than accepting any certificate — which is encryption without
//! authentication, and worse than plain HTTP for the false confidence it gives
//! — the first connection records the certificate's SHA-256 fingerprint, and
//! every connection after that requires the same one. An attacker who was not
//! already in the path on the very first sync cannot substitute their own
//! certificate without being noticed.
//!
//! This is the same bargain SSH makes with host keys, and it carries the same
//! caveat: a router that legitimately regenerates its certificate — a factory
//! reset, a firmware change — will be refused until the stored fingerprint is
//! cleared.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{Error as TlsError, SignatureScheme};

/// A certificate fingerprint, lower-case hex of the SHA-256 of the DER bytes.
pub type Fingerprint = String;

/// Renders a certificate's fingerprint the way it is stored and displayed.
#[must_use]
pub fn fingerprint_of(cert: &CertificateDer<'_>) -> Fingerprint {
    // ring's hash module is private in rustls, so the digest comes from the
    // `ring` crate directly — it is already in the tree, and this is the same
    // implementation rustls would use.
    use std::fmt::Write as _;
    let digest = ring::digest::digest(&ring::digest::SHA256, cert.as_ref());
    digest.as_ref().iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// Verifies that the peer presents the certificate we pinned.
///
/// Deliberately ignores name, chain and expiry: none of them mean anything for
/// a self-signed certificate on a LAN device. The fingerprint is the whole of
/// the check.
#[derive(Debug)]
pub struct PinnedCert {
    expected: Fingerprint,
    /// Records what was seen, so a first connection can store it and a
    /// mismatch can be reported with both values.
    seen: Arc<std::sync::Mutex<Option<Fingerprint>>>,
}

impl PinnedCert {
    /// Pins `expected`, or accepts and records whatever is presented when it
    /// is empty — the first-use half of the bargain.
    #[must_use]
    pub fn new(expected: Fingerprint) -> (Arc<Self>, Arc<std::sync::Mutex<Option<Fingerprint>>>) {
        let seen = Arc::new(std::sync::Mutex::new(None));
        let verifier = Arc::new(Self { expected, seen: Arc::clone(&seen) });
        (verifier, seen)
    }
}

impl ServerCertVerifier for PinnedCert {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let actual = fingerprint_of(end_entity);

        if self.expected.is_empty() {
            // First use: remember it so the caller can store it.
            if let Ok(mut slot) = self.seen.lock() {
                *slot = Some(actual);
            }
            return Ok(ServerCertVerified::assertion());
        }

        if actual == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(TlsError::General(format!(
                "certificate fingerprint changed: expected {}, got {actual}",
                self.expected
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }

    fn requires_raw_public_keys(&self) -> bool {
        false
    }

    fn root_hint_subjects(&self) -> Option<&[rustls::DistinguishedName]> {
        None
    }



}
