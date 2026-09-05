//! HTTP digest authentication, as TR-064 asks for it.

use std::collections::HashMap;

use md5::{Digest, Md5};

/// A random 16-hex-digit client nonce.
pub(super) fn fresh_cnonce() -> String {
    // uuid v4 is already a dependency and is backed by a CSPRNG; its simple
    // form gives us 32 hex digits, of which 16 are plenty.
    uuid::Uuid::new_v4().simple().to_string()[..16].to_string()
}

pub(super) fn md5_hex(data: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

pub(super) fn parse_auth_header(header: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let header = header.trim_start_matches("Digest ");
    for part in header.split(',') {
        if let Some((k, v)) = part.trim().split_once('=') {
            let key = k.trim().to_string();
            let val = v.trim().trim_matches('"').to_string();
            map.insert(key, val);
        }
    }
    map
}

pub(super) fn build_digest_header(
    user: &str,
    pass: &str,
    method: &str,
    uri: &str,
    auth_params: &HashMap<String, String>,
) -> String {
    let realm = auth_params.get("realm").cloned().unwrap_or_default();
    let nonce = auth_params.get("nonce").cloned().unwrap_or_default();
    let qop = auth_params.get("qop").cloned();

    let ha1 = md5_hex(format!("{user}:{realm}:{pass}").as_bytes());
    let ha2 = md5_hex(format!("{method}:{uri}").as_bytes());

    if let Some(qop_val) = qop.as_deref() {
        if qop_val.contains("auth") {
            let nc = "00000001";
            // A fresh client nonce per request. The previous constant
            // ("0a4f113b", straight out of the RFC 2617 example) meant every
            // request produced an identical digest for a given server nonce,
            // which is exactly what cnonce exists to prevent.
            let cnonce = fresh_cnonce();
            let resp = md5_hex(format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}").as_bytes());
            return format!(
                "Digest username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{resp}\", qop=auth, nc={nc}, cnonce=\"{cnonce}\""
            );
        }
    }

    let response = md5_hex(format!("{ha1}:{nonce}:{ha2}").as_bytes());
    format!(
        "Digest username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response}\""
    )
}

#[cfg(test)]
mod tests {
    use super::fresh_cnonce;

    /// A constant client nonce defeats digest replay protection.
    #[test]
    fn client_nonces_differ_between_requests() {
        let (a, b) = (fresh_cnonce(), fresh_cnonce());
        assert_ne!(a, b);
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
