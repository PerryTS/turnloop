"""A process-owned HTTP/WebSocket fixture; assert traffic before declaring PASS."""
import json
import queue
import subprocess
import threading
import urllib.request


class WebFixture:
    def __init__(self, path):
        self.path = path
        self.process = None
        self.url = None

    def __enter__(self):
        self.process = subprocess.Popen(['node', str(self.path)], text=True,
                                        stdout=subprocess.PIPE)
        ready = queue.Queue()
        threading.Thread(target=lambda: ready.put(self.process.stdout.readline()), daemon=True).start()
        try:
            self.url = json.loads(ready.get(timeout=10))['url']
            print('Web fixture: ' + self.url, flush=True)
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def verify(self, minimum):
        with urllib.request.urlopen(self.url + '/stats', timeout=5) as response:
            stats = json.load(response)
        print('Web fixture traffic: ' + json.dumps(stats), flush=True)
        for field, threshold in {'fetches': minimum * 3, 'slow': minimum,
                                 'aborted': minimum, 'websockets': minimum * 4,
                                 'echoed': minimum * (257 + 6400)}.items():
            if stats.get(field, 0) < threshold:
                raise RuntimeError(f'Fixture subject did not run: {field} < {threshold}: {stats}')

    def __exit__(self, *_):
        if self.process is not None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
            self.process.stdout.close()
