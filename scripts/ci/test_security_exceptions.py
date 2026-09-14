"""Security exceptions bypass age only for one verified, still-young lock entry."""
from contextlib import redirect_stdout
from datetime import date, datetime, timedelta, timezone
import io
import unittest
from soak import check_age, check_unused, security_exceptions


class SecurityExceptions(unittest.TestCase):
    def setUp(self):
        self.now = datetime(2026, 9, 14, 16, tzinfo=timezone.utc)
        self.published = self.now - timedelta(minutes=49)
        self.package = {'name': 'rustls', 'version': '0.23.45', 'checksum': 'a' * 64}
        self.record = {'cksum': 'a' * 64, 'pubtime': self.published.isoformat()}
        self.entry = dict(crate='rustls', version='0.23.45', advisory='RUSTSEC-2026-0285',
                          reason='Fix TLS encryption-level validation', expires=date(2026, 9, 21))

    def parse(self, entry=None, now=None):
        return security_exceptions({'security-exceptions': [self.entry if entry is None else entry]}, now or self.now)

    def test_active_exact_exception_prints_all_fields(self):
        exceptions = self.parse()
        key = ('rustls', '0.23.45')
        output = io.StringIO()
        with redirect_stdout(output):
            self.assertTrue(check_age(self.package, self.record, self.now, exceptions[key]))
        for value in self.entry.values():
            self.assertIn(str(value), output.getvalue())
        check_unused(exceptions, {key})
        self.assertEqual(len(exceptions), 1)

    def test_other_versions_and_crates_still_soak(self):
        exceptions = self.parse()
        for change in ({'version': '0.23.46'}, {'name': 'other'}):
            p = {**self.package, **change}
            with self.subTest(change=change), self.assertRaisesRegex(RuntimeError, 'Supply-chain soak'):
                check_age(p, self.record, self.now, exceptions.get((p['name'], p['version'])))
        with self.assertRaisesRegex(RuntimeError, 'does not match'):
            check_age({**self.package, 'version': '0.23.46'}, self.record, self.now, self.entry)

    def test_checksum_and_future_time_cannot_be_exempted(self):
        for record, error in [({**self.record, 'cksum': 'b' * 64}, 'Checksum mismatch'),
                              ({**self.record, 'pubtime': (self.now + timedelta(seconds=1)).isoformat()}, 'future')]:
            with self.subTest(record=record), self.assertRaisesRegex(RuntimeError, error):
                check_age(self.package, record, self.now, self.entry)

    def test_expired_unused_and_eligibility_boundary_fail(self):
        with self.assertRaisesRegex(RuntimeError, 'Expired'):
            self.parse(now=self.now + timedelta(days=8))
        with self.assertRaisesRegex(RuntimeError, 'Unused'):
            check_unused(self.parse(), set())
        eligible = self.published + timedelta(days=7)
        self.assertTrue(check_age(self.package, self.record, eligible - timedelta(microseconds=1), self.entry))
        with self.assertRaisesRegex(RuntimeError, 'Unused'):
            check_age(self.package, self.record, eligible, self.entry)
        self.assertFalse(check_age(self.package, self.record, eligible))
        for expires in (date(2026, 9, 20), date(2026, 9, 22)):
            with self.subTest(expires=expires), self.assertRaisesRegex(RuntimeError, 'publish date'):
                check_age(self.package, self.record, self.now, {**self.entry, 'expires': expires})

    def test_missing_malformed_and_duplicate_entries_fail(self):
        for field in self.entry:
            entry = self.entry.copy()
            del entry[field]
            with self.subTest(field=field), self.assertRaises(RuntimeError):
                self.parse(entry)
        for field, value in [('version', '^0.23.45'), ('version', '*'), ('advisory', 'none'),
                             ('reason', ' '), ('crate', '*'), ('expires', 7), ('reason', None)]:
            with self.subTest(field=field, value=value), self.assertRaises(RuntimeError):
                self.parse({**self.entry, field: value})
        with self.assertRaisesRegex(RuntimeError, 'Duplicate'):
            security_exceptions({'security-exceptions': [self.entry, self.entry]}, self.now)
        self.assertEqual(self.parse({**self.entry, 'expires': '2026-09-21'}), self.parse())


if __name__ == '__main__':
    unittest.main()
