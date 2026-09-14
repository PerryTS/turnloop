//! connection-monitoring-and-pooling/connection-monitoring-and-pooling.md
//! §§ Connection Pool, Connection Pool Clearing, Wait Queue, Connection Checkout.
//! Host performs Connect/Close actions and reports authentication-ready connections.
use crate::{Error, ErrorKind, Result};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease {
    pub id: u64,
    pub generation: u64,
}
#[derive(Debug)]
pub enum PoolEvent {
    Connect(Lease),
    Close(Lease),
    CheckedOut { token: u64, connection: Lease },
    CheckoutFailed { token: u64, error: Error },
    Cleared { generation: u64 },
    Closed,
}
#[derive(Clone, Debug)]
pub struct PoolOptions {
    pub min_size: usize,
    pub max_size: usize,
    pub max_connecting: usize,
    pub wait_timeout: Duration,
    pub max_idle: Duration,
}
impl Default for PoolOptions {
    fn default() -> Self {
        Self {
            min_size: 0,
            max_size: 100,
            max_connecting: 2,
            wait_timeout: Duration::ZERO,
            max_idle: Duration::ZERO,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Connecting,
    Idle,
    Out,
}
struct Entry {
    lease: Lease,
    status: Status,
    last_used: Instant,
}
struct Waiter {
    token: u64,
    deadline: Option<Instant>,
}
pub struct Pool {
    options: PoolOptions,
    entries: Vec<Entry>,
    waiters: VecDeque<Waiter>,
    events: VecDeque<PoolEvent>,
    generation: u64,
    next_id: u64,
    ready: bool,
    closed: bool,
}
impl Pool {
    pub fn new(options: PoolOptions) -> Result<Self> {
        if options.max_connecting == 0
            || (options.max_size != 0 && options.min_size > options.max_size)
        {
            return Err(Error::new(ErrorKind::InvalidArgument, "Invalid pool size"));
        }
        let reserve = if options.max_size == 0 {
            100
        } else {
            options.max_size
        };
        Ok(Self {
            options,
            entries: Vec::with_capacity(reserve),
            waiters: VecDeque::with_capacity(reserve),
            events: VecDeque::with_capacity(reserve * 3 + 2),
            generation: 0,
            next_id: 0,
            ready: false,
            closed: false,
        })
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn total(&self) -> usize {
        self.entries.len()
    }
    pub fn checked_out(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status == Status::Out)
            .count()
    }
    pub fn ready(&mut self, now: Instant) {
        if !self.closed {
            self.ready = true;
            self.drive(now);
        }
    }
    pub fn checkout(&mut self, token: u64, now: Instant) -> Result<()> {
        if self.waiters.iter().any(|w| w.token == token) {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "Duplicate checkout token",
            ));
        }
        if self.closed || !self.ready {
            self.events.push_back(PoolEvent::CheckoutFailed {
                token,
                error: Error::new(
                    if self.closed {
                        ErrorKind::PoolClosed
                    } else {
                        ErrorKind::PoolCleared
                    },
                    "Connection pool is not ready",
                ),
            });
            return Ok(());
        }
        self.waiters.push_back(Waiter {
            token,
            deadline: if self.options.wait_timeout.is_zero() {
                None
            } else {
                Some(now + self.options.wait_timeout)
            },
        });
        self.drive(now);
        Ok(())
    }
    pub fn connected(&mut self, lease: Lease, now: Instant) -> Result<()> {
        let Some(e) = self
            .entries
            .iter_mut()
            .find(|e| e.lease == lease && e.status == Status::Connecting)
        else {
            return Err(Error::protocol("Unknown connecting pool connection"));
        };
        e.status = Status::Idle;
        e.last_used = now;
        self.drive(now);
        Ok(())
    }
    pub fn connect_failed(&mut self, lease: Lease, error: Error, now: Instant) -> Result<()> {
        let at = self
            .entries
            .iter()
            .position(|e| e.lease == lease && e.status == Status::Connecting)
            .ok_or_else(|| Error::protocol("Unknown connecting pool connection"))?;
        self.entries.swap_remove(at);
        self.events.push_back(PoolEvent::Close(lease));
        if let Some(w) = self.waiters.pop_front() {
            self.events.push_back(PoolEvent::CheckoutFailed {
                token: w.token,
                error,
            });
        }
        self.clear();
        let _ = now;
        Ok(())
    }
    pub fn checkin(&mut self, lease: Lease, now: Instant) -> Result<()> {
        let at = self
            .entries
            .iter()
            .position(|e| e.lease == lease && e.status == Status::Out)
            .ok_or_else(|| Error::protocol("Unknown or already checked-in connection"))?;
        if self.closed || lease.generation != self.generation {
            self.entries.swap_remove(at);
            self.events.push_back(PoolEvent::Close(lease));
        } else {
            self.entries[at].status = Status::Idle;
            self.entries[at].last_used = now;
        }
        self.drive(now);
        Ok(())
    }
    pub fn clear(&mut self) {
        if self.closed {
            return;
        }
        self.generation += 1;
        self.ready = false;
        self.events.push_back(PoolEvent::Cleared {
            generation: self.generation,
        });
        let mut i = 0;
        while i < self.entries.len() {
            if self.entries[i].status != Status::Out {
                let e = self.entries.swap_remove(i);
                self.events.push_back(PoolEvent::Close(e.lease));
            } else {
                i += 1;
            }
        }
        while let Some(w) = self.waiters.pop_front() {
            self.events.push_back(PoolEvent::CheckoutFailed {
                token: w.token,
                error: Error::new(ErrorKind::PoolCleared, "Connection pool was cleared"),
            });
        }
    }
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.clear();
        self.closed = true;
        self.events.push_back(PoolEvent::Closed);
    }
    pub fn cancel_checkout(&mut self, token: u64) -> bool {
        if let Some(i) = self.waiters.iter().position(|w| w.token == token) {
            self.waiters.remove(i);
            self.events.push_back(PoolEvent::CheckoutFailed {
                token,
                error: Error::new(ErrorKind::Cancelled, "Connection checkout cancelled"),
            });
            true
        } else {
            false
        }
    }
    pub fn next_timeout(&self) -> Option<Instant> {
        self.waiters
            .iter()
            .filter_map(|w| w.deadline)
            .chain(
                self.entries
                    .iter()
                    .filter(|e| e.status == Status::Idle && !self.options.max_idle.is_zero())
                    .map(|e| e.last_used + self.options.max_idle),
            )
            .min()
    }
    pub fn handle_timeout(&mut self, now: Instant) {
        let mut i = 0;
        while i < self.waiters.len() {
            if self.waiters[i].deadline.is_some_and(|d| d <= now) {
                let w = self.waiters.remove(i).unwrap();
                self.events.push_back(PoolEvent::CheckoutFailed {
                    token: w.token,
                    error: Error::new(
                        ErrorKind::Timeout,
                        "Timed out while checking out a connection from connection pool",
                    ),
                });
            } else {
                i += 1;
            }
        }
        self.drive(now);
    }
    pub fn poll_event(&mut self) -> Option<PoolEvent> {
        self.events.pop_front()
    }
    fn drive(&mut self, now: Instant) {
        if !self.ready || self.closed {
            return;
        }
        let mut i = 0;
        while i < self.entries.len() {
            let e = &self.entries[i];
            if e.status == Status::Idle
                && !self.options.max_idle.is_zero()
                && now >= e.last_used + self.options.max_idle
            {
                let e = self.entries.swap_remove(i);
                self.events.push_back(PoolEvent::Close(e.lease));
            } else {
                i += 1;
            }
        }
        while !self.waiters.is_empty() {
            let Some(at) = self.entries.iter().rposition(|e| e.status == Status::Idle) else {
                break;
            };
            let w = self.waiters.pop_front().unwrap();
            let e = &mut self.entries[at];
            e.status = Status::Out;
            self.events.push_back(PoolEvent::CheckedOut {
                token: w.token,
                connection: e.lease,
            });
        }
        let mut connecting = self
            .entries
            .iter()
            .filter(|e| e.status == Status::Connecting)
            .count();
        while (self.entries.len() < self.options.min_size || connecting < self.waiters.len())
            && connecting < self.options.max_connecting
            && (self.options.max_size == 0 || self.entries.len() < self.options.max_size)
        {
            self.next_id += 1;
            let lease = Lease {
                id: self.next_id,
                generation: self.generation,
            };
            self.entries.push(Entry {
                lease,
                status: Status::Connecting,
                last_used: now,
            });
            self.events.push_back(PoolEvent::Connect(lease));
            connecting += 1;
        }
    }
}
