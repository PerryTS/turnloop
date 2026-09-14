//! MongoDB client with timer-driven SDAM, CMAP ownership and operation retries.
use super::connection::{instant, time};
use super::{Connection, Pool};
use crate::{
    operation::{
        Operation, OperationAction, OperationKind, OperationOptions, RetrySession,
        ServerCapabilities,
    },
    topology::{
        ApplicationError, ApplicationErrorKind, ServerType, Topology, TopologyEvent, TopologyType,
    },
    uri::{Address, Options, ReadPreference},
};
use bson::{
    Document, doc,
    raw::{RawDocument, RawDocumentBuf},
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    future::{Future, poll_fn},
    io,
    pin::Pin,
    rc::Rc,
    task::{Poll, Waker},
    time::Duration,
};
use turnloop_io::turnloop::{JoinHandle, Sleep};
use turnloop_io::{Backend, ExecutorHandle, Instant, deadline};
use turnloop_tls::asynchronous::ClientTls;
#[derive(Clone)]
pub struct ConnectOptions {
    pub protocol: Options,
    pub tls: Option<ClientTls>,
}
struct State<B: Backend + 'static> {
    topology: Topology,
    pools: BTreeMap<String, Pool<B>>,
    changed: Option<Waker>,
    waiters: Vec<Waker>,
    heartbeats: u64,
    failure: Option<String>,
    cleanup: Vec<Pin<Box<dyn Future<Output = ()>>>>,
}
struct Owner<B: Backend + 'static> {
    state: Rc<RefCell<State<B>>>,
    executor: ExecutorHandle<B>,
    options: ConnectOptions,
    _monitor: JoinHandle<()>,
}
pub struct Client<B: Backend + 'static> {
    owner: Rc<Owner<B>>,
    operation: Operation,
    session: [u8; 16],
    txn: i64,
    selected: String,
}
type Probe<B> = Pin<Box<dyn Future<Output = (Option<Connection<B>>, io::Result<Document>)>>>;
struct Monitor<B: Backend> {
    address: String,
    connection: Option<Connection<B>>,
    pending: Option<Probe<B>>,
    started: Instant,
}
fn wake<B: Backend + 'static>(state: &State<B>) {
    for w in &state.waiters {
        w.wake_by_ref();
    }
}
fn network_failure<B: Backend + 'static>(state: &mut State<B>, address: &str, now: Instant) {
    if let Some(s) = state.topology.servers.get(address) {
        let error = ApplicationError {
            generation: s.generation,
            max_wire_version: s.max_wire_version,
            handshake_complete: true,
            kind: ApplicationErrorKind::Network,
            response: None,
        };
        state.topology.application_error(address, error, time(now));
    }
}
impl<B: Backend + 'static> Client<B> {
    pub async fn connect(
        executor: &ExecutorHandle<B>,
        options: ConnectOptions,
        at: Instant,
    ) -> io::Result<Self> {
        let mut options = options;
        if let Some(name) = options.protocol.srv.clone() {
            use turnloop_io::dns::{query, Query, Record};
            let srv = query(executor, format!("_mongodb._tcp.{name}"), Query::Srv, at).await?;
            let txt = query(executor, name, Query::Txt, at).await?;
            let records: Vec<_> = srv.into_iter().filter_map(|r| match r {
                Record::Srv {target,port,..} => Some(Address {host:target,port}), _ => None,
            }).collect();
            let txt: Vec<_> = txt.into_iter().filter_map(|r| match r {
                Record::Txt {text,..} => Some(text), _ => None,
            }).collect();
            options.protocol.resolve(&records,&txt).map_err(io::Error::other)?;
        }
        let state = Rc::new(RefCell::new(State {
            topology: Topology::new(&options.protocol, time(executor.now())),
            pools: BTreeMap::new(),
            changed: None,
            waiters: Vec::new(),
            heartbeats: 0,
            failure: None,
            cleanup: Vec::new(),
        }));
        let shared = state.clone();
        let exec = executor.clone();
        let config = options.clone();
        let mut monitors: Vec<Monitor<B>> = Vec::new();
        let mut timer: Option<(Instant, Sleep<B>)> = None;
        let monitor = executor
            .spawn_local(poll_fn(move |cx| {
                let mut state = shared.borrow_mut();
                state.changed = Some(cx.waker().clone());
                let now = exec.now();
                state.cleanup.retain_mut(|future| future.as_mut().poll(cx).is_pending());
                // Each probe owns one connection future. No waiting is done inside
                // the driver turn and all servers progress independently.
                for m in &mut monitors {
                    if let Some(future) = &mut m.pending
                        && let Poll::Ready((connection, result)) = future.as_mut().poll(cx) {
                            m.pending = None;
                            m.connection = connection;
                            state.heartbeats += 1;
                            match result {
                                Ok(hello) => {
                                    state.topology.update(
                                        &m.address,
                                        &hello,
                                        time(now),
                                        now.saturating_duration_since(m.started),
                                    );
                                    if state
                                        .topology
                                        .servers
                                        .get(&m.address)
                                        .is_some_and(|s| s.kind.readable())
                                    {
                                        if let Some(pool) = state.pools.get(&m.address) {
                                            pool.update_policy(|p| p.ready(time(now)));
                                        } else {
                                            let result = Address::parse(&m.address)
                                                .map_err(io::Error::other)
                                                .and_then(|address| {
                                                    super::pool::create(&exec, address, &config)
                                                });
                                            match result {
                                                Ok(pool) => {
                                                    state.pools.insert(m.address.clone(), pool);
                                                }
                                                Err(e) => state.failure = Some(e.to_string()),
                                            }
                                        }
                                    }
                                }
                                Err(_) => network_failure(&mut state, &m.address, now),
                            }
                            wake(&state);
                        }
                }
                state.topology.handle_timeout(time(now));
                while let Some(event) = state.topology.poll_event() {
                    match event {
                        TopologyEvent::Check { address, .. } => {
                            let index = if let Some(i) =
                                monitors.iter().position(|m| m.address == address)
                            {
                                i
                            } else {
                                monitors.push(Monitor {
                                    address: address.clone(),
                                    connection: None,
                                    pending: None,
                                    started: now,
                                });
                                monitors.len() - 1
                            };
                            if monitors[index].pending.is_none() {
                                let old = monitors[index].connection.take();
                                let exec = exec.clone();
                                let options = config.clone();
                                monitors[index].started = now;
                                monitors[index].pending = Some(Box::pin(async move {
                                    let result = async {
                                        let address =
                                            Address::parse(&address).map_err(io::Error::other)?;
                                        let timeout = if options.protocol.connect_timeout.is_zero()
                                        {
                                            Duration::from_secs(30)
                                        } else {
                                            options.protocol.connect_timeout
                                        };
                                        let at = exec.now() + timeout;
                                        let mut connection = match old {
                                            Some(c) if c.is_reusable() => c,
                                            _ => {
                                                let socket = turnloop_io::resolve(
                                                    &exec,
                                                    &address.host,
                                                    address.port,
                                                    at,
                                                )
                                                .await?;
                                                let mut protocol = options.protocol.clone();
                                                protocol.credential = None;
                                                let c = Connection::connect(
                                                    &exec,
                                                    socket,
                                                    protocol,
                                                    options.tls.as_ref(),
                                                    at,
                                                )
                                                .await?;
                                                let hello =
                                                    c.hello().cloned().ok_or_else(|| {
                                                        io::Error::other("missing hello")
                                                    })?;
                                                return Ok((c, hello));
                                            }
                                        };
                                        let command = RawDocumentBuf::try_from(
                                            &doc! {"hello":1,"$db":"admin"},
                                        )
                                        .map_err(io::Error::other)?;
                                        let mut hello = None;
                                        connection
                                            .command(&command, &[], at, |reply| {
                                                hello = Some(
                                                    reply.try_into().map_err(io::Error::other)?,
                                                );
                                                Ok(())
                                            })
                                            .await?;
                                        Ok::<_, io::Error>((
                                            connection,
                                            hello.ok_or_else(|| {
                                                io::Error::other("missing heartbeat reply")
                                            })?,
                                        ))
                                    }
                                    .await;
                                    match result {
                                        Ok((connection, hello)) => (Some(connection), Ok(hello)),
                                        Err(e) => (None, Err(e)),
                                    }
                                }));
                                // Poll newly created work in the next executor pass.
                                cx.waker().wake_by_ref();
                            }
                        }
                        TopologyEvent::ClearPool { address, .. } => {
                            if let Some(pool) = state.pools.get(&address) {
                                pool.update_policy(|p| p.clear());
                            }
                        }
                        TopologyEvent::ServerRemoved(address) => {
                            state.pools.remove(&address);
                            monitors.retain(|m| m.address != address);
                        }
                        _ => {}
                    }
                }
                let at = state.topology.next_timeout().map(instant);
                if timer.as_ref().map(|(at, _)| *at) != at {
                    timer = at.map(|at| (at, exec.sleep_until(at)));
                }
                if let Some((_, sleep)) = &mut timer
                    && let Poll::Ready(result) = Pin::new(sleep).poll(cx) {
                        if let Err(e) = result {
                            state.failure = Some(e.to_string());
                            wake(&state);
                            return Poll::Ready(());
                        }
                        timer = None;
                        cx.waker().wake_by_ref();
                    }
                Poll::Pending
            }))
            .map_err(turnloop_io::error)?;
        let mut session = [0; 16];
        turnloop_tls::rustls::crypto::ring::default_provider()
            .secure_random
            .fill(&mut session)
            .map_err(|_| io::Error::other("secure entropy unavailable"))?;
        let owner = Rc::new(Owner {
            state,
            executor: executor.clone(),
            options,
            _monitor: monitor,
        });
        let this = Self {
            owner,
            operation: Operation::new(),
            session,
            txn: 0,
            selected: String::with_capacity(256),
        };
        let mut address = String::new();
        select(&this.owner, ReadPreference::Primary, None, at, &mut address).await?;
        Ok(this)
    }
    pub fn heartbeat_count(&self) -> u64 {
        self.owner.state.borrow().heartbeats
    }
    pub fn server_count(&self) -> usize {
        self.owner.state.borrow().topology.servers.len()
    }
    /// Retry classification comes from the sans-I/O Operation coordinator. A
    /// retryable write keeps the same session/transaction identity on both sends.
    pub async fn command(
        &mut self,
        body: &RawDocument,
        sequences: &[(&str, &[&RawDocument])],
        kind: OperationKind,
        at: Instant,
        mut receive: impl FnMut(&RawDocument) -> io::Result<()>,
    ) -> io::Result<()> {
        self.txn = self
            .txn
            .checked_add(1)
            .ok_or_else(|| io::Error::other("transaction number exhausted"))?;
        let options = &self.owner.options.protocol;
        self.operation
            .begin(
                body,
                sequences,
                OperationOptions {
                    token: 1,
                    kind,
                    retry: match kind {
                        OperationKind::Read => options.retry_reads,
                        OperationKind::Write => options.retry_writes,
                        OperationKind::RunCommand => false,
                    },
                    timeout: Some(at.saturating_duration_since(self.owner.executor.now())),
                    session: Some(RetrySession {
                        id: self.session,
                        txn_number: self.txn,
                    }),
                    read_preference: options.read_preference,
                },
                time(self.owner.executor.now()),
            )
            .map_err(io::Error::other)?;
        struct Cancel<'a>(&'a mut Operation);
        impl Drop for Cancel<'_> { fn drop(&mut self) { self.0.cancel(); } }
        let cancel = Cancel(&mut self.operation);
        run(
            &self.owner,
            cancel.0,
            at,
            &mut self.selected,
            &mut receive,
        )
        .await
    }
    /// Own batches while exposing one document at a time. getMore and killCursors
    /// retain the selected server; neither operation is retried on another server.
    pub async fn cursor(&mut self, body: &RawDocument, at: Instant) -> io::Result<Cursor<B>> {
        let mut address = String::new();
        select(
            &self.owner,
            self.owner.options.protocol.read_preference,
            None,
            at,
            &mut address,
        )
        .await?;
        let pool = self
            .owner
            .state
            .borrow()
            .pools
            .get(&address)
            .cloned()
            .ok_or_else(|| io::Error::other("selected pool disappeared"))?;
        let mut connection = pool.acquire(at).await?;
        let mut reply = None;
        connection
            .command(body, &[], at, |r| {
                reply = Some(r.to_owned());
                Ok(())
            })
            .await?;
        let reply = reply.ok_or_else(|| io::Error::other("missing cursor reply"))?;
        let batch = crate::command::CursorBatch::parse(&reply).map_err(io::Error::other)?;
        let mut core = crate::command::Cursor::new(batch.namespace, address, None, None)
            .map_err(io::Error::other)?;
        core.accept(&batch).map_err(io::Error::other)?;
        let rows = batch
            .rows()
            .map(|r| r.map(RawDocument::to_owned))
            .collect::<crate::Result<VecDeque<_>>>()
            .map_err(io::Error::other)?;
        Ok(Cursor {
            owner: self.owner.clone(),
            core,
            rows,
            command: crate::command::Command::new(),
            closed: false,
        })
    }
}
async fn select<B: Backend + 'static>(
    owner: &Owner<B>,
    preference: ReadPreference,
    excluded: Option<&str>,
    at: Instant,
    address: &mut String,
) -> io::Result<ServerCapabilities> {
    deadline(
        &owner.executor,
        at,
        poll_fn(|cx| {
            let mut state = owner.state.borrow_mut();
            if let Some(e) = &state.failure {
                return Poll::Ready(Err(io::Error::other(e.clone())));
            }
            let options = &owner.options.protocol;
            // The common primary/direct case needs no temporary candidate vector.
            let fast = options.read_preference_tags.is_empty()
                && options.max_staleness.is_none()
                && preference == ReadPreference::Primary;
            let mut candidates = Vec::new();
            let chosen = if fast {
                if !state.topology.compatible() {
                    return Poll::Ready(Err(io::Error::other("incompatible MongoDB wire version")));
                }
                let suitable = |a: &str, s: &crate::topology::Server| {
                    state.pools.contains_key(a) && (s.kind == ServerType::RSPrimary
                        || state.topology.kind == TopologyType::Single && s.kind.readable()
                        || s.kind == ServerType::Standalone || s.kind == ServerType::Mongos)
                };
                state.topology.servers.iter().find(|(a,s)| Some(a.as_str()) != excluded && suitable(a,s))
                    .or_else(|| state.topology.servers.iter().find(|(a,s)| suitable(a,s)))
                    .map(|(a,_)| a.as_str())
            } else {
                let excluded = excluded.map_or([""], |e| [e]);
                if let Err(e) = state.topology.candidates_deprioritized(
                    preference,
                    &options.read_preference_tags,
                    options.max_staleness,
                    &excluded,
                    &mut candidates,
                ) {
                    return Poll::Ready(Err(io::Error::other(e)));
                }
                let mut entropy = [0; 16];
                if turnloop_tls::rustls::crypto::ring::default_provider()
                    .secure_random
                    .fill(&mut entropy)
                    .is_err()
                {
                    return Poll::Ready(Err(io::Error::other("secure entropy unavailable")));
                }
                state.topology.choose(
                    &candidates,
                    [
                        u64::from_le_bytes(entropy[..8].try_into().expect("eight bytes")),
                        u64::from_le_bytes(entropy[8..].try_into().expect("eight bytes")),
                    ],
                )
            };
            if let Some(chosen) = chosen {
                let s = &state.topology.servers[chosen];
                let caps = ServerCapabilities {
                    wire_version: s.max_wire_version,
                    sessions: s.session_timeout.is_some(),
                    standalone: s.kind == ServerType::Standalone,
                    direct: options.direct,
                };
                address.clear();
                address.push_str(chosen);
                return Poll::Ready(Ok(caps));
            }
            if !state.waiters.iter().any(|w| w.will_wake(cx.waker())) {
                state.waiters.push(cx.waker().clone());
            }
            state.topology.request_check(time(owner.executor.now()));
            if let Some(w) = &state.changed {
                w.wake_by_ref();
            }
            Poll::Pending
        }),
    )
    .await
}
async fn run<B: Backend + 'static>(
    owner: &Owner<B>,
    operation: &mut Operation,
    at: Instant,
    selected: &mut String,
    receive: &mut impl FnMut(&RawDocument) -> io::Result<()>,
) -> io::Result<()> {
    loop {
        operation.handle_timeout(time(owner.executor.now()));
        match operation.action() {
            OperationAction::Select {
                read_preference,
                deprioritized,
            } => {
                let caps = select(owner, read_preference, deprioritized, at, selected).await?;
                operation
                    .selected(selected, caps)
                    .map_err(io::Error::other)?;
            }
            OperationAction::Checkout { address } => {
                let pool = owner
                    .state
                    .borrow()
                    .pools
                    .get(address)
                    .cloned()
                    .ok_or_else(|| io::Error::other("selected pool disappeared"))?;
                match pool.acquire(at).await {
                    Ok(mut connection) => {
                        operation.checked_out().map_err(io::Error::other)?;
                        let complete = connection.operation(operation, at, &mut *receive).await?;
                        if complete {
                            return Ok(());
                        }
                        let mut state = owner.state.borrow_mut();
                        if let (Some(error), Some(server)) = (operation.last_error(), state.topology.servers.get(selected.as_str())) {
                            let application = ApplicationError {
                                generation:server.generation, max_wire_version:server.max_wire_version,
                                handshake_complete:true,
                                kind:match error.kind {
                                    crate::ErrorKind::Network => ApplicationErrorKind::Network,
                                    crate::ErrorKind::Timeout => ApplicationErrorKind::Timeout,
                                    _ => ApplicationErrorKind::Command,
                                },
                                response:error.response.as_deref(),
                            };
                            state.topology.application_error(selected, application, time(owner.executor.now()));
                        }
                        state.topology.request_check(time(owner.executor.now()));
                        if let Some(w) = &state.changed {
                            w.wake_by_ref();
                        }
                    }
                    Err(e) => operation
                        .failed(crate::Error::new(crate::ErrorKind::Network, e.to_string())),
                }
            }
            OperationAction::Complete { .. } => return Ok(()),
            OperationAction::Failed { error, .. } => return Err(io::Error::other(error.clone())),
            _ => return Err(io::Error::other("unexpected operation coordinator state")),
        }
    }
}
pub struct Cursor<B: Backend + 'static> {
    owner: Rc<Owner<B>>,
    core: crate::command::Cursor,
    rows: VecDeque<RawDocumentBuf>,
    command: crate::command::Command,
    closed: bool,
}
impl<B: Backend + 'static> Cursor<B> {
    pub async fn next(&mut self, at: Instant) -> io::Result<Option<RawDocumentBuf>> {
        loop {
        if let Some(row) = self.rows.pop_front() {
            return Ok(Some(row));
        }
        if self.closed
            || !self
                .core
                .get_more(&mut self.command)
                .map_err(io::Error::other)?
        {
            return Ok(None);
        }
        let pool = self
            .owner
            .state
            .borrow()
            .pools
            .get(&self.core.server)
            .cloned()
            .ok_or_else(|| io::Error::other("cursor server removed"))?;
        let mut connection = pool.acquire(at).await?;
        connection
            .command(self.command.raw(), &[], at, |reply| {
                let batch = crate::command::CursorBatch::parse(reply).map_err(io::Error::other)?;
                self.core.accept(&batch).map_err(io::Error::other)?;
                for row in batch.rows() {
                    self.rows
                        .push_back(row.map_err(io::Error::other)?.to_owned());
                }
                Ok(())
            })
            .await?;
        // A live cursor may return an empty batch. Await another real getMore;
        // only id=0 terminates the stream, and the same deadline bounds the loop.
        }
    }
    pub async fn close(&mut self, at: Instant) -> io::Result<()> {
        if self.core.id != 0 && !self.closed {
            self.command
                .kill_cursor(&self.core.database, &self.core.collection, self.core.id)
                .map_err(io::Error::other)?;
            let pool = self
                .owner
                .state
                .borrow()
                .pools
                .get(&self.core.server)
                .cloned()
                .ok_or_else(|| io::Error::other("cursor server removed"))?;
            pool.acquire(at)
                .await?
                .command(self.command.raw(), &[], at, |_| Ok(()))
                .await?;
        }
        self.closed = true;
        self.core.id = 0;
        self.rows.clear();
        Ok(())
    }
}

impl<B: Backend + 'static> Drop for Cursor<B> {
    fn drop(&mut self) {
        if self.closed || self.core.id == 0 { return; }
        let pool = self.owner.state.borrow().pools.get(&self.core.server).cloned();
        let Some(pool) = pool else { return; };
        let mut command = crate::command::Command::new();
        if command.kill_cursor(&self.core.database, &self.core.collection, self.core.id).is_err() { return; }
        let at = self.owner.executor.now() + Duration::from_secs(5);
        let mut state = self.owner.state.borrow_mut();
        state.cleanup.push(Box::pin(async move {
            if let Ok(mut connection) = pool.acquire(at).await {
                let _ = connection.command(command.raw(), &[], at, |_| Ok(())).await;
            }
        }));
        if let Some(w) = &state.changed { w.wake_by_ref(); }
    }
}
