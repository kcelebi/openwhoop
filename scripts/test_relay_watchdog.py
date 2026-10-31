import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
import importlib.util
import sys
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('relay_watchdog', Path(__file__).with_name('relay-watchdog.py'))
watchdog = importlib.util.module_from_spec(spec)
sys.modules['relay_watchdog'] = watchdog
spec.loader.exec_module(watchdog)
check = watchdog.check


class WatchdogTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.db = str(Path(self.directory.name) / 'relay.sqlite')
        self.state = str(Path(self.directory.name) / 'watchdog.json')
        with sqlite3.connect(self.db) as c:
            c.execute('create table cloud_hr_outbox(received_at_ms integer)')
            c.execute('insert into cloud_hr_outbox values (1000)')

    def run_at(self, seconds):
        with patch('relay_watchdog.time.monotonic', return_value=seconds):
            check(self.db, self.state)

    @patch('relay_watchdog.subprocess.run')
    def test_stale_database_gets_startup_grace_then_restart(self, restart):
        self.run_at(100)
        self.run_at(189)
        restart.assert_not_called()
        self.run_at(190)
        restart.assert_called_once()
        self.assertEqual(restart.call_args.args[0][-3:-1], ['kickstart', '-k'])
        self.run_at(200)
        restart.assert_called_once()

    @patch('relay_watchdog.subprocess.run')
    def test_advancing_sample_resets_stall_timer(self, restart):
        self.run_at(100)
        with sqlite3.connect(self.db) as c:
            c.execute('insert into cloud_hr_outbox values (2000)')
        self.run_at(185)
        self.run_at(270)
        restart.assert_not_called()

    @patch('relay_watchdog.subprocess.run')
    def test_reboot_resets_monotonic_timer(self, restart):
        self.run_at(500)
        self.run_at(10)
        restart.assert_not_called()
        self.assertEqual(json.loads(Path(self.state).read_text())['last_progress'], 10)


if __name__ == '__main__':
    unittest.main()
