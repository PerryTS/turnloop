"""Pinned browser supply chain and owned-driver failure regressions."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
import zipfile
from common import ROOT
from browser_driver import BrowserDriver

spec = importlib.util.spec_from_file_location('install_browsers', ROOT / 'scripts/ci/install-browsers.py')
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class BrowserTests(unittest.TestCase):
    def test_pins_match_and_archives_are_verified_before_extraction(self):
        pins = json.loads((ROOT / 'scripts/ci/browsers.json').read_text())['Linux-x86_64']
        self.assertEqual(pins['chrome']['version'], pins['chromedriver']['version'])
        for pin in pins.values():
            self.assertRegex(pin['sha256'], r'^[0-9a-f]{64}$')
            self.assertIn(pin['version'], pin['url'])
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            archive = root / 'browser.zip'
            with zipfile.ZipFile(archive, 'w') as bundle:
                bundle.writestr('browser/bin', b'actual executable')
            pin = {'sha256': '0' * 64, 'executable': 'browser/bin'}
            with self.assertRaisesRegex(RuntimeError, 'SHA-256 mismatch'):
                installer.extract_verified(archive, pin, root / 'bad')
            self.assertFalse((root / 'bad').exists())
            pin['sha256'] = hashlib.sha256(archive.read_bytes()).hexdigest()
            executable = installer.extract_verified(archive, pin, root / 'good')
            self.assertEqual(executable.read_bytes(), b'actual executable')
            with zipfile.ZipFile(archive, 'w') as bundle:
                bundle.writestr('../escape', b'bad')
            pin['sha256'] = hashlib.sha256(archive.read_bytes()).hexdigest()
            with self.assertRaisesRegex(RuntimeError, 'Unsafe browser archive path'):
                installer.extract_verified(archive, pin, root / 'unsafe')
            self.assertFalse((root / 'escape').exists())

    def test_real_driver_stderr_is_diagnostic_not_startup_failure(self):
        import contextlib
        import sys
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            driver = root / 'driver'
            driver.write_text(f'#!{sys.executable}\n' + '''import http.server, json, sys
print('driver warning: still healthy', file=sys.stderr, flush=True)
port = int(next(a.split('=')[1] for a in sys.argv if a.startswith('--port=')))
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  self.send_response(200); self.end_headers()
  self.wfile.write(json.dumps({'value': {'ready': True}}).encode())
http.server.HTTPServer(('127.0.0.1', port), Handler).serve_forever()
''')
            driver.chmod(0o755)
            capture = io.StringIO()
            with contextlib.redirect_stdout(capture), self.assertRaisesRegex(RuntimeError, 'zero passed'):
                with BrowserDriver('chrome', {}, paths={'chromedriver': str(driver)}, logs=root) as live:
                    process = live.process
                    self.assertIn('CHROMEDRIVER_REMOTE', live.env)
                    # Exercise the actual zero-test guard after driver startup.
                    spec = importlib.util.spec_from_file_location('run_tests', ROOT / 'scripts/ci/run-tests.py')
                    runner = importlib.util.module_from_spec(spec)
                    spec.loader.exec_module(runner)
                    with self.assertRaisesRegex(RuntimeError, 'positive passed-test count'):
                        runner.checked_tests([sys.executable, '-c', 'print("test result: ok. 0 passed; 7 ignored;")'], cwd=root)
                    raise RuntimeError('zero passed')
            self.assertIsNotNone(process.poll())
            self.assertIn('driver warning: still healthy', capture.getvalue())
            self.assertIn('Driver diagnostics:', capture.getvalue())
            # A dead driver fails promptly, logs its exact status and is reaped.
            driver.write_text(f'#!{sys.executable}\nimport sys\nprint("cannot launch", flush=True)\nsys.exit(23)\n')
            with contextlib.redirect_stdout(capture), self.assertRaisesRegex(RuntimeError, 'exited 23.*zero browser tests ran'):
                with BrowserDriver('chrome', {}, paths={'chromedriver': str(driver)}, logs=root):
                    self.fail('dead driver cannot be ready')
            self.assertIn('cannot launch', capture.getvalue())


if __name__ == '__main__':
    unittest.main()
