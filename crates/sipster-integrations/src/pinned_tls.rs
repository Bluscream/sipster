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

/// The first certificate accepted in this process, and the pin for every
/// connection after it.
///
/// Without this, "first use" meant *every* connection: the learned fingerprint
/// only reaches the config when a sync finishes, so a single sync opened a
/// dozen connections that each accepted whatever they were given and verified
/// nothing. Remembering it here closes the window to the one connection that
/// genuinely has nothing to compare against.
///
/// Never cleared. A router that legitimately changes certificate is picked up
/// on the next start, which is also when the config's stored pin is re-read.
static FIRST_SEEN: std::sync::Mutex<Option<Fingerprint>> = std::sync::Mutex::new(None);

impl PinnedCert {
    /// Pins `expected`, or accepts and records whatever is presented when it
    /// is empty — the first-use half of the bargain.
    #[must_use]
    pub fn new(expected: Fingerprint) -> (Arc<Self>, Arc<std::sync::Mutex<Option<Fingerprint>>>) {
        // A pin learned earlier in this process stands in for a configured
        // one, so only the very first connection is unverified.
        let expected = if expected.is_empty() {
            FIRST_SEEN
                .lock()
                .ok()
                .and_then(|seen| seen.clone())
                .unwrap_or_default()
        } else {
            expected
        };
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
            // First use: remember it so the caller can store it, and pin it
            // for the rest of this process straight away.
            if let Ok(mut slot) = self.seen.lock() {
                *slot = Some(actual.clone());
            }
            if let Ok(mut first) = FIRST_SEEN.lock() {
                first.get_or_insert(actual);
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

/// A `ureq` TLS connector that pins the certificate and tolerates a peer that
/// closes without `close_notify`.
///
/// # Why not ureq's own
///
/// ureq already tries to treat a missing `close_notify` as a clean end of
/// stream, but its check is stale: it looks for `ConnectionAborted` carrying
/// the text `CloseNotify`, while rustls 0.23 reports `UnexpectedEof` with
/// `close_notify`. Nothing matches, so the error escapes.
///
/// That matters here because a FRITZ!Box serves `phonebook.lua` with
/// `Connection: close` and no `Content-Length` — the body *ends* at the close
/// — and then drops the socket without the closing handshake. Strictly it is
/// the router's fault, but the data is complete and refusing it only means no
/// contacts. Every download failed while the SOAP calls succeeded, so the sync
/// reported success with zero contacts.
///
/// Truncation is detectable here precisely because the body is framed by the
/// close; a response with a `Content-Length` is still checked against it by
/// the layer above, so this does not hide a short read.
#[derive(Debug)]
pub struct PinnedConnector {
    config: Arc<rustls::ClientConfig>,
}

impl PinnedConnector {
    #[must_use]
    pub fn new(config: Arc<rustls::ClientConfig>) -> Self {
        Self { config }
    }
}

impl ureq::TlsConnector for PinnedConnector {
    fn connect(
        &self,
        dns_name: &str,
        io: Box<dyn ureq::ReadWrite>,
    ) -> Result<Box<dyn ureq::ReadWrite>, ureq::Error> {
        // A LAN device is usually reached by IP, which is not a valid SNI
        // name. The name is irrelevant to a pinned certificate, so an
        // unusable one falls back to a placeholder rather than failing.
        let server_name = ServerName::try_from(dns_name.to_owned())
            .unwrap_or_else(|_| ServerName::try_from("localhost").expect("a valid literal"));

        let connection = rustls::ClientConnection::new(Arc::clone(&self.config), server_name)
            .map_err(|e| std::io::Error::other(format!("TLS handshake setup failed: {e}")))?;

        Ok(Box::new(LenientStream(rustls::StreamOwned::new(connection, io))))
    }
}

/// A TLS stream that reports a missing `close_notify` as end of stream.
#[derive(Debug)]
struct LenientStream(rustls::StreamOwned<rustls::ClientConnection, Box<dyn ureq::ReadWrite>>);

/// Whether this is the "peer went away without saying goodbye" error.
fn is_unclean_close(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionAborted
    ) && e.to_string().to_ascii_lowercase().contains("close_notify")
}

impl std::io::Read for LenientStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.0.read(buf) {
            Err(ref e) if is_unclean_close(e) => Ok(0),
            other => other,
        }
    }
}

impl std::io::Write for LenientStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl ureq::ReadWrite for LenientStream {
    fn socket(&self) -> Option<&std::net::TcpStream> {
        self.0.get_ref().socket()
    }
}

#[cfg(test)]
mod tests {
    use super::{fingerprint_of, is_unclean_close, PinnedCert};
    use rustls::client::danger::ServerCertVerifier;
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

    /// Any bytes will do: the verifier only ever hashes them.
    fn cert(bytes: &'static [u8]) -> CertificateDer<'static> {
        CertificateDer::from(bytes)
    }

    fn verify(verifier: &PinnedCert, cert: &CertificateDer<'_>) -> Result<(), rustls::Error> {
        let name = ServerName::try_from("router.invalid").expect("a valid name");
        verifier
            .verify_server_cert(cert, &[], &name, &[], UnixTime::now())
            .map(|_| ())
    }

    #[test]
    fn a_fingerprint_is_lower_case_hex_of_a_sha256() {
        let fingerprint = fingerprint_of(&cert(b"anything"));
        assert_eq!(fingerprint.len(), 64);
        assert!(fingerprint
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
    }

    #[test]
    fn different_certificates_have_different_fingerprints() {
        assert_ne!(fingerprint_of(&cert(b"one")), fingerprint_of(&cert(b"two")));
    }

    #[test]
    fn the_pinned_certificate_is_accepted() {
        let presented = cert(b"the router");
        let (verifier, _) = PinnedCert::new(fingerprint_of(&presented));
        assert!(verify(&verifier, &presented).is_ok());
    }

    #[test]
    fn a_different_certificate_is_refused() {
        let (verifier, _) = PinnedCert::new(fingerprint_of(&cert(b"the router")));
        // The whole point of pinning: an impostor must not be accepted just
        // because it offers a well-formed self-signed certificate.
        assert!(verify(&verifier, &cert(b"an impostor")).is_err());
    }

    #[test]
    fn a_first_connection_reports_what_it_saw() {
        let presented = cert(b"a router we have never met");
        let (verifier, seen) = PinnedCert::new(String::new());
        assert!(verify(&verifier, &presented).is_ok());
        assert_eq!(
            seen.lock().expect("not poisoned").as_deref(),
            Some(fingerprint_of(&presented).as_str())
        );
    }

    #[test]
    fn a_missing_close_notify_reads_as_end_of_stream() {
        let err = std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "peer closed connection without sending TLS close_notify",
        );
        assert!(is_unclean_close(&err));
    }

    #[test]
    fn a_genuine_truncation_is_still_an_error() {
        // Without this distinction the lenient read would hide a short body
        // rather than just a missing handshake.
        let err = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "failed to fill whole buffer");
        assert!(!is_unclean_close(&err));
    }

    #[test]
    fn an_unrelated_error_is_not_treated_as_end_of_stream() {
        let err = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "connection reset by peer");
        assert!(!is_unclean_close(&err));
    }
}
