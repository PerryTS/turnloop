//! Browser timestamps must be supplied by the host. std::Instant cannot be
//! constructed there without the unsupported std clock. This matches the core
//! lane's duration-based web Instant and requires no browser imports or I/O.
use std::{ops::Add, time::Duration};
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Instant(Duration);
impl Instant {
    pub const fn from_duration(ticks: Duration) -> Self {
        Self(ticks)
    }
    pub const fn as_duration(self) -> Duration {
        self.0
    }
    pub fn checked_add(self, duration: Duration) -> Option<Self> {
        self.0.checked_add(duration).map(Self)
    }
    pub fn saturating_duration_since(self, earlier: Self) -> Duration {
        self.0.saturating_sub(earlier.0)
    }
}
impl Add<Duration> for Instant {
    type Output = Self;
    fn add(self, duration: Duration) -> Self {
        self.checked_add(duration).expect("instant overflow")
    }
}
