//! server-discovery-and-monitoring/server-discovery-and-monitoring.md §§ Parsing
//! hello, TopologyType table, updateRSFromPrimary, Error Handling; server-selection/
//! server-selection.md §§ Read Preference, Latency Window, max-staleness.
use crate::Instant;
use crate::{
    uri::{Options, ReadPreference},
    Error, ErrorKind, Result,
};
use bson::{oid::ObjectId, Document};
use std::{
    collections::{BTreeMap, VecDeque},
    time::Duration,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerType {
    Unknown,
    PossiblePrimary,
    Standalone,
    Mongos,
    RSPrimary,
    RSSecondary,
    RSArbiter,
    RSOther,
    RSGhost,
}
impl ServerType {
    pub fn readable(self) -> bool {
        matches!(
            self,
            Self::Standalone | Self::Mongos | Self::RSPrimary | Self::RSSecondary
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopologyType {
    Unknown,
    Single,
    ReplicaSetNoPrimary,
    ReplicaSetWithPrimary,
    Sharded,
}
#[derive(Clone, Debug)]
pub struct Server {
    pub kind: ServerType,
    pub set_name: Option<String>,
    pub hosts: Vec<String>,
    pub me: Option<String>,
    pub primary: Option<String>,
    pub set_version: Option<i32>,
    pub election_id: Option<ObjectId>,
    pub min_wire_version: i32,
    pub max_wire_version: i32,
    pub session_timeout: Option<i64>,
    pub topology_version: Option<Document>,
    pub error: Option<String>,
    pub generation: u64,
    pub rtt: Option<Duration>,
    pub last_update: Instant,
    pub last_write_ms: Option<i64>,
    pub tags: BTreeMap<String, String>,
    pub next_check: Option<Instant>,
    pub hello_ok: bool,
    pub operation_count: u32,
}
impl Server {
    fn unknown(now: Instant, generation: u64) -> Self {
        Self {
            kind: ServerType::Unknown,
            set_name: None,
            hosts: Vec::new(),
            me: None,
            primary: None,
            set_version: None,
            election_id: None,
            min_wire_version: 0,
            max_wire_version: 0,
            session_timeout: None,
            topology_version: None,
            error: None,
            generation,
            rtt: None,
            last_update: now,
            last_write_ms: None,
            tags: BTreeMap::new(),
            next_check: Some(now),
            hello_ok: false,
            operation_count: 0,
        }
    }
    fn parse(d: &Document, now: Instant, old: &Server, rtt: Duration, heartbeat: Duration) -> Self {
        let mut s = Self::unknown(now, old.generation);
        s.next_check = Some(now + heartbeat);
        s.hello_ok = d.get_bool("helloOk").unwrap_or(false);
        let ok = d
            .get_f64("ok")
            .ok()
            .or_else(|| d.get_i32("ok").ok().map(f64::from))
            .unwrap_or(0.0)
            != 0.0;
        if !ok {
            s.error = Some(d.get_str("errmsg").unwrap_or("hello failed").to_owned());
            return s;
        }
        s.kind = if d.get_str("msg").ok() == Some("isdbgrid") {
            ServerType::Mongos
        } else if d.get_bool("isreplicaset").ok() == Some(true) {
            ServerType::RSGhost
        } else if d.contains_key("setName") {
            if d.get_bool("hidden").ok() == Some(true) {
                ServerType::RSOther
            } else if d
                .get_bool("isWritablePrimary")
                .or_else(|_| d.get_bool("ismaster"))
                .unwrap_or(false)
            {
                ServerType::RSPrimary
            } else if d.get_bool("secondary").ok() == Some(true) {
                ServerType::RSSecondary
            } else if d.get_bool("arbiterOnly").ok() == Some(true) {
                ServerType::RSArbiter
            } else {
                ServerType::RSOther
            }
        } else {
            ServerType::Standalone
        };
        s.set_name = d.get_str("setName").ok().map(str::to_owned);
        s.me = d.get_str("me").ok().map(str::to_lowercase);
        s.primary = d.get_str("primary").ok().map(str::to_lowercase);
        for key in ["hosts", "passives", "arbiters"] {
            if let Ok(a) = d.get_array(key) {
                s.hosts
                    .extend(a.iter().filter_map(|b| b.as_str().map(str::to_lowercase)));
            }
        }
        s.set_version = d.get_i32("setVersion").ok();
        s.election_id = d.get_object_id("electionId").ok();
        s.min_wire_version = d.get_i32("minWireVersion").unwrap_or(0);
        s.max_wire_version = d.get_i32("maxWireVersion").unwrap_or(0);
        s.session_timeout = integer(d, "logicalSessionTimeoutMinutes");
        s.topology_version = d.get_document("topologyVersion").ok().cloned();
        s.last_write_ms = d
            .get_document("lastWrite")
            .ok()
            .and_then(|w| w.get_datetime("lastWriteDate").ok())
            .map(|v| v.timestamp_millis());
        if let Ok(tags) = d.get_document("tags") {
            s.tags = tags
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                .collect();
        }
        s.rtt = Some(
            old.rtt
                .map_or(rtt, |old| old.mul_f64(0.8) + rtt.mul_f64(0.2)),
        );
        s.operation_count = old.operation_count;
        s
    }
}
#[derive(Clone, Debug)]
pub enum TopologyEvent {
    Check { address: String, hello: bool },
    ServerAdded(String),
    ServerRemoved(String),
    ServerChanged(String),
    TopologyChanged(TopologyType),
    ClearPool { address: String, generation: u64 },
}
pub struct Topology {
    pub kind: TopologyType,
    pub set_name: Option<String>,
    pub servers: BTreeMap<String, Server>,
    pub max_set_version: Option<i32>,
    pub max_election_id: Option<ObjectId>,
    pub heartbeat: Duration,
    pub local_threshold: Duration,
    seed_count: usize,
    events: VecDeque<TopologyEvent>,
}
impl Topology {
    pub fn new(options: &Options, now: Instant) -> Self {
        let kind = if options.direct {
            TopologyType::Single
        } else if options.replica_set.is_some() {
            TopologyType::ReplicaSetNoPrimary
        } else {
            TopologyType::Unknown
        };
        let servers = options
            .seeds
            .iter()
            .map(|a| (a.authority(), Server::unknown(now, 0)))
            .collect();
        Self {
            kind,
            set_name: options.replica_set.clone(),
            servers,
            max_set_version: None,
            max_election_id: None,
            heartbeat: options.heartbeat,
            local_threshold: options.local_threshold,
            seed_count: options.seeds.len(),
            events: VecDeque::with_capacity(16),
        }
    }
    pub fn next_timeout(&self) -> Option<Instant> {
        self.servers.values().filter_map(|s| s.next_check).min()
    }
    pub fn handle_timeout(&mut self, now: Instant) {
        for (a, s) in &mut self.servers {
            if s.next_check.is_some_and(|d| d <= now) {
                s.next_check = None;
                self.events.push_back(TopologyEvent::Check {
                    address: a.clone(),
                    hello: s.hello_ok,
                });
            }
        }
    }
    pub fn request_check(&mut self, now: Instant) {
        for s in self.servers.values_mut() {
            let at = (s.last_update + Duration::from_millis(500)).max(now);
            s.next_check = Some(s.next_check.map_or(at, |d| d.min(at)));
        }
    }
    pub fn poll_event(&mut self) -> Option<TopologyEvent> {
        self.events.pop_front()
    }
    pub fn update(&mut self, address: &str, hello: &Document, now: Instant, rtt: Duration) {
        let address = address.to_ascii_lowercase();
        let Some(old) = self.servers.get(&address) else {
            return;
        };
        if older(
            old.topology_version.as_ref(),
            hello.get_document("topologyVersion").ok(),
            false,
        ) {
            return;
        }
        let mut s = Server::parse(hello, now, old, rtt, self.heartbeat);
        let before = self.kind;
        if s.kind == ServerType::Unknown {
            s.generation += 1;
            self.events.push_back(TopologyEvent::ClearPool {
                address: address.clone(),
                generation: s.generation,
            });
        }
        self.servers.insert(address.clone(), s.clone());
        if self.kind == TopologyType::Single {
            if self.set_name.is_some() && self.set_name != s.set_name {
                let mut unknown = Server::unknown(now, s.generation);
                unknown.next_check = Some(now + self.heartbeat);
                self.servers.insert(address.clone(), unknown);
            }
        } else if self.kind == TopologyType::Sharded {
            if !matches!(s.kind, ServerType::Unknown | ServerType::Mongos) {
                self.remove(&address);
            }
        } else {
            match s.kind {
                ServerType::Standalone => {
                    if self.kind == TopologyType::Unknown && self.seed_count == 1 {
                        self.kind = TopologyType::Single;
                    } else {
                        self.remove(&address);
                    }
                }
                ServerType::Mongos => {
                    if self.kind == TopologyType::Unknown {
                        self.kind = TopologyType::Sharded;
                    } else {
                        self.remove(&address);
                    }
                }
                ServerType::RSPrimary
                | ServerType::RSSecondary
                | ServerType::RSArbiter
                | ServerType::RSOther => {
                    if self.set_name.is_none() {
                        self.set_name = s.set_name.clone();
                    }
                    if self.set_name != s.set_name {
                        self.remove(&address);
                    } else if s.kind == ServerType::RSPrimary {
                        let stale = if s.max_wire_version >= 17 {
                            (s.election_id, s.set_version)
                                < (self.max_election_id, self.max_set_version)
                        } else {
                            s.set_version.is_some()
                                && s.election_id.is_some()
                                && self.max_set_version.is_some()
                                && self.max_election_id.is_some()
                                && (s.set_version, s.election_id)
                                    < (self.max_set_version, self.max_election_id)
                        };
                        if stale {
                            let mut unknown = Server::unknown(now, s.generation);
                            unknown.next_check = Some(now + self.heartbeat);
                            unknown.error = Some(
                                "primary marked stale due to electionId/setVersion mismatch".into(),
                            );
                            self.servers.insert(address.clone(), unknown);
                        } else {
                            if s.max_wire_version >= 17 {
                                self.max_election_id = s.election_id;
                                self.max_set_version = s.set_version;
                            } else {
                                if s.election_id.is_some() && s.set_version.is_some() {
                                    self.max_election_id = s.election_id;
                                }
                                self.max_set_version = self.max_set_version.max(s.set_version);
                            }
                            for (a, other) in &mut self.servers {
                                if a != &address && other.kind == ServerType::RSPrimary {
                                    let generation = other.generation;
                                    *other = Server::unknown(now, generation);
                                    other.error = Some(
                                        "primary marked stale due to discovery of newer primary"
                                            .into(),
                                    );
                                }
                            }
                            self.add_hosts(&s.hosts, now);
                            let remove: Vec<_> = self
                                .servers
                                .keys()
                                .filter(|a| !s.hosts.contains(a))
                                .cloned()
                                .collect();
                            for a in remove {
                                self.remove(&a);
                            }
                        }
                    } else {
                        if self.kind != TopologyType::ReplicaSetWithPrimary {
                            self.add_hosts(&s.hosts, now);
                            if let Some(primary) = &s.primary {
                                if let Some(p) = self.servers.get_mut(primary) {
                                    if p.kind == ServerType::Unknown {
                                        p.kind = ServerType::PossiblePrimary;
                                    }
                                }
                            }
                        }
                        if s.me.as_ref().is_some_and(|me| me != &address) {
                            self.remove(&address);
                        }
                    }
                    if self.kind == TopologyType::Unknown {
                        self.kind = TopologyType::ReplicaSetNoPrimary;
                    }
                }
                _ => {}
            }
            if matches!(
                self.kind,
                TopologyType::ReplicaSetNoPrimary | TopologyType::ReplicaSetWithPrimary
            ) {
                self.check_primary();
            }
        }
        if self.servers.contains_key(&address) {
            self.events.push_back(TopologyEvent::ServerChanged(address));
        }
        if before != self.kind {
            self.events
                .push_back(TopologyEvent::TopologyChanged(self.kind));
        }
    }
    fn add_hosts(&mut self, hosts: &[String], now: Instant) {
        for host in hosts {
            if !self.servers.contains_key(host) {
                self.servers.insert(host.clone(), Server::unknown(now, 0));
                self.events
                    .push_back(TopologyEvent::ServerAdded(host.clone()));
            }
        }
    }
    fn remove(&mut self, address: &str) {
        if self.servers.remove(address).is_some() {
            self.events
                .push_back(TopologyEvent::ServerRemoved(address.to_owned()));
        }
    }
    fn check_primary(&mut self) {
        self.kind = if self
            .servers
            .values()
            .any(|s| s.kind == ServerType::RSPrimary)
        {
            TopologyType::ReplicaSetWithPrimary
        } else {
            TopologyType::ReplicaSetNoPrimary
        };
    }
    pub fn session_timeout(&self) -> Option<i64> {
        let mut timeout = None;
        for s in self.servers.values().filter(|s| s.kind.readable()) {
            let n = s.session_timeout?;
            timeout = Some(timeout.map_or(n, |v: i64| v.min(n)));
        }
        timeout
    }
    /// Wire 6 through 27: MongoDB 3.6 through 8.2. Unknown servers do not constrain.
    pub fn compatible(&self) -> bool {
        self.servers.values().all(|s| {
            matches!(s.kind, ServerType::Unknown | ServerType::PossiblePrimary)
                || (s.min_wire_version <= 27 && s.max_wire_version >= 6)
        })
    }
    pub fn application_error(&mut self, address: &str, error: ApplicationError<'_>, now: Instant) {
        let Some(s) = self.servers.get_mut(address) else {
            return;
        };
        if error.generation < s.generation
            || older(
                s.topology_version.as_ref(),
                error
                    .response
                    .and_then(|d| d.get_document("topologyVersion").ok()),
                true,
            )
        {
            return;
        }
        let d = error.response;
        let detail = d
            .and_then(|d| d.get_document("writeConcernError").ok())
            .or(d);
        let code = detail.and_then(|d| d.get_i32("code").ok());
        let message = detail.and_then(|d| d.get_str("errmsg").ok()).unwrap_or("");
        let state_change = code.map_or_else(
            || {
                message.contains("not master")
                    || message.contains("not primary")
                    || message.contains("node is recovering")
            },
            |c| matches!(c, 11600 | 11602 | 13436 | 189 | 91 | 10107 | 13435 | 10058),
        );
        if error.kind == ApplicationErrorKind::Timeout && !state_change {
            return;
        }
        if state_change || error.kind == ApplicationErrorKind::Network || !error.handshake_complete
        {
            let clear = error.kind != ApplicationErrorKind::Command
                || !error.handshake_complete
                || matches!(code, Some(11600 | 91))
                || error.max_wire_version < 8;
            let generation = s.generation + u64::from(clear);
            let tv = if state_change {
                d.and_then(|d| d.get_document("topologyVersion").ok())
                    .cloned()
            } else {
                None
            };
            *s = Server::unknown(now, generation);
            s.error = Some(message.to_owned());
            s.topology_version = tv;
            s.next_check = Some(now + Duration::from_millis(500));
            if clear {
                self.events.push_back(TopologyEvent::ClearPool {
                    address: address.into(),
                    generation,
                });
            }
            if matches!(
                self.kind,
                TopologyType::ReplicaSetWithPrimary | TopologyType::ReplicaSetNoPrimary
            ) {
                self.check_primary();
            }
        }
    }
    /// Writes use Primary; host supplies random entropy for unbiased selection from
    /// the latency window. `candidates` reuses a caller-owned vector.
    pub fn candidates<'a>(
        &'a self,
        pref: ReadPreference,
        tags: &[BTreeMap<String, String>],
        max_staleness: Option<Duration>,
        out: &mut Vec<&'a str>,
    ) -> Result<()> {
        self.candidates_deprioritized(pref, tags, max_staleness, &[], out)
    }
    pub fn candidates_deprioritized<'a>(
        &'a self,
        pref: ReadPreference,
        tags: &[BTreeMap<String, String>],
        max_staleness: Option<Duration>,
        excluded: &[&str],
        out: &mut Vec<&'a str>,
    ) -> Result<()> {
        self.candidates_inner(pref, tags, max_staleness, excluded, out)?;
        if out.is_empty() && !excluded.is_empty() {
            self.candidates_inner(pref, tags, max_staleness, &[], out)?;
        }
        Ok(())
    }
    /// Power-of-two choice; supply independent uniformly distributed host entropy.
    pub fn choose<'a>(&self, candidates: &[&'a str], entropy: [u64; 2]) -> Option<&'a str> {
        if candidates.is_empty() {
            return None;
        }
        let a = ((entropy[0] as u128 * candidates.len() as u128) >> 64) as usize;
        if candidates.len() == 1 {
            return Some(candidates[0]);
        }
        let mut b = ((entropy[1] as u128 * (candidates.len() - 1) as u128) >> 64) as usize;
        if b >= a {
            b += 1;
        }
        Some(
            if self.servers.get(candidates[a])?.operation_count
                <= self.servers.get(candidates[b])?.operation_count
            {
                candidates[a]
            } else {
                candidates[b]
            },
        )
    }
    pub fn operation_started(&mut self, address: &str) -> Result<()> {
        let s = self
            .servers
            .get_mut(address)
            .ok_or_else(|| Error::protocol("Selected server was removed"))?;
        s.operation_count = s
            .operation_count
            .checked_add(1)
            .ok_or_else(|| Error::protocol("Server operation count overflow"))?;
        Ok(())
    }
    pub fn operation_finished(&mut self, address: &str) {
        if let Some(s) = self.servers.get_mut(address) {
            s.operation_count = s.operation_count.saturating_sub(1);
        }
    }
    fn candidates_inner<'a>(
        &'a self,
        pref: ReadPreference,
        tags: &[BTreeMap<String, String>],
        max_staleness: Option<Duration>,
        excluded: &[&str],
        out: &mut Vec<&'a str>,
    ) -> Result<()> {
        out.clear();
        if !self.compatible() {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "Incompatible MongoDB wire version",
            ));
        }
        if pref == ReadPreference::Primary
            && (tags.iter().any(|t| !t.is_empty()) || max_staleness.is_some())
        {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "Primary read preference does not allow tags or staleness",
            ));
        }
        if matches!(
            self.kind,
            TopologyType::ReplicaSetWithPrimary | TopologyType::ReplicaSetNoPrimary
        ) && max_staleness.is_some_and(|n| {
            n < Duration::from_secs(90) || n < self.heartbeat + Duration::from_secs(10)
        }) {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "maxStalenessSeconds is too small",
            ));
        }
        let primary = self
            .servers
            .iter()
            .filter(|(a, _)| !excluded.contains(&a.as_str()))
            .map(|(_, s)| s)
            .find(|s| s.kind == ServerType::RSPrimary);
        let max_write = self
            .servers
            .values()
            .filter(|s| s.kind == ServerType::RSSecondary)
            .filter_map(|s| s.last_write_ms)
            .max();
        let filter = |s: &Server| -> bool {
            if matches!(self.kind, TopologyType::Single | TopologyType::Sharded) {
                return !matches!(
                    s.kind,
                    ServerType::Unknown | ServerType::PossiblePrimary | ServerType::RSGhost
                );
            }
            let candidate = match pref {
                ReadPreference::Primary => s.kind == ServerType::RSPrimary,
                ReadPreference::PrimaryPreferred if primary.is_some() => {
                    s.kind == ServerType::RSPrimary
                }
                ReadPreference::Secondary
                | ReadPreference::SecondaryPreferred
                | ReadPreference::PrimaryPreferred => s.kind == ServerType::RSSecondary,
                ReadPreference::Nearest => {
                    matches!(s.kind, ServerType::RSPrimary | ServerType::RSSecondary)
                }
            };
            if !candidate {
                return false;
            }
            if s.kind == ServerType::RSSecondary {
                if let Some(max) = max_staleness {
                    let Some(write) = s.last_write_ms else {
                        return false;
                    };
                    let stale = if let Some(p) = primary {
                        let Some(pw) = p.last_write_ms else {
                            return false;
                        };
                        let update_delta = if s.last_update >= p.last_update {
                            s.last_update.duration_since(p.last_update).as_millis() as i128
                        } else {
                            -(p.last_update.duration_since(s.last_update).as_millis() as i128)
                        };
                        update_delta + pw as i128 - write as i128
                            + self.heartbeat.as_millis() as i128
                    } else {
                        max_write.unwrap_or(write) as i128 - write as i128
                            + self.heartbeat.as_millis() as i128
                    };
                    if stale > max.as_millis() as i128 {
                        return false;
                    }
                }
            }
            true
        };
        for (a, s) in &self.servers {
            if !excluded.contains(&a.as_str()) && filter(s) {
                out.push(a);
            }
        }
        if !matches!(self.kind, TopologyType::Single | TopologyType::Sharded)
            && !(pref == ReadPreference::PrimaryPreferred && primary.is_some())
            && !tags.is_empty()
        {
            let matching = tags.iter().find(|t| {
                out.iter().any(|a| {
                    t.iter()
                        .all(|(k, v)| self.servers[*a].tags.get(k) == Some(v))
                })
            });
            if let Some(t) = matching {
                out.retain(|a| {
                    t.iter()
                        .all(|(k, v)| self.servers[*a].tags.get(k) == Some(v))
                });
            } else {
                out.clear();
            }
        }
        if out.is_empty() && pref == ReadPreference::SecondaryPreferred {
            out.extend(
                self.servers
                    .iter()
                    .filter(|(a, s)| {
                        !excluded.contains(&a.as_str()) && s.kind == ServerType::RSPrimary
                    })
                    .map(|(a, _)| a.as_str()),
            );
        }
        if let Some(min) = out.iter().filter_map(|a| self.servers[*a].rtt).min() {
            out.retain(|a| {
                self.servers[*a]
                    .rtt
                    .is_some_and(|r| r <= min + self.local_threshold)
            });
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplicationErrorKind {
    Command,
    Network,
    Timeout,
}
pub struct ApplicationError<'a> {
    pub generation: u64,
    pub max_wire_version: i32,
    pub handshake_complete: bool,
    pub kind: ApplicationErrorKind,
    pub response: Option<&'a Document>,
}
fn integer(d: &Document, k: &str) -> Option<i64> {
    d.get_i64(k)
        .ok()
        .or_else(|| d.get_i32(k).ok().map(i64::from))
}
fn older(current: Option<&Document>, incoming: Option<&Document>, equal: bool) -> bool {
    let Some((c, i)) = current.zip(incoming) else {
        return false;
    };
    if c.get_object_id("processId").ok() != i.get_object_id("processId").ok() {
        return false;
    }
    match (integer(c, "counter"), integer(i, "counter")) {
        (Some(c), Some(i)) => {
            if equal {
                c >= i
            } else {
                c > i
            }
        }
        _ => false,
    }
}
