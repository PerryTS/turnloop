"""Loopback TCP forwarder for the PostgreSQL docker-proxy reproduction.

Each accepted socket gets a distinct upstream connection. Preserve half-closes
and bytes (including TLS); shutdown wakes blocking relays and joins every thread.
This is test tooling, not a turnloop backend.
"""
import select
import socket
import socketserver
import threading


class TcpProxy:
    def __init__(self, upstream_port):
        self.connections = 0
        self.cancel_requests = 0
        self.bytes_forwarded = 0
        self._lock = threading.Lock()
        self._sockets = set()
        self._stopping = False
        proxy = self

        class Relay(socketserver.BaseRequestHandler):
            def handle(self):
                with proxy._lock:
                    if proxy._stopping:
                        return
                    proxy.connections += 1
                    proxy._sockets.add(self.request)
                upstream = None
                try:
                    upstream = socket.create_connection(('127.0.0.1', upstream_port), timeout=5)
                    with proxy._lock:
                        if proxy._stopping:
                            return
                        proxy._sockets.add(upstream)
                    self.request.settimeout(5)
                    upstream.settimeout(5)
                    peers = {self.request: upstream, upstream: self.request}
                    prefix = bytearray()
                    while peers:
                        # No periodic polling: socket shutdown wakes this wait.
                        readable, _, _ = select.select(list(peers), [], [])
                        for source in readable:
                            data = source.recv(65536)
                            destination = peers[source]
                            if not data:
                                destination.shutdown(socket.SHUT_WR)
                                del peers[source]
                                continue
                            destination.sendall(data)
                            with proxy._lock:
                                proxy.bytes_forwarded += len(data)
                                if source is self.request and len(prefix) < 8:
                                    prefix.extend(data[:8 - len(prefix)])
                                    if prefix == b'\x00\x00\x00\x10\x04\xd2\x16\x2e':
                                        proxy.cancel_requests += 1
                except OSError:
                    # A failed relay closes its peer; the client test must see it.
                    pass
                finally:
                    with proxy._lock:
                        proxy._sockets.discard(self.request)
                        proxy._sockets.discard(upstream)
                    if upstream is not None:
                        upstream.close()

        self._server = socketserver.ThreadingTCPServer(('127.0.0.1', 0), Relay)
        self.port = self._server.server_address[1]
        self._thread = threading.Thread(target=self._server.serve_forever)

    def __enter__(self):
        self._thread.start()
        return self

    def __exit__(self, *_args):
        with self._lock:
            self._stopping = True
            for stream in self._sockets:
                try:
                    stream.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
        self._server.shutdown()
        self._server.server_close()  # joins the non-daemon relay threads
        self._thread.join()
