"""Owned WebDriver lifetime with persistent logs and explicit browser binaries."""
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import socket
import subprocess
import time
import urllib.error
import urllib.request
from common import ROOT, fail


class BrowserDriver:
    def __init__(self, browser, env, *, paths=None, logs=None, timeout=15):
        self.browser = browser
        self.env = env.copy()
        self.timeout = timeout
        self.logs = logs or ROOT / '.tools/browser-logs'
        self.process = None
        self.log = None
        self.paths = paths

    def __enter__(self):
        self.logs.mkdir(parents=True, exist_ok=True)
        self.log_path = self.logs / (self.browser + '-driver.log')
        self.log = self.log_path.open('w')
        try:
            name = 'chromedriver' if self.browser == 'chrome' else 'geckodriver'
            if self.paths is None:
                config = Path(self.env.get('TURNLOOP_BROWSER_PATHS', ROOT / '.tools/browsers/paths.json'))
                if config.is_file():
                    self.paths = json.loads(config.read_text())
                elif platform.system() == 'Linux' or self.env.get('CI'):
                    fail('Install pinned browsers first: python3 scripts/ci/install-browsers.py')
                else:
                    # Local Mac diagnostics use explicitly supplied/installed tools.
                    driver = self.env.get(name.upper()) or shutil.which(name)
                    if not driver:
                        fail(f'No local {name}; Linux CI requires the pinned installer')
                    self.paths = {name: driver}
            driver = self.paths[name]
            self.driver_args = ['--' + name, driver]
            caps = {}
            if self.browser in self.paths:
                key = 'goog:chromeOptions' if self.browser == 'chrome' else 'moz:firefoxOptions'
                caps[key] = {'binary': self.paths[self.browser]}
            capabilities = self.logs / (self.browser + '-capabilities.json')
            capabilities.write_text(json.dumps(caps))
            self.env['WASM_BINDGEN_TEST_WEBDRIVER_JSON'] = str(capabilities.resolve())
            for key in ('CHROMEDRIVER_REMOTE', 'GECKODRIVER_REMOTE', 'SAFARIDRIVER_REMOTE'):
                self.env.pop(key, None)
            with socket.socket() as reservation:
                reservation.bind(('127.0.0.1', 0))
                port = reservation.getsockname()[1]
            command = [driver, f'--port={port}']
            command += ['--verbose'] if self.browser == 'chrome' else ['--log', 'trace']
            print('Starting owned driver: ' + ' '.join(command), flush=True)
            self.log.write('command: ' + ' '.join(command) + '\n')
            self.log.flush()
            self.process = subprocess.Popen(command, stdout=self.log, stderr=subprocess.STDOUT,
                                            env=self.env, start_new_session=True)
            url = f'http://127.0.0.1:{port}'
            until = time.monotonic() + self.timeout
            while time.monotonic() < until:
                if self.process.poll() is not None:
                    fail(f'{name} exited {self.process.returncode} before /status; zero browser tests ran')
                try:
                    with urllib.request.urlopen(url + '/status', timeout=0.5) as response:
                        status = json.load(response)
                    if status.get('value', {}).get('ready') is True:
                        self.env[name.upper() + '_REMOTE'] = url
                        return self
                except (OSError, ValueError, urllib.error.URLError):
                    pass
                time.sleep(0.05)
            fail(f'{name} /status timeout; zero browser tests ran')
        except BaseException:
            self.__exit__(*__import__('sys').exc_info())
            raise

    def __exit__(self, kind, value, traceback):
        if self.process is not None:
            # The process group belongs exclusively to this invocation, including
            # any browser children still alive after failed session creation.
            try:
                os.killpg(self.process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait(timeout=5)
        if self.log is not None:
            self.log.close()
        if kind is not None:
            print(f'Browser failure: {value}\nDriver diagnostics: {self.log_path}', flush=True)
            print(self.log_path.read_text(errors='replace'), flush=True)
        return False
