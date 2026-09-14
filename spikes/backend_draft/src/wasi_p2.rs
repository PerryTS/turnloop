//! Executable adapter to the p2 experiment; inherits its documented allocation failure.
use crate::{DraftBackend, Integration, Timeout, TurnInfo};
use std::{
    ops::{Deref, DerefMut},
    time::Instant,
};
use turnloop_wasi_p2_spike::{Completion, Driver, Timeout as PollTimeout};
#[derive(Default)]
pub struct WasiP2 {
    driver: Driver,
}
impl Deref for WasiP2 {
    type Target = Driver;
    fn deref(&self) -> &Driver {
        &self.driver
    }
}
impl DerefMut for WasiP2 {
    fn deref_mut(&mut self) -> &mut Driver {
        &mut self.driver
    }
}
impl DraftBackend for WasiP2 {
    type Completion = Completion;
    type Error = wasi::sockets::network::ErrorCode;
    fn integration(&self) -> Integration {
        Integration::RuntimeOwned
    }
    fn turn(
        &mut self,
        timeout: Timeout,
        out: &mut Vec<Completion>,
    ) -> Result<TurnInfo, Self::Error> {
        if out.capacity() < turnloop_wasi_p2_spike::CAPACITY * 2 {
            return Err(wasi::sockets::network::ErrorCode::OutOfMemory);
        }
        let timeout = match timeout {
            Timeout::Now => PollTimeout::Now,
            Timeout::Forever => PollTimeout::Forever,
            Timeout::After(d) => PollTimeout::After(d.as_nanos().min(u64::MAX as u128) as u64),
            Timeout::Until(at) => PollTimeout::After(
                at.saturating_duration_since(Instant::now())
                    .as_nanos()
                    .min(u64::MAX as u128) as u64,
            ),
        };
        let before = self.driver.waits;
        self.driver.turn(timeout, out)?;
        Ok(TurnInfo {
            completions: out.len(),
            waited: self.driver.waits != before,
        })
    }
}
