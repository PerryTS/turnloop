pub fn sha256(b: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(b).into()
}

pub fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut inner = vec![0x36; 64];
    let mut outer = vec![0x5c; 64];
    for (i, b) in key.iter().enumerate() {
        inner[i] ^= *b;
        outer[i] ^= *b;
    }
    inner.extend_from_slice(message);
    outer.extend_from_slice(&sha256(&inner));
    sha256(&outer)
}
pub fn base64(bytes: &[u8]) -> String {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as usize;
        let b = chunk.get(1).copied().unwrap_or(0) as usize;
        let c = chunk.get(2).copied().unwrap_or(0) as usize;
        s.push(alphabet[a >> 2] as char);
        s.push(alphabet[((a & 3) << 4) | (b >> 4)] as char);
        s.push(if chunk.len() > 1 {
            alphabet[((b & 15) << 2) | (c >> 6)] as char
        } else {
            '='
        });
        s.push(if chunk.len() > 2 {
            alphabet[c & 63] as char
        } else {
            '='
        });
    }
    s
}
pub fn hex_salted_password() -> [u8; 32] {
    [
        96, 154, 98, 181, 182, 135, 186, 101, 146, 177, 42, 85, 44, 121, 254, 59, 241, 158, 78, 20,
        90, 22, 123, 121, 91, 122, 202, 181, 232, 159, 160, 246,
    ]
}
