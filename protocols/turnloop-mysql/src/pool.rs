//! Sans-I/O pool: the host executes Connect/Close requests, then reports their
//! completion. Checkout events are FIFO; idle reuse is LIFO, as in pg/mysql2.
use crate::Instant;
use std::{collections::VecDeque, time::Duration};
const MYSQL: bool = true;
#[derive(Debug, Clone)]
pub struct Config {
    pub max: usize,
    pub min: usize,
    pub max_idle: usize,
    pub idle_timeout: Option<Duration>,
    pub wait_for_connections: bool,
    pub queue_limit: Option<usize>,
    pub max_uses: Option<u64>,
    pub max_lifetime: Option<Duration>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            max: 10,
            min: 0,
            max_idle: 10,
            idle_timeout: Some(Duration::from_secs(if MYSQL { 60 } else { 10 })),
            wait_for_connections: true,
            queue_limit: None,
            max_uses: None,
            max_lifetime: None,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionId {
    slot: usize,
    generation: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    pub connection: ConnectionId,
    pub token: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Closed,
    QueueLimit,
    NoConnections,
    Timeout,
    ConnectionFailed,
    InvalidLease,
    DuplicateToken,
    InvalidConfig,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Closed => {
                if MYSQL {
                    "Pool is closed."
                } else {
                    "Cannot use a pool after calling end on the pool"
                }
            }
            Self::QueueLimit => "Queue limit reached.",
            Self::NoConnections => "No connections available.",
            Self::Timeout => "timeout exceeded when trying to connect",
            Self::ConnectionFailed => "connection attempt failed",
            Self::InvalidLease => {
                "Release called on client which has already been released to the pool."
            }
            Self::DuplicateToken => "duplicate checkout token",
            Self::InvalidConfig => "invalid pool configuration",
        })
    }
}
impl std::error::Error for Error {}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Connect(ConnectionId),
    Connected(ConnectionId),
    Acquired(Lease),
    CheckoutFailed { token: u64, error: Error },
    Released(ConnectionId),
    Close(ConnectionId),
    Removed(ConnectionId),
    Ended,
}
#[derive(Debug, Clone, Copy)]
struct Request {
    token: u64,
    deadline: Option<Instant>,
}
#[derive(Debug, Clone, Copy)]
enum State {
    Empty,
    Connecting(Request),
    Busy(u64),
    Idle(Instant),
    Closing,
}
struct Slot {
    generation: u64,
    state: State,
    born: Instant,
    uses: u64,
}
pub struct Pool {
    config: Config,
    slots: Vec<Slot>,
    idle: VecDeque<ConnectionId>,
    waiting: VecDeque<Request>,
    events: VecDeque<Event>,
    ending: bool,
    ended: bool,
}
impl Pool {
    pub fn new(config: Config) -> Result<Self, Error> {
        if config.max == 0 || config.min > config.max || config.max_uses == Some(0) {
            return Err(Error::InvalidConfig);
        }
        Ok(Self {
            config,
            slots: Vec::new(),
            idle: VecDeque::new(),
            waiting: VecDeque::new(),
            events: VecDeque::new(),
            ending: false,
            ended: false,
        })
    }
    pub fn next_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }
    pub fn total_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| !matches!(s.state, State::Empty))
            .count()
    }
    fn live_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| !matches!(s.state, State::Empty | State::Closing))
            .count()
    }
    pub fn idle_count(&self) -> usize {
        self.idle.len()
    }
    pub fn waiting_count(&self) -> usize {
        self.waiting.len()
    }
    fn slot(&mut self, id: ConnectionId) -> Result<&mut Slot, Error> {
        let s = self.slots.get_mut(id.slot).ok_or(Error::InvalidLease)?;
        if s.generation != id.generation || matches!(s.state, State::Empty) {
            return Err(Error::InvalidLease);
        }
        Ok(s)
    }
    /// Accepted checkouts yield exactly one Acquired or CheckoutFailed event.
    pub fn checkout(
        &mut self,
        token: u64,
        now: Instant,
        deadline: Option<Instant>,
    ) -> Result<(), Error> {
        if self.ending {
            return Err(Error::Closed);
        }
        if self.waiting.iter().any(|r| r.token == token)
            || self.slots.iter().any(|s| match s.state {
                State::Connecting(r) => r.token == token,
                State::Busy(t) => t == token,
                _ => false,
            })
        {
            return Err(Error::DuplicateToken);
        }
        if self.idle.is_empty() && self.total_count() >= self.config.max {
            if !self.config.wait_for_connections {
                return Err(Error::NoConnections);
            }
            if self
                .config
                .queue_limit
                .is_some_and(|max| max != 0 && self.waiting.len() >= max)
            {
                return Err(Error::QueueLimit);
            }
        }
        self.waiting.push_back(Request { token, deadline });
        self.schedule(now);
        Ok(())
    }
    fn schedule(&mut self, now: Instant) {
        if self.ending {
            return;
        }
        while let Some(r) = self.waiting.front().copied() {
            if r.deadline.is_some_and(|t| t <= now) {
                self.waiting.pop_front();
                self.events.push_back(Event::CheckoutFailed {
                    token: r.token,
                    error: Error::Timeout,
                });
                continue;
            }
            if let Some(id) = self.idle.pop_back() {
                self.waiting.pop_front();
                let s = &mut self.slots[id.slot];
                s.state = State::Busy(r.token);
                s.uses += 1;
                self.events.push_back(Event::Acquired(Lease {
                    connection: id,
                    token: r.token,
                }));
                continue;
            }
            if self.total_count() >= self.config.max {
                break;
            }
            self.waiting.pop_front();
            let index = self
                .slots
                .iter()
                .position(|s| matches!(s.state, State::Empty))
                .unwrap_or(self.slots.len());
            if index == self.slots.len() {
                self.slots.push(Slot {
                    generation: 0,
                    state: State::Empty,
                    born: now,
                    uses: 0,
                });
            }
            let s = &mut self.slots[index];
            s.generation += 1;
            s.state = State::Connecting(r);
            s.born = now;
            s.uses = 0;
            self.events.push_back(Event::Connect(ConnectionId {
                slot: index,
                generation: s.generation,
            }));
        }
    }
    pub fn connected(&mut self, id: ConnectionId, now: Instant) -> Result<(), Error> {
        let s = self.slot(id)?;
        if matches!(s.state, State::Closing) {
            return Ok(());
        }
        let State::Connecting(r) = s.state else {
            return Err(Error::InvalidLease);
        };
        if r.deadline.is_some_and(|t| t <= now) {
            self.events.push_back(Event::CheckoutFailed {
                token: r.token,
                error: Error::Timeout,
            });
            self.close(id);
            return Ok(());
        }
        s.born = now;
        s.state = State::Busy(r.token);
        s.uses = 1;
        self.events.push_back(Event::Connected(id));
        self.events.push_back(Event::Acquired(Lease {
            connection: id,
            token: r.token,
        }));
        Ok(())
    }
    pub fn connect_failed(&mut self, id: ConnectionId, now: Instant) -> Result<(), Error> {
        let s = self.slot(id)?;
        let state = s.state;
        if !matches!(state, State::Connecting(_) | State::Closing) {
            return Err(Error::InvalidLease);
        }
        s.state = State::Empty;
        if let State::Connecting(r) = state {
            self.events.push_back(Event::CheckoutFailed {
                token: r.token,
                error: Error::ConnectionFailed,
            });
        }
        self.events.push_back(Event::Removed(id));
        self.schedule(now);
        self.maybe_ended();
        Ok(())
    }
    fn close(&mut self, id: ConnectionId) {
        self.idle.retain(|v| *v != id);
        self.slots[id.slot].state = State::Closing;
        self.events.push_back(Event::Close(id));
    }
    pub fn checkin(&mut self, lease: Lease, now: Instant, destroy: bool) -> Result<(), Error> {
        let s = self.slot(lease.connection)?;
        if !matches!(s.state,State::Busy(t) if t==lease.token) {
            return Err(Error::InvalidLease);
        }
        let uses = s.uses;
        let born = s.born;
        self.events.push_back(Event::Released(lease.connection));
        if destroy
            || self.ending
            || self.config.max_uses.is_some_and(|m| uses >= m)
            || self
                .config
                .max_lifetime
                .is_some_and(|d| !d.is_zero() && now.saturating_duration_since(born) >= d)
            || (self.waiting.is_empty() && self.idle.len() >= self.config.max_idle)
        {
            self.close(lease.connection);
        } else {
            self.slots[lease.connection.slot].state = State::Idle(now);
            self.idle.push_back(lease.connection);
        }
        self.schedule(now);
        Ok(())
    }
    pub fn closed(&mut self, id: ConnectionId, now: Instant) -> Result<(), Error> {
        let s = self.slot(id)?;
        if !matches!(s.state, State::Closing) {
            return Err(Error::InvalidLease);
        }
        s.state = State::Empty;
        self.events.push_back(Event::Removed(id));
        self.schedule(now);
        self.maybe_ended();
        Ok(())
    }
    /// Report a broken pooled connection. Any connecting request fails once.
    pub fn connection_error(&mut self, id: ConnectionId) -> Result<(), Error> {
        let state = self.slot(id)?.state;
        if matches!(state, State::Closing) {
            return Ok(());
        }
        if let State::Connecting(r) = state {
            self.events.push_back(Event::CheckoutFailed {
                token: r.token,
                error: Error::ConnectionFailed,
            });
        }
        self.close(id);
        Ok(())
    }
    pub fn next_timeout(&self) -> Option<Instant> {
        let count = self.live_count();
        self.waiting
            .iter()
            .filter_map(|r| r.deadline)
            .chain(self.slots.iter().filter_map(|s| match s.state {
                State::Connecting(r) => r.deadline,
                State::Idle(since) => {
                    let idle = if count > self.config.min {
                        self.config
                            .idle_timeout
                            .filter(|d| !d.is_zero())
                            .and_then(|d| since.checked_add(d))
                    } else {
                        None
                    };
                    let lifetime = self
                        .config
                        .max_lifetime
                        .filter(|d| !d.is_zero())
                        .and_then(|d| s.born.checked_add(d));
                    idle.into_iter().chain(lifetime).min()
                }
                _ => None,
            }))
            .min()
    }
    pub fn handle_timeout(&mut self, now: Instant) {
        let mut i = 0;
        while i < self.waiting.len() {
            if self.waiting[i].deadline.is_some_and(|t| t <= now) {
                let r = self.waiting.remove(i).unwrap();
                self.events.push_back(Event::CheckoutFailed {
                    token: r.token,
                    error: Error::Timeout,
                });
            } else {
                i += 1;
            }
        }
        for i in 0..self.slots.len() {
            let s = &self.slots[i];
            let id = ConnectionId {
                slot: i,
                generation: s.generation,
            };
            match s.state {
                State::Connecting(r) if r.deadline.is_some_and(|t| t <= now) => {
                    self.events.push_back(Event::CheckoutFailed {
                        token: r.token,
                        error: Error::Timeout,
                    });
                    self.close(id);
                }
                State::Idle(since)
                    if (self.live_count() > self.config.min
                        && self.config.idle_timeout.is_some_and(|d| {
                            !d.is_zero() && now.saturating_duration_since(since) >= d
                        }))
                        || self.config.max_lifetime.is_some_and(|d| {
                            !d.is_zero() && now.saturating_duration_since(s.born) >= d
                        }) =>
                {
                    self.close(id)
                }
                _ => {}
            }
        }
        self.schedule(now);
    }
    /// pg drains checked-out clients; mysql2 closes all connections on end.
    pub fn end(&mut self) -> Result<(), Error> {
        if self.ending {
            return Err(Error::Closed);
        }
        self.ending = true;
        while let Some(r) = self.waiting.pop_front() {
            self.events.push_back(Event::CheckoutFailed {
                token: r.token,
                error: Error::Closed,
            });
        }
        for i in 0..self.slots.len() {
            let s = &self.slots[i];
            let id = ConnectionId {
                slot: i,
                generation: s.generation,
            };
            match s.state {
                State::Connecting(r) => {
                    self.events.push_back(Event::CheckoutFailed {
                        token: r.token,
                        error: Error::Closed,
                    });
                    self.close(id);
                }
                State::Idle(_) => self.close(id),
                State::Busy(_) if MYSQL => self.close(id),
                _ => {}
            }
        }
        self.maybe_ended();
        Ok(())
    }
    fn maybe_ended(&mut self) {
        if self.ending && !self.ended && self.total_count() == 0 {
            self.ended = true;
            self.events.push_back(Event::Ended);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fifo_timeouts_reuse_and_stale_release() {
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        let now = Instant::now();
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        let now = Instant::from_duration(std::time::Duration::ZERO);
        let mut p = Pool::new(Config {
            max: 1,
            min: 0,
            max_idle: 1,
            ..Config::default()
        })
        .unwrap();
        p.checkout(1, now, None).unwrap();
        let Some(Event::Connect(id)) = p.next_event() else {
            panic!()
        };
        p.connected(id, now).unwrap();
        assert_eq!(p.next_event(), Some(Event::Connected(id)));
        let Some(Event::Acquired(a)) = p.next_event() else {
            panic!()
        };
        p.checkout(2, now, None).unwrap();
        p.checkout(3, now, Some(now + Duration::from_secs(1)))
            .unwrap();
        assert_eq!(p.waiting_count(), 2);
        p.checkin(a, now, false).unwrap();
        assert_eq!(p.next_event(), Some(Event::Released(id)));
        let Some(Event::Acquired(b)) = p.next_event() else {
            panic!()
        };
        assert_eq!(b.token, 2);
        assert_eq!(p.checkin(a, now, false), Err(Error::InvalidLease));
        p.handle_timeout(now + Duration::from_secs(1));
        assert_eq!(
            p.next_event(),
            Some(Event::CheckoutFailed {
                token: 3,
                error: Error::Timeout
            })
        );
        p.checkin(b, now, false).unwrap();
        p.next_event();
        assert_eq!(p.idle_count(), 1);
        let deadline = p.next_timeout().unwrap();
        p.handle_timeout(deadline);
        assert_eq!(p.next_event(), Some(Event::Close(id)));
        p.closed(id, deadline).unwrap();
        assert_eq!(p.total_count(), 0);
        p.next_event();
        p.end().unwrap();
        assert_eq!(p.next_event(), Some(Event::Ended));
        assert!(p.next_event().is_none());
    }
    #[test]
    fn connect_timeout_and_late_success_do_not_acquire() {
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        let now = Instant::now();
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        let now = Instant::from_duration(std::time::Duration::ZERO);
        let mut p = Pool::new(Config::default()).unwrap();
        p.checkout(5, now, Some(now + Duration::from_secs(1)))
            .unwrap();
        let Some(Event::Connect(id)) = p.next_event() else {
            panic!()
        };
        p.handle_timeout(now + Duration::from_secs(1));
        assert_eq!(
            p.next_event(),
            Some(Event::CheckoutFailed {
                token: 5,
                error: Error::Timeout
            })
        );
        assert_eq!(p.next_event(), Some(Event::Close(id)));
        p.connected(id, now + Duration::from_secs(2)).unwrap();
        assert!(p.next_event().is_none());
        p.closed(id, now).unwrap();
        assert_eq!(p.next_event(), Some(Event::Removed(id)));
    }
    #[test]
    fn min_queue_limit_and_end_semantics() {
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        let now = Instant::now();
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        let now = Instant::from_duration(std::time::Duration::ZERO);
        let mut p = Pool::new(Config {
            max: 2,
            min: 1,
            queue_limit: Some(1),
            ..Config::default()
        })
        .unwrap();
        let mut leases = Vec::new();
        for token in [1, 2] {
            p.checkout(token, now, None).unwrap();
            let Some(Event::Connect(id)) = p.next_event() else {
                panic!()
            };
            p.connected(id, now).unwrap();
            p.next_event();
            let Some(Event::Acquired(lease)) = p.next_event() else {
                panic!()
            };
            leases.push(lease);
        }
        p.checkout(3, now, None).unwrap();
        assert_eq!(p.checkout(4, now, None), Err(Error::QueueLimit));
        p.checkin(leases[0], now, false).unwrap();
        p.next_event();
        let Some(Event::Acquired(lease)) = p.next_event() else {
            panic!()
        };
        assert_eq!(lease.token, 3);
        p.checkin(lease, now, false).unwrap();
        p.next_event();
        p.checkin(leases[1], now, false).unwrap();
        p.next_event();
        let deadline = p.next_timeout().unwrap();
        p.handle_timeout(deadline);
        let Some(Event::Close(id)) = p.next_event() else {
            panic!()
        };
        assert!(p.next_event().is_none());
        p.closed(id, deadline).unwrap();
        p.next_event();
        assert_eq!(p.idle_count(), 1);
        assert!(p.next_timeout().is_none());
        p.checkout(5, deadline, None).unwrap();
        let Some(Event::Acquired(lease)) = p.next_event() else {
            panic!()
        };
        p.end().unwrap();
        if !MYSQL {
            assert!(p.next_event().is_none());
            p.checkin(lease, deadline, false).unwrap();
            assert_eq!(p.next_event(), Some(Event::Released(lease.connection)));
        }
        assert_eq!(p.next_event(), Some(Event::Close(lease.connection)));
        p.closed(lease.connection, deadline).unwrap();
        p.next_event();
        assert_eq!(p.next_event(), Some(Event::Ended));
    }
}
