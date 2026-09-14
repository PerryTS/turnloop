//! Cluster and Sentinel discovery helpers. The host owns per-node connections;
//! redirects describe exactly where to retry and whether ASKING is required.
use crate::{Error, resp::Value};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    pub slot: u16,
    pub endpoint: Endpoint,
    pub asking: bool,
}
impl Redirect {
    pub fn parse(error: &Error) -> Option<Self> {
        let mut parts = error.message.split_whitespace();
        let asking = match parts.next()? {
            "MOVED" => false,
            "ASK" => true,
            _ => return None,
        };
        let slot = parts.next()?.parse::<u16>().ok().filter(|n| *n < 16384)?;
        let (host, port) = parts.next()?.rsplit_once(':')?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            slot,
            endpoint: Endpoint {
                host: host.trim_matches(['[', ']']).into(),
                port: port.parse().ok()?,
            },
            asking,
        })
    }
}
/// CRC16/XMODEM with Redis's first non-empty {...} hash tag rules.
pub fn key_slot(mut key: &[u8]) -> u16 {
    if let Some(open) = key.iter().position(|b| *b == b'{')
        && let Some(close) = key[open + 1..].iter().position(|b| *b == b'}')
        && close != 0
    {
        key = &key[open + 1..open + 1 + close];
    }
    let mut crc = 0u16;
    for byte in key {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc % 16384
}
#[derive(Debug, Default)]
pub struct SlotMap {
    nodes: Vec<Endpoint>,
    slots: Vec<Option<usize>>,
}
impl SlotMap {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            slots: vec![None; 16384],
        }
    }
    pub fn endpoint(&self, slot: u16) -> Option<&Endpoint> {
        self.slots
            .get(usize::from(slot))
            .copied()
            .flatten()
            .and_then(|i| self.nodes.get(i))
    }
    fn assign(&mut self, start: u16, end: u16, node: Endpoint) -> Result<(), Error> {
        if start > end || end >= 16384 {
            return Err(Error::new("Invalid slot range"));
        }
        if self.slots.is_empty() {
            self.slots.resize(16384, None);
        }
        let index = self
            .nodes
            .iter()
            .position(|n| *n == node)
            .unwrap_or_else(|| {
                self.nodes.push(node);
                self.nodes.len() - 1
            });
        self.slots[usize::from(start)..=usize::from(end)].fill(Some(index));
        Ok(())
    }
    /// Atomic refresh: malformed maps never destroy the existing topology.
    pub fn update_slots(&mut self, reply: &Value, fallback_host: &str) -> Result<(), Error> {
        let mut next = Self::new();
        for range in items(reply)? {
            let range = items(range)?;
            if range.len() < 3 {
                return Err(Error::new("Invalid CLUSTER SLOTS"));
            }
            let node = items(&range[2])?;
            if node.len() < 2 {
                return Err(Error::new("Invalid cluster node"));
            }
            let host = node[0]
                .text()
                .filter(|s| !s.is_empty())
                .map(|s| s.into_owned())
                .unwrap_or_else(|| fallback_host.into());
            next.assign(
                integer(&range[0])?,
                integer(&range[1])?,
                Endpoint {
                    host,
                    port: integer(&node[1])?,
                },
            )?;
        }
        *self = next;
        Ok(())
    }
    pub fn update_shards(&mut self, reply: &Value, fallback_host: &str) -> Result<(), Error> {
        let mut next = Self::new();
        for shard in items(reply)? {
            let ranges = items(field(shard, b"slots")?)?;
            if ranges.len() % 2 != 0 {
                return Err(Error::new("Invalid shard slots"));
            }
            let nodes = items(field(shard, b"nodes")?)?;
            let master = nodes
                .iter()
                .find(|n| field(n, b"role").ok().and_then(Value::bytes) == Some(b"master"))
                .ok_or_else(|| Error::new("Shard has no master"))?;
            let host = field(master, b"endpoint")
                .ok()
                .and_then(Value::text)
                .filter(|s| !s.is_empty())
                .map(|s| s.into_owned())
                .unwrap_or_else(|| fallback_host.into());
            let node = Endpoint {
                host,
                port: integer(field(master, b"port")?)?,
            };
            for pair in ranges.as_chunks::<2>().0 {
                next.assign(integer(&pair[0])?, integer(&pair[1])?, node.clone())?;
            }
        }
        *self = next;
        Ok(())
    }
    pub fn apply_redirect(&mut self, redirect: &Redirect) -> Result<(), Error> {
        if !redirect.asking {
            self.assign(redirect.slot, redirect.slot, redirect.endpoint.clone())?;
        }
        Ok(())
    }
    /// All explicitly supplied keys must share one slot, even if different slots
    /// currently reside on the same node. Empty keys select the caller's seed.
    pub fn route_keys(&self, keys: &[&[u8]]) -> Result<Option<&Endpoint>, Error> {
        let Some(first) = keys.first() else {
            return Ok(None);
        };
        let slot = key_slot(first);
        if keys.iter().any(|k| key_slot(k) != slot) {
            return Err(Error {
                name: "ReplyError",
                message: "CROSSSLOT Keys in request don't hash to the same slot".into(),
            });
        }
        self.endpoint(slot)
            .map(Some)
            .ok_or_else(|| Error::new("Cluster slots cache is empty"))
    }
    /// Known command key layouts. Unknown/module commands require explicit keys
    /// via route_keys, avoiding unsafe guesses. Transactions pin their slot in
    /// the host for MULTI through EXEC/DISCARD.
    pub fn route_command(&self, args: &[&[u8]]) -> Result<Option<&Endpoint>, Error> {
        let name = *args.first().ok_or_else(|| Error::new("Empty command"))?;
        let is = |names: &[&[u8]]| names.iter().any(|n| name.eq_ignore_ascii_case(n));
        if is(&[
            b"PING", b"INFO", b"CLUSTER", b"AUTH", b"HELLO", b"QUIT", b"MULTI", b"EXEC",
            b"DISCARD", b"UNWATCH",
        ]) {
            return Ok(None);
        }
        if is(&[b"MGET", b"DEL", b"EXISTS", b"UNLINK", b"TOUCH", b"WATCH"]) {
            return self.route_keys(&args[1..]);
        }
        if is(&[
            b"RENAME",
            b"RENAMENX",
            b"RPOPLPUSH",
            b"LMOVE",
            b"SMOVE",
            b"BRPOPLPUSH",
            b"BLMOVE",
        ]) {
            return self.route_keys(args.get(1..3).ok_or_else(|| Error::new("Missing keys"))?);
        }
        if is(&[b"BLPOP", b"BRPOP", b"BZPOPMIN", b"BZPOPMAX"]) {
            if args.len() < 3 {
                return Err(Error::new("Missing blocking keys"));
            }
            return self.route_keys(&args[1..args.len() - 1]);
        }
        if is(&[b"MSET", b"MSETNX"]) {
            if args.len() < 3 || args.len() % 2 != 1 {
                return Err(Error::new("Invalid MSET arguments"));
            }
            let slot = key_slot(args[1]);
            if args[1..].iter().step_by(2).any(|key| key_slot(key) != slot) {
                return Err(Error::new(
                    "CROSSSLOT Keys in request don't hash to the same slot",
                ));
            }
            return self.route_keys(&args[1..2]);
        }
        if is(&[b"EVAL", b"EVALSHA", b"FCALL", b"FCALL_RO"]) {
            let count = args
                .get(2)
                .and_then(|b| std::str::from_utf8(b).ok())
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or_else(|| Error::new("Invalid numkeys"))?;
            return self.route_keys(
                args.get(
                    3..3usize
                        .checked_add(count)
                        .ok_or_else(|| Error::new("Invalid numkeys"))?,
                )
                .ok_or_else(|| Error::new("Missing script keys"))?,
            );
        }
        if is(&[
            b"GET",
            b"SET",
            b"SETEX",
            b"PSETEX",
            b"INCR",
            b"DECR",
            b"INCRBY",
            b"EXPIRE",
            b"TTL",
            b"PTTL",
            b"TYPE",
            b"HGET",
            b"HSET",
            b"HGETALL",
            b"HDEL",
            b"HLEN",
            b"LPUSH",
            b"RPUSH",
            b"LPOP",
            b"RPOP",
            b"LRANGE",
            b"SADD",
            b"SMEMBERS",
            b"ZADD",
            b"ZRANGE",
        ]) {
            return self.route_keys(args.get(1..2).ok_or_else(|| Error::new("Missing key"))?);
        }
        Err(Error::new(
            "Unknown command key layout; supply explicit keys",
        ))
    }
}
fn items(value: &Value) -> Result<&[Value], Error> {
    value.items().ok_or_else(|| Error::new("Expected array"))
}
fn integer(value: &Value) -> Result<u16, Error> {
    value
        .integer()
        .and_then(|n| u16::try_from(n).ok())
        .or_else(|| value.text()?.parse().ok())
        .ok_or_else(|| Error::new("Invalid port/slot"))
}
fn field<'a>(value: &'a Value, key: &[u8]) -> Result<&'a Value, Error> {
    match value {
        Value::Map(entries) => entries
            .iter()
            .find(|(k, _)| k.bytes() == Some(key))
            .map(|(_, v)| v),
        Value::Array(entries) => entries
            .as_chunks::<2>()
            .0
            .iter()
            .find(|p| p[0].bytes() == Some(key))
            .map(|p| &p[1]),
        _ => None,
    }
    .ok_or_else(|| Error::new("Missing discovery field"))
}
/// Reply to SENTINEL get-master-addr-by-name. Null means this Sentinel does not
/// know the group; the discovery driver should try its next configured seed.
pub fn sentinel_master(value: &Value) -> Result<Option<Endpoint>, Error> {
    if *value == Value::Null {
        return Ok(None);
    }
    let values = items(value)?;
    if values.len() != 2 {
        return Err(Error::new("Invalid Sentinel address"));
    }
    Ok(Some(Endpoint {
        host: values[0]
            .text()
            .ok_or_else(|| Error::new("Invalid Sentinel host"))?
            .into_owned(),
        port: integer(&values[1])?,
    }))
}

/// One per logical cluster request. Reuse with `reset` when its terminal event
/// is consumed. ASK never mutates the stable slot owner; MOVED does.
#[derive(Debug)]
pub struct RedirectTracker {
    remaining: u32,
}
impl Default for RedirectTracker {
    fn default() -> Self {
        Self::new(16)
    }
}
impl RedirectTracker {
    pub fn new(max_redirects: u32) -> Self {
        Self {
            remaining: max_redirects,
        }
    }
    pub fn reset(&mut self, max_redirects: u32) {
        self.remaining = max_redirects;
    }
    /// None is an ordinary ReplyError. On Some, retry on the supplied endpoint,
    /// prepending ASKING on that SAME connection when asking=true. The host
    /// suppresses intermediate redirect replies and preserves the logical token.
    pub fn follow(&mut self, error: &Error, map: &mut SlotMap) -> Result<Option<Redirect>, Error> {
        let Some(redirect) = Redirect::parse(error) else {
            return Ok(None);
        };
        if self.remaining == 0 {
            return Err(Error::new(
                "Too many Cluster redirections. Last error: MOVED/ASK",
            ));
        }
        self.remaining -= 1;
        map.apply_redirect(&redirect)?;
        Ok(Some(redirect))
    }
}

use std::time::{Duration, Instant};
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryAction {
    /// Run SENTINEL get-master-addr-by-name on this Sentinel connection.
    QueryMaster {
        sentinel: Endpoint,
        name: String,
    },
    /// Run ROLE against this candidate before publishing it to callers.
    VerifyRole {
        candidate: Endpoint,
    },
    Discovered(Endpoint),
    /// All seeds exhausted: evaluate sentinelRetryStrategy in the host.
    Retry {
        attempt: u32,
    },
    Stopped,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscoveryState {
    Master,
    Role,
    Retry,
    Waiting,
    Done,
}
/// Stateful Sentinel seed iteration and stale-master validation. Sentinel and
/// data-node credentials/TLS can differ; the host selects a Connection config
/// for QueryMaster versus VerifyRole. Transport failures call `failed`.
pub struct SentinelDiscovery {
    seeds: Vec<Endpoint>,
    name: String,
    index: usize,
    attempt: u32,
    candidate: Option<Endpoint>,
    state: DiscoveryState,
    action: Option<DiscoveryAction>,
    deadline: Option<Instant>,
}
impl SentinelDiscovery {
    pub fn new(seeds: Vec<Endpoint>, name: String) -> Result<Self, Error> {
        if seeds.is_empty() || name.is_empty() {
            return Err(Error::new("Sentinel seeds and name are required"));
        }
        let action = Some(DiscoveryAction::QueryMaster {
            sentinel: seeds[0].clone(),
            name: name.clone(),
        });
        Ok(Self {
            seeds,
            name,
            index: 0,
            attempt: 0,
            candidate: None,
            state: DiscoveryState::Master,
            action,
            deadline: None,
        })
    }
    pub fn poll_action(&mut self) -> Option<DiscoveryAction> {
        self.action.take()
    }
    pub fn reply(&mut self, value: &Value) -> Result<(), Error> {
        if self.action.is_some() {
            return Err(Error::new(
                "Consume discovery action before supplying reply",
            ));
        }
        match self.state {
            DiscoveryState::Master => match sentinel_master(value) {
                Ok(Some(candidate)) => {
                    self.candidate = Some(candidate.clone());
                    self.state = DiscoveryState::Role;
                    self.action = Some(DiscoveryAction::VerifyRole { candidate });
                }
                _ => self.failed(),
            },
            DiscoveryState::Role => {
                if value.items().and_then(|v| v.first()).and_then(Value::bytes) == Some(b"master") {
                    self.state = DiscoveryState::Done;
                    self.action = Some(DiscoveryAction::Discovered(self.candidate.take().unwrap()));
                } else {
                    self.failed();
                }
            }
            _ => return Err(Error::new("Unexpected Sentinel discovery reply")),
        }
        Ok(())
    }
    pub fn failed(&mut self) {
        if !matches!(self.state, DiscoveryState::Master | DiscoveryState::Role) {
            return;
        }
        self.candidate = None;
        self.index += 1;
        if self.index == self.seeds.len() {
            self.state = DiscoveryState::Retry;
            self.attempt = self.attempt.saturating_add(1);
            self.action = Some(DiscoveryAction::Retry {
                attempt: self.attempt,
            });
        } else {
            self.query();
        }
    }
    fn query(&mut self) {
        self.state = DiscoveryState::Master;
        self.action = Some(DiscoveryAction::QueryMaster {
            sentinel: self.seeds[self.index].clone(),
            name: self.name.clone(),
        });
    }
    pub fn retry(&mut self, now: Instant, delay: Option<Duration>) -> Result<(), Error> {
        if self.state != DiscoveryState::Retry || self.action.is_some() {
            return Err(Error::new("No Sentinel retry decision requested"));
        }
        if let Some(delay) = delay {
            self.deadline = now.checked_add(delay);
            self.state = DiscoveryState::Waiting;
        } else {
            self.state = DiscoveryState::Done;
            self.action = Some(DiscoveryAction::Stopped);
        }
        Ok(())
    }
    pub fn next_timeout(&self) -> Option<Instant> {
        self.deadline
    }
    pub fn handle_timeout(&mut self, now: Instant) {
        if self.deadline.is_some_and(|d| d <= now) {
            self.deadline = None;
            self.index = 0;
            self.query();
        }
    }
}
