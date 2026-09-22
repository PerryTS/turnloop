//! auth/auth.md §§ SCRAM-SHA-1, SCRAM-SHA-256 and mongodb-handshake/handshake.md
//! § Speculative Authentication. Entropy is provided by the host, never acquired here;
//! see the crate's [host entropy](crate#host-entropy) obligations.
use crate::{Error, ErrorKind, Result, uri::Credential};
use base64::{Engine, engine::general_purpose::STANDARD};
use bson::{Binary, Document, doc, spec::BinarySubtype};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Digest;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mechanism {
    Sha1,
    Sha256,
}
impl Mechanism {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "SCRAM-SHA-1" => Ok(Self::Sha1),
            "SCRAM-SHA-256" => Ok(Self::Sha256),
            _ => Err(auth_error("Unsupported authentication mechanism")),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sha1 => "SCRAM-SHA-1",
            Self::Sha256 => "SCRAM-SHA-256",
        }
    }
}
pub struct Scram {
    mechanism: Mechanism,
    nonce: String,
    first: String,
    password: String,
    conversation: Option<i32>,
    expected: Vec<u8>,
    state: u8,
}
impl Scram {
    pub fn new(credential: &Credential, mechanism: Mechanism, nonce: &str) -> Result<Self> {
        if nonce.len() < 16
            || nonce
                .bytes()
                .any(|c| !(0x21..=0x7e).contains(&c) || c == b',')
        {
            return Err(auth_error(
                "Host must provide a cryptographically random printable nonce of at least 16 bytes",
            ));
        }
        let username = credential.username.replace('=', "=3D").replace(',', "=2C");
        let password = match mechanism {
            Mechanism::Sha1 => {
                use std::fmt::Write;
                let digest = md5::Md5::digest(
                    format!("{}:mongo:{}", credential.username, credential.password).as_bytes(),
                );
                let mut hex = String::with_capacity(32);
                for byte in digest {
                    write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
                }
                hex
            }
            Mechanism::Sha256 => stringprep::saslprep(&credential.password)
                .map_err(|_| auth_error("Password cannot be SASLprep normalized"))?
                .into_owned(),
        };
        Ok(Self {
            mechanism,
            nonce: nonce.into(),
            first: format!("n={username},r={nonce}"),
            password,
            conversation: None,
            expected: Vec::new(),
            state: 0,
        })
    }
    pub fn start(&mut self, source: &str, speculative: bool) -> Document {
        self.state = 1;
        let mut d = doc! {"saslStart":1,"mechanism":self.mechanism.as_str(),"payload":binary(format!("n,,{}",self.first).into_bytes()),"options":{"skipEmptyExchange":true}};
        if speculative {
            d.insert("db", source);
        } else {
            d.insert("$db", source);
        }
        d
    }
    pub fn receive(&mut self, response: &Document, source: &str) -> Result<Option<Document>> {
        let id = response
            .get_i32("conversationId")
            .map_err(|_| auth_error("Missing SCRAM conversation id"))?;
        if self.conversation.is_some_and(|v| v != id) {
            return Err(auth_error("SCRAM conversation id changed"));
        }
        self.conversation = Some(id);
        let payload = response
            .get_binary_generic("payload")
            .map_err(|_| auth_error("Missing SCRAM payload"))?;
        let done = response
            .get_bool("done")
            .map_err(|_| auth_error("Missing SCRAM done"))?;
        let payload =
            std::str::from_utf8(payload).map_err(|_| auth_error("Invalid SCRAM UTF-8"))?;
        let next = match self.state {
            1 => {
                if done {
                    return Err(auth_error("SCRAM ended before server proof"));
                }
                let attrs = parse_attributes(payload)?;
                if attrs.iter().any(|(k, _)| *k == "m") {
                    return Err(auth_error("Unsupported SCRAM extension"));
                }
                let get = |k| {
                    attrs
                        .iter()
                        .find(|(key, _)| *key == k)
                        .map(|(_, v)| *v)
                        .ok_or_else(|| auth_error("Missing SCRAM attribute"))
                };
                let nonce = get("r")?;
                if !nonce.starts_with(&self.nonce) || nonce.len() <= self.nonce.len() {
                    return Err(auth_error(
                        "Server SCRAM nonce does not extend client nonce",
                    ));
                }
                let salt = STANDARD
                    .decode(get("s")?)
                    .map_err(|_| auth_error("Invalid SCRAM salt"))?;
                let rounds = get("i")?
                    .parse::<u32>()
                    .map_err(|_| auth_error("Invalid SCRAM iteration count"))?;
                if !(4096..=1_000_000).contains(&rounds) {
                    return Err(auth_error(
                        "SCRAM iteration count outside supported range 4096..1000000",
                    ));
                }
                let final_bare = format!("c=biws,r={nonce}");
                let auth_message = format!("{},{payload},{final_bare}", self.first);
                macro_rules! derive {
                    ($hash:ty,$len:expr) => {{
                        let mut salted = [0u8; $len];
                        pbkdf2::pbkdf2_hmac::<$hash>(
                            self.password.as_bytes(),
                            &salt,
                            rounds,
                            &mut salted,
                        );
                        let mut client = <Hmac<$hash>>::new_from_slice(&salted).unwrap();
                        client.update(b"Client Key");
                        let key = client.finalize().into_bytes();
                        let stored = <$hash>::digest(key);
                        let mut sig = <Hmac<$hash>>::new_from_slice(&stored).unwrap();
                        sig.update(auth_message.as_bytes());
                        let sig = sig.finalize().into_bytes();
                        let proof: Vec<u8> = key.iter().zip(sig).map(|(a, b)| a ^ b).collect();
                        let mut server = <Hmac<$hash>>::new_from_slice(&salted).unwrap();
                        server.update(b"Server Key");
                        let mut expected =
                            <Hmac<$hash>>::new_from_slice(&server.finalize().into_bytes()).unwrap();
                        expected.update(auth_message.as_bytes());
                        self.expected = expected.finalize().into_bytes().to_vec();
                        salted.fill(0);
                        proof
                    }};
                }
                let proof = match self.mechanism {
                    Mechanism::Sha1 => derive!(sha1::Sha1, 20),
                    Mechanism::Sha256 => derive!(sha2::Sha256, 32),
                };
                self.password.clear();
                self.state = 2;
                format!("{final_bare},p={}", STANDARD.encode(proof)).into_bytes()
            }
            2 => {
                let attrs = parse_attributes(payload)?;
                if attrs.iter().any(|(k, _)| *k == "e") {
                    return Err(auth_error("SCRAM server rejected authentication"));
                }
                let signature = attrs
                    .iter()
                    .find(|(k, _)| *k == "v")
                    .ok_or_else(|| auth_error("Missing SCRAM server signature"))?
                    .1;
                let signature = STANDARD
                    .decode(signature)
                    .map_err(|_| auth_error("Invalid SCRAM server signature"))?;
                // Constant-time over the fixed digest length, with length checked separately.
                let difference = signature
                    .iter()
                    .zip(&self.expected)
                    .fold(0u8, |acc, (a, b)| acc | (a ^ b));
                if signature.len() != self.expected.len() || difference != 0 {
                    return Err(auth_error("SCRAM server signature mismatch"));
                }
                self.state = if done { 4 } else { 3 };
                if done {
                    return Ok(None);
                }
                Vec::new()
            }
            3 => {
                if !done || !payload.is_empty() {
                    return Err(auth_error("Invalid SCRAM final exchange"));
                }
                self.state = 4;
                return Ok(None);
            }
            _ => return Err(auth_error("Unexpected SCRAM response")),
        };
        Ok(Some(
            doc! {"saslContinue":1,"conversationId":id,"payload":binary(next),"$db":source},
        ))
    }
}
fn parse_attributes(s: &str) -> Result<Vec<(&str, &str)>> {
    let mut a = Vec::new();
    for part in s.split(',') {
        let (k, v) = part
            .split_once('=')
            .ok_or_else(|| auth_error("Malformed SCRAM attribute"))?;
        if k.len() != 1 || a.iter().any(|(x, _)| *x == k) {
            return Err(auth_error("Duplicate or malformed SCRAM attribute"));
        }
        a.push((k, v));
    }
    Ok(a)
}
fn binary(bytes: Vec<u8>) -> Binary {
    Binary {
        subtype: BinarySubtype::Generic,
        bytes,
    }
}
fn auth_error(s: &'static str) -> Error {
    Error::new(ErrorKind::Authentication, s)
}
