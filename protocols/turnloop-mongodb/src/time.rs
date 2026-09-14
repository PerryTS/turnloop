//! Native/WASI use std::time::Instant, supplied by the host. Browser Wasm uses
//! explicit monotonic ticks because std::time::Instant cannot be constructed there.
//! Mirrors the core lane's host-clock representation without depending on the loop.
use std::{
    ops::{Add, Sub},
    time::Duration,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostInstant(Duration);
impl HostInstant {
    pub const fn from_duration(ticks: Duration) -> Self {
        Self(ticks)
    }
    pub const fn as_duration(self) -> Duration {
        self.0
    }
    pub fn checked_add(self, d: Duration) -> Option<Self> {
        self.0.checked_add(d).map(Self)
    }
    pub fn checked_sub(self, d: Duration) -> Option<Self> {
        self.0.checked_sub(d).map(Self)
    }
    pub fn saturating_duration_since(self, earlier: Self) -> Duration {
        self.0.saturating_sub(earlier.0)
    }
    pub fn duration_since(self, earlier: Self) -> Duration {
        self.saturating_duration_since(earlier)
    }
}
impl Add<Duration> for HostInstant {
    type Output = Self;
    fn add(self, d: Duration) -> Self {
        self.checked_add(d).expect("Host instant overflow")
    }
}
impl Sub<Duration> for HostInstant {
    type Output = Self;
    fn sub(self, d: Duration) -> Self {
        self.checked_sub(d).expect("Host instant underflow")
    }
}
impl Sub for HostInstant {
    type Output = Duration;
    fn sub(self, earlier: Self) -> Duration {
        self.duration_since(earlier)
    }
}
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub use HostInstant as Instant;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use std::time::Instant;
