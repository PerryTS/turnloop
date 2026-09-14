//! Rust side of HostCallback: bounded completion inbox, no scheduling callbacks in turn.
//! Wire host.js's generation-checked posts here after core's OpId/Completion exist.
use crate::{DraftBackend, Integration, Timeout, TurnInfo};
const CAP: usize = 256;
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    UnsupportedTimeout,
    OutputCapacity,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Post {
    ScheduleTurn,
    Coalesced,
}
pub struct Web<C> {
    queue: [Option<C>; CAP],
    head: usize,
    len: usize,
    scheduled: bool,
}
impl<C> Default for Web<C> {
    fn default() -> Self {
        Self {
            queue: std::array::from_fn(|_| None),
            head: 0,
            len: 0,
            scheduled: false,
        }
    }
}
impl<C> Web<C> {
    /// Caller retains an unaccepted completion on full; never silently discard it.
    /// ScheduleTurn means the host should queue its callback after this call returns.
    pub fn post(&mut self, c: C) -> Result<Post, C> {
        if self.len == CAP {
            return Err(c);
        }
        self.queue[(self.head + self.len) % CAP] = Some(c);
        self.len += 1;
        if self.scheduled {
            Ok(Post::Coalesced)
        } else {
            self.scheduled = true;
            Ok(Post::ScheduleTurn)
        }
    }
}
impl<C> DraftBackend for Web<C> {
    type Completion = C;
    type Error = Error;
    fn integration(&self) -> Integration {
        Integration::HostCallback
    }
    fn turn(&mut self, timeout: Timeout, out: &mut Vec<C>) -> Result<TurnInfo, Error> {
        if !matches!(timeout, Timeout::Now) {
            return Err(Error::UnsupportedTimeout);
        }
        if out.capacity() < self.len {
            return Err(Error::OutputCapacity);
        }
        out.clear();
        let count = self.len;
        while self.len > 0 {
            if let Some(c) = self.queue[self.head].take() {
                out.push(c);
            }
            self.head = (self.head + 1) % CAP;
            self.len -= 1;
        }
        self.scheduled = false;
        Ok(TurnInfo {
            completions: count,
            waited: false,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capacity_rejection_preserves_completions_and_schedule_request() {
        let mut driver = Web::default();
        for token in 0..CAP {
            assert_eq!(
                driver.post(token),
                Ok(if token == 0 {
                    Post::ScheduleTurn
                } else {
                    Post::Coalesced
                })
            );
        }
        assert_eq!(driver.post(CAP), Err(CAP));
        let mut too_small = Vec::with_capacity(CAP - 1);
        assert_eq!(
            driver.turn(Timeout::Now, &mut too_small),
            Err(Error::OutputCapacity)
        );
        assert_eq!(
            driver.turn(Timeout::Forever, &mut too_small),
            Err(Error::UnsupportedTimeout)
        );
        let mut out = Vec::with_capacity(CAP);
        let info = driver
            .turn(Timeout::Now, &mut out)
            .expect("enough capacity");
        assert_eq!(info.completions, CAP);
        assert!(!info.waited);
        assert!(out.iter().copied().eq(0..CAP));
        assert_eq!(driver.post(999), Ok(Post::ScheduleTurn));
    }
}
