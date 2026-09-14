#![deny(unsafe_op_in_unsafe_fn)]
use js_sys::{Array, Function, Uint8Array};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
use wasm_bindgen::{JsCast, prelude::*};
const CAP: usize = 128;
#[wasm_bindgen(module = "/host.js")]
extern "C" {
    type Host;
    #[wasm_bindgen(js_name = makeHost)]
    fn make_host(post: &Function, schedule: &Function) -> Host;
    #[wasm_bindgen(method)]
    fn wake(this: &Host);
    #[wasm_bindgen(method, js_name = beginTurn)]
    fn begin_turn(this: &Host);
    #[wasm_bindgen(method, catch)]
    fn timer(this: &Host, id: u32, ms: f64) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch)]
    fn fetch(this: &Host, id: u32, url: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch)]
    fn websocket(this: &Host, id: u32, url: &str, bytes: &Uint8Array) -> Result<(), JsValue>;
    #[wasm_bindgen(method)]
    fn cancel(this: &Host, id: u32);
    #[wasm_bindgen(method)]
    fn release(this: &Host, id: u32);
    #[wasm_bindgen(method)]
    fn complete(this: &Host, id: u32, kind: u32, value: JsValue);
    #[wasm_bindgen(method, getter)]
    fn schedules(this: &Host) -> u32;
    #[wasm_bindgen(method, getter)]
    fn callbacks(this: &Host) -> u32;
    #[wasm_bindgen(method)]
    fn dispose(this: &Host);
}
struct Slot {
    id: u32,
    token: u64,
    terminal: bool,
    closed: bool,
    delivered: bool,
}
struct Completion {
    token: u64,
    kind: u32,
    value: JsValue,
}
struct State {
    slots: [Option<Slot>; CAP],
    queue: VecDeque<Completion>,
    generation: u32,
}
impl State {
    fn post(&mut self, id: u32, kind: u32, value: JsValue) -> bool {
        let Some(s) = self.slots[id as usize % CAP].as_mut() else {
            return false;
        };
        if s.id != id || s.terminal {
            return false;
        }
        s.terminal = true;
        self.queue.push_back(Completion {
            token: s.token,
            kind,
            value,
        });
        true
    }
    fn reserve(&mut self, token: u64) -> Result<u32, JsValue> {
        let index = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or_else(|| JsValue::from_str("Capacity"))?;
        self.generation = self
            .generation
            .checked_add(1)
            .filter(|g| *g < u32::MAX / CAP as u32)
            .ok_or_else(|| JsValue::from_str("Generation exhausted"))?;
        let id = self.generation * CAP as u32 + index as u32;
        self.slots[index] = Some(Slot {
            id,
            token,
            terminal: false,
            closed: false,
            delivered: false,
        });
        Ok(id)
    }
}
#[wasm_bindgen]
pub struct WebLoop {
    host: Host,
    state: Rc<RefCell<State>>,
    _post: Closure<dyn FnMut(u32, u32, JsValue) -> bool>,
}
#[wasm_bindgen]
impl WebLoop {
    #[wasm_bindgen(constructor)]
    pub fn new(schedule_turn: Function) -> Self {
        let state = Rc::new(RefCell::new(State {
            slots: std::array::from_fn(|_| None),
            queue: VecDeque::with_capacity(CAP * 2),
            generation: 0,
        }));
        let weak = Rc::downgrade(&state);
        let post = Closure::wrap(Box::new(move |id, kind, value| {
            weak.upgrade()
                .is_some_and(|s| s.borrow_mut().post(id, kind, value))
        }) as Box<dyn FnMut(u32, u32, JsValue) -> bool>);
        let host = make_host(post.as_ref().unchecked_ref(), &schedule_turn);
        Self {
            host,
            state,
            _post: post,
        }
    }
    pub fn integration(&self) -> String {
        "HostCallback".into()
    }
    fn finish_submit(&self, id: u32, result: Result<(), JsValue>) -> Result<u32, JsValue> {
        if let Err(error) = result {
            self.host.release(id);
            self.state.borrow_mut().slots[id as usize % CAP] = None;
            return Err(error);
        }
        Ok(id)
    }
    pub fn timer(&self, ms: f64, token: u64) -> Result<u32, JsValue> {
        if !ms.is_finite() || !(0.0..=2_147_483_647.0).contains(&ms) {
            return Err(JsValue::from_str("Invalid timeout"));
        }
        let id = self.state.borrow_mut().reserve(token)?;
        self.finish_submit(id, self.host.timer(id, ms))
    }
    pub fn fetch(&self, url: &str, token: u64) -> Result<u32, JsValue> {
        let id = self.state.borrow_mut().reserve(token)?;
        self.finish_submit(id, self.host.fetch(id, url))
    }
    /// One WebSocket exchange, sufficient to test host transport, ownership and cancellation.
    pub fn websocket(&self, url: &str, bytes: &Uint8Array, token: u64) -> Result<u32, JsValue> {
        let id = self.state.borrow_mut().reserve(token)?;
        self.finish_submit(id, self.host.websocket(id, url, bytes))
    }
    pub fn cancel(&self, id: u32) -> bool {
        if !self.state.borrow_mut().post(id, 5, JsValue::UNDEFINED) {
            return false;
        }
        self.host.cancel(id);
        self.host.wake();
        true
    }
    pub fn close(&self, id: u32, token: u64) -> bool {
        {
            let state = self.state.borrow();
            if !state.slots[id as usize % CAP]
                .as_ref()
                .is_some_and(|s| s.id == id && !s.closed && !s.delivered)
            {
                return false;
            }
        }
        self.cancel(id);
        let mut state = self.state.borrow_mut();
        if let Some(s) = &mut state.slots[id as usize % CAP] {
            s.closed = true;
        }
        state.queue.push_back(Completion {
            token,
            kind: 6,
            value: JsValue::UNDEFINED,
        });
        drop(state);
        self.host.wake();
        true
    }
    /// Core queue draining is allocation-free after initialization. This JS-facing
    /// test facade allocates result arrays; it is not the proposed core Backend ABI.
    pub fn turn(&self, timeout: i32) -> Result<Array, JsValue> {
        if timeout != 0 {
            return Err(JsValue::from_str("Unsupported: web turn accepts Now only"));
        }
        self.host.begin_turn();
        let mut state = self.state.borrow_mut();
        for slot in &mut state.slots {
            if slot.as_ref().is_some_and(|s| s.delivered)
                && let Some(s) = slot.take()
            {
                self.host.release(s.id);
            }
        }
        let out = Array::new();
        while let Some(c) = state.queue.pop_front() {
            let record = Array::new();
            record.push(&js_sys::BigInt::from(c.token));
            record.push(&JsValue::from(c.kind));
            record.push(&c.value);
            out.push(&record);
        }
        for s in state.slots.iter_mut().flatten() {
            if s.terminal {
                s.delivered = true;
            }
        }
        Ok(out)
    }
    pub fn alive(&self) -> bool {
        self.state
            .borrow()
            .slots
            .iter()
            .flatten()
            .any(|s| !s.terminal)
    }
    pub fn callback_count(&self) -> u32 {
        self.host.callbacks()
    }
    pub fn schedule_count(&self) -> u32 {
        self.host.schedules()
    }
    /// Fault-injection entry point for stale and duplicate callback tests.
    pub fn inject(&self, id: u32, kind: u32) {
        self.host.complete(id, kind, JsValue::UNDEFINED);
    }
}
impl Drop for WebLoop {
    fn drop(&mut self) {
        self.host.dispose();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use wasm_bindgen_futures::JsFuture;
    use wasm_bindgen_test::*;
    #[cfg(feature = "browser")]
    wasm_bindgen_test_configure!(run_in_browser);
    #[wasm_bindgen(module = "/tests/contract.js")]
    extern "C" {
        #[wasm_bindgen(js_name = runSuite)]
        fn run_suite(a: JsValue, b: JsValue) -> js_sys::Promise;
    }
    #[wasm_bindgen_test]
    async fn host_callback_contract() {
        let calls = Rc::new(Cell::new(0));
        let observed = calls.clone();
        let schedule =
            Closure::wrap(Box::new(move || observed.set(observed.get() + 1)) as Box<dyn FnMut()>);
        let a = WebLoop::new(schedule.as_ref().unchecked_ref::<Function>().clone());
        let b = WebLoop::new(schedule.as_ref().unchecked_ref::<Function>().clone());
        let result = JsFuture::from(run_suite(a.into(), b.into()))
            .await
            .expect("JS contract assertions");
        assert_eq!(result.as_f64(), Some(10.0));
        assert!(calls.get() > 0);
    }
}
