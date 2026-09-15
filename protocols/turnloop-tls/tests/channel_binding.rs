use turnloop_tls::tls_server_end_point;

const CERTIFICATES: &[&[u8]] = &[
    include_bytes!("certificates/rsa-md5.der"),
    include_bytes!("certificates/rsa-sha1.der"),
    include_bytes!("certificates/rsa-sha256.der"),
    include_bytes!("certificates/rsa-sha384.der"),
    include_bytes!("certificates/rsa-sha512.der"),
    include_bytes!("certificates/ecdsa-sha256.der"),
    include_bytes!("certificates/ecdsa-sha384.der"),
    include_bytes!("certificates/ecdsa-sha512.der"),
    include_bytes!("certificates/pss-sha1.der"),
    include_bytes!("certificates/pss-sha256.der"),
    include_bytes!("certificates/pss-sha384.der"),
    include_bytes!("certificates/pss-sha512.der"),
    include_bytes!("certificates/pss-sha384-mgf256.der"),
    include_bytes!("certificates/ed25519.der"),
];

#[test]
fn real_signature_algorithms_match_independent_certificate_digests() {
    let expected: Vec<_> = include_str!("certificates/digests.txt").lines().collect();
    assert_eq!(expected.len(), 14);
    assert_eq!(CERTIFICATES.len(), expected.len());
    let mut hashes = 0;
    for (certificate, expected) in CERTIFICATES.iter().zip(expected) {
        let (name, hex) = expected.split_once(' ').expect("name and digest");
        let digest = tls_server_end_point(certificate);
        if name == "ed25519" {
            assert!(digest.is_none());
        } else {
            let digest = digest.expect(name);
            let actual: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(actual, hex, "{name}");
            hashes += 1;
        }
    }
    assert_eq!(hashes, 13);
}

#[test]
fn truncated_and_malformed_certificates_never_produce_binding() {
    let mut truncated = 0;
    for certificate in &CERTIFICATES[..13] {
        assert!(tls_server_end_point(certificate).is_some());
        for n in 0..certificate.len() {
            assert!(
                tls_server_end_point(&certificate[..n]).is_none(),
                "prefix {n}"
            );
            truncated += 1;
        }
        let mut trailing = certificate.to_vec();
        trailing.push(0);
        assert!(tls_server_end_point(&trailing).is_none());
        let mut tag = certificate.to_vec();
        tag[0] = 0x31;
        assert!(tls_server_end_point(&tag).is_none());
    }
    assert!(truncated > 5000);
    for bad in [
        &[0x30, 0x80][..],                        // indefinite length
        &[0x30, 0x81, 0x7f],                      // nonminimal length
        &[0x30, 0x82, 0, 0x80],                   // leading zero
        &[0x30, 0x89, 1, 0, 0, 0, 0, 0, 0, 0, 0], // overflow
        &[0x30, 0xff],
    ] {
        assert!(tls_server_end_point(bad).is_none());
    }
}

fn tlv(tag: u8, value: &[u8]) -> Vec<u8> {
    assert!(value.len() < 128);
    [vec![tag, value.len() as u8], value.to_vec()].concat()
}
fn envelope(algorithm: &[u8]) -> Vec<u8> {
    tlv(
        0x30,
        &[tlv(0x30, &[2, 1, 1]), tlv(0x30, algorithm), tlv(3, &[0, 1])].concat(),
    )
}
#[test]
fn malformed_signature_identifiers_and_pss_parameters_are_rejected() {
    let pss_oid = tlv(6, b"\x2a\x86\x48\x86\xf7\x0d\x01\x01\x0a");
    let sha256 = tlv(0x30, &tlv(6, b"\x60\x86\x48\x01\x65\x03\x04\x02\x01"));
    let hash = tlv(0xa0, &sha256);
    let pss = |params: &[u8]| envelope(&[pss_oid.clone(), tlv(0x30, params)].concat());
    assert!(tls_server_end_point(&pss(&[])).is_some()); // SHA-1 default
    assert!(tls_server_end_point(&pss(&hash)).is_some());
    for params in [
        vec![0xa0, 0x80],
        tlv(0xa0, &[0x30, 0x03, 0x06, 0x01, 0xff]), // unknown hash
        tlv(0xa0, &[sha256.clone(), vec![0]].concat()), // trailing hash bytes
        [hash.clone(), hash.clone()].concat(),      // duplicate
        [tlv(0xa2, &[2, 1, 20]), hash].concat(),    // out of order
        tlv(0xa1, &sha256),                         // not MGF1
        tlv(0xa2, &[2, 1, 0xff]),                   // negative salt length
        tlv(0xa2, &[2, 2, 0, 20]),                  // nonminimal integer
        tlv(0xa2, &[2, 0]),                         // empty integer
        tlv(0xa3, &[2, 1, 2]),                      // unsupported trailer
        tlv(0xa4, &[]),                             // unknown parameter
    ] {
        assert!(tls_server_end_point(&pss(&params)).is_none(), "{params:x?}");
    }
    assert!(tls_server_end_point(&envelope(&pss_oid)).is_none()); // missing PSS parameters
    let rsa = tlv(6, b"\x2a\x86\x48\x86\xf7\x0d\x01\x01\x0b");
    assert!(tls_server_end_point(&envelope(&rsa)).is_some());
    for params in [vec![5, 1, 0], vec![5, 0, 5, 0], vec![2, 1, 1]] {
        assert!(tls_server_end_point(&envelope(&[rsa.clone(), params].concat())).is_none());
    }
    let ec = tlv(6, b"\x2a\x86\x48\xce\x3d\x04\x03\x02");
    assert!(tls_server_end_point(&envelope(&[ec, vec![5, 0]].concat())).is_none());
}
