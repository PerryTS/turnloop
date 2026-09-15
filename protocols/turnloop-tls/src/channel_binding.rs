//! RFC 5929 certificate binding. This reads only the outer certificate envelope
//! and signature AlgorithmIdentifier; certificate verification belongs to rustls.
use ring::digest::{self, Algorithm, Digest, SHA256, SHA384, SHA512};

/// Hash a verified server leaf's DER for RFC 5929 §4.1 tls-server-end-point.
/// RSA/ECDSA SHA-256/384/512 and RSA-PSS parameters are supported. MD5/SHA-1
/// signatures use SHA-256. Unknown algorithms (including Ed25519), malformed DER
/// and undefined bindings return `None`. This helper does not verify certificates
/// and performs no allocation; call it after successful TLS authentication.
pub fn tls_server_end_point(certificate: &[u8]) -> Option<Digest> {
    Some(digest::digest(signature_hash(certificate)?, certificate))
}

const RSA: &[u8] = b"\x2a\x86\x48\x86\xf7\x0d\x01\x01";
const ECDSA: &[u8] = b"\x2a\x86\x48\xce\x3d\x04\x03";
const SHA2: &[u8] = b"\x60\x86\x48\x01\x65\x03\x04\x02";
const SHA1: &[u8] = b"\x2b\x0e\x03\x02\x1a";

/// Definite, minimally encoded DER lengths only. Each sub-reader is bounded to
/// its parent value; no offsets, recursion or heap storage escape that slice.
struct Der<'a>(&'a [u8]);
impl<'a> Der<'a> {
    fn byte(&mut self) -> Option<u8> {
        let (&first, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(first)
    }
    fn value(&mut self, tag: u8) -> Option<&'a [u8]> {
        if self.byte()? != tag {
            return None;
        }
        let first = self.byte()?;
        let len = if first < 128 {
            first as usize
        } else {
            let count = (first & 127) as usize;
            if count == 0 || count > size_of::<usize>() || self.0.first() == Some(&0) {
                return None;
            }
            let mut len = 0usize;
            for _ in 0..count {
                len = len.checked_mul(256)?.checked_add(self.byte()? as usize)?;
            }
            if len < 128 {
                return None;
            }
            len
        };
        let (value, rest) = self.0.split_at_checked(len)?;
        self.0 = rest;
        Some(value)
    }
    fn end(self) -> Option<()> {
        self.0.is_empty().then_some(())
    }
    fn null_or_absent(self) -> Option<()> {
        matches!(self.0, [] | [5, 0]).then_some(())
    }
    fn integer(&mut self) -> Option<usize> {
        let value = self.value(2)?;
        let first = *value.first()?;
        if first & 128 != 0 || (value.len() > 1 && first == 0 && value[1] & 128 == 0) {
            return None;
        }
        value
            .iter()
            .try_fold(0usize, |n, b| n.checked_mul(256)?.checked_add(*b as usize))
    }
}

fn signature_hash(certificate: &[u8]) -> Option<&'static Algorithm> {
    let mut der = Der(certificate);
    let mut cert = Der(der.value(0x30)?);
    der.end()?;
    if cert.value(0x30)?.is_empty() {
        // TBSCertificate is opaque to this reader.
        return None;
    }
    let mut alg = Der(cert.value(0x30)?);
    let oid = alg.value(6)?;
    // Certificate signatures are octet-aligned BIT STRINGs, not empty strings.
    let signature = cert.value(3)?;
    if signature.len() < 2 || signature[0] != 0 {
        return None;
    }
    cert.end()?;
    if let Some(suffix) = oid.strip_prefix(RSA) {
        if suffix == [10] {
            // id-RSASSA-PSS
            let params = alg.value(0x30)?;
            alg.end()?;
            return pss_hash(params);
        }
        alg.null_or_absent()?;
        match suffix {
            [4 | 5 | 11] => Some(&SHA256), // MD5, SHA-1, SHA-256
            [12] => Some(&SHA384),
            [13] => Some(&SHA512),
            _ => None,
        }
    } else if let Some(suffix) = oid.strip_prefix(ECDSA) {
        alg.end()?; // ECDSA parameters MUST be absent.
        match suffix {
            [2] => Some(&SHA256),
            [3] => Some(&SHA384),
            [4] => Some(&SHA512),
            _ => None,
        }
    } else {
        None
    }
}

fn hash_identifier(bytes: &[u8]) -> Option<&'static Algorithm> {
    let mut der = Der(bytes);
    let mut alg = Der(der.value(0x30)?);
    der.end()?;
    let oid = alg.value(6)?;
    alg.null_or_absent()?;
    if oid == SHA1 {
        return Some(&SHA256);
    }
    match oid.strip_prefix(SHA2)? {
        [1] => Some(&SHA256),
        [2] => Some(&SHA384),
        [3] => Some(&SHA512),
        _ => None,
    }
}

fn pss_hash(parameters: &[u8]) -> Option<&'static Algorithm> {
    // RFC 4055: absent hashAlgorithm defaults to SHA-1 (RFC 5929 -> SHA-256).
    // MGF1's hash is validated separately; it does not select the binding hash.
    let mut hash = &SHA256;
    let mut params = Der(parameters);
    let mut previous = 0;
    while let Some(&tag) = params.0.first() {
        if !(0xa0..=0xa3).contains(&tag) || tag <= previous {
            return None;
        }
        previous = tag;
        let field = params.value(tag)?;
        match tag {
            0xa0 => hash = hash_identifier(field)?,
            0xa1 => {
                let mut der = Der(field);
                let mut mgf = Der(der.value(0x30)?);
                der.end()?;
                if mgf.value(6)?.strip_prefix(RSA)? != [8] {
                    // id-mgf1
                    return None;
                }
                hash_identifier(mgf.0)?;
            }
            0xa2 | 0xa3 => {
                let mut der = Der(field);
                let integer = der.integer()?;
                der.end()?;
                if tag == 0xa3 && integer != 1 {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(hash)
}
