// Platform host imports. Host callbacks post records; only the host dispatches user work.
export class Host {
  constructor(post, schedule) {
    this.post = post; this.schedule = schedule;
    this.operations = new Map(); this.scheduled = false;
    this.schedules = 0; this.callbacks = 0; this.epoch = 0; this.disposed = false;
  }
  wake() {
    if (this.disposed || this.scheduled) return;
    this.scheduled = true; this.schedules++;
    const epoch = ++this.epoch;
    queueMicrotask(() => { if (!this.disposed && this.scheduled && epoch === this.epoch) { this.callbacks++; this.schedule(); } });
  }
  beginTurn() { this.scheduled = false; this.epoch++; }
  complete(id, kind, value) {
    if (!this.disposed && this.post(id, kind, value)) this.wake();
  }
  timer(id, ms) {
    const deadline = performance.now() + ms;
    let handle;
    const fire = () => {
      const now = performance.now();
      if (now < deadline) { handle = setTimeout(fire, Math.ceil(deadline - now)); return; }
      this.complete(id, 1, now);
    };
    handle = setTimeout(fire, Math.ceil(ms));
    this.operations.set(id, { cancel: () => clearTimeout(handle) });
  }
  fetch(id, url) {
    const controller = new AbortController();
    this.operations.set(id, { cancel: () => controller.abort() });
    fetch(url, { signal: controller.signal }).then(async response => {
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      this.complete(id, 2, new Uint8Array(await response.arrayBuffer()));
    }).catch(error => this.complete(id, 4, String(error)));
  }
  websocket(id, url, bytes) {
    const socket = new WebSocket(url); socket.binaryType = 'arraybuffer';
    this.operations.set(id, { cancel: () => {
      socket.onopen = socket.onmessage = socket.onerror = socket.onclose = null;
      socket.close();
    }});
    socket.onopen = () => socket.send(bytes);
    socket.onmessage = event => this.complete(id, 3, new Uint8Array(event.data));
    socket.onerror = () => this.complete(id, 4, 'WebSocket error');
    socket.onclose = () => this.complete(id, 4, 'WebSocket closed before response');
  }
  cancel(id) { this.operations.get(id)?.cancel(); }
  release(id) { this.cancel(id); this.operations.delete(id); }
  dispose() {
    this.disposed = true;
    for (const id of this.operations.keys()) this.release(id);
    this.post = () => false; this.schedule = () => {};
  }
}
export function makeHost(post, schedule) { return new Host(post, schedule); }
