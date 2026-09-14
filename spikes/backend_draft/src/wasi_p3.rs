//! Proposed p3 boundary. No provider is shipped: wit-bindgen's set is private.
//! This typechecks the adapter shape without claiming that block_on is one wait.
use crate::{DraftBackend, Integration, Timeout, TurnInfo};
/// A future dedicated ABI lowering/runtime step API must supply this interface.
/// It owns resource state and stable buffers; methods advance only driver operations.
pub trait WaitSet {
    type Event;
    type Completion;
    type Error;
    /// Arms/replaces a deadline waitable, then makes exactly one ABI poll or wait.
    /// Returns after that event even if it is only an intermediate subtask event.
    fn step(&mut self, timeout: Timeout) -> Result<(Option<Self::Event>, bool), Self::Error>;
    fn finish(&mut self, event: Self::Event) -> Result<Option<Self::Completion>, Self::Error>;
}
#[derive(Debug)]
pub enum Error<E> {
    Backend(E),
    OutputCapacity,
}
pub struct WasiP3<W> {
    pub wait_set: W,
}
impl<W: WaitSet> DraftBackend for WasiP3<W> {
    type Completion = W::Completion;
    type Error = Error<W::Error>;
    fn integration(&self) -> Integration {
        Integration::RuntimeOwned
    }
    fn turn(
        &mut self,
        timeout: Timeout,
        out: &mut Vec<W::Completion>,
    ) -> Result<TurnInfo, Self::Error> {
        if out.capacity() == 0 {
            return Err(Error::OutputCapacity);
        }
        out.clear();
        let (event, waited) = self.wait_set.step(timeout).map_err(Error::Backend)?;
        let before = out.len();
        if let Some(event) = event
            && let Some(c) = self.wait_set.finish(event).map_err(Error::Backend)?
        {
            out.push(c);
        }
        Ok(TurnInfo {
            completions: out.len() - before,
            waited,
        })
    }
}
