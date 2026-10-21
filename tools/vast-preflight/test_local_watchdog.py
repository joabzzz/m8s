"""Temporary local journals and exact in-memory FakeProvider only."""
import dataclasses
import json
import os
from pathlib import Path
import tempfile
import unittest

from local_watchdog import FakeProvider, Inventory, LocalWatchdog, OwnedVolume
from preflight import OwnedInstance, PreflightError


class WatchdogTests(unittest.TestCase):
    def setUp(self):
        self.assertNotIn('VAST_API_KEY', os.environ)
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        os.chmod(self.tmp.name, 0o700)
        self.row = OwnedInstance(123, 'owner', 'run1', 'stopped')
        self.provider = FakeProvider(Inventory(100, True, (self.row,), (), True))

    def open(self):
        return LocalWatchdog(self.tmp.name, self.provider)

    def start(self, watch):
        watch.prepare('owner', 'run1', 100, 200, 1_000_000)
        state = json.loads((Path(self.tmp.name) / 'journal.json').read_text())
        self.assertEqual(state['runs'][0]['phase'], 'prepared')
        watch.mark_submitted()
        watch.bind(123, 100)

    def test_intent_persisted_before_submission_and_no_retry_after_restart(self):
        with self.open() as watch:
            watch.prepare('owner', 'run1', 100, 200, 100_000)
            watch.mark_submitted()
        with self.open() as watch:
            with self.assertRaises(PreflightError):
                watch.mark_submitted()
            # Uncertain create absent snapshot must never become confirmed absence.
            self.provider.inventory = Inventory(100, True, (), (), True)
            self.assertEqual(watch.tick(100), 'retry-launch-uncertain')
            self.assertTrue(watch.state['blocked'])
            with self.assertRaises(PreflightError):
                watch.prepare('owner', 'run2', 101, 201, 100_000)

    def test_restart_recovery_bind_and_repeated_destroy_until_absence(self):
        with self.open() as watch:
            watch.prepare('owner', 'run1', 100, 200, 1_000_000)
            watch.mark_submitted()
        with self.open() as watch:
            watch.bind(123, 100)
            self.assertEqual(watch.tick(100), 'destroy')
            self.assertEqual(watch.tick(100), 'destroy')
            self.assertEqual(self.provider.destroyed_instances, [123, 123])
            self.provider.inventory = Inventory(100, True, (), (), True)
            self.assertEqual(watch.tick(100), 'confirmed-absent')
            self.assertEqual(watch.state['spent_micro_usd'], 1_000_000)
        with self.open() as watch:
            self.assertEqual(watch.state['runs'][0]['phase'], 'confirmed')
            watch.prepare('owner', 'run2', 100, 200, 1_000_000)
            watch.tick(100)  # prepared, never submitted; absence settles reserve conservatively
            with self.assertRaises(PreflightError):
                watch.prepare('owner', 'run3', 100, 200, 100_000)
            self.assertFalse(watch.live_authorized)

    def test_account_wide_lock_serializes_competing_owners(self):
        with self.open():
            with self.assertRaises(PreflightError):
                self.open()
        with self.open():
            pass

    def test_volume_binding_destroy_and_mismatch_blocks(self):
        self.provider.inventory = dataclasses.replace(self.provider.inventory,
            volumes=(OwnedVolume(456, 'owner', 'run1'), OwnedVolume(789, 'other', 'run1')))
        with self.open() as watch:
            self.start(watch)
            self.assertEqual(watch.tick(100), 'destroy')
            self.assertEqual(watch.state['runs'][0]['bound_volumes'], [456])
            self.assertEqual(self.provider.destroyed_volumes, [456])
            self.provider.inventory = Inventory(100, True, (),
                                                (OwnedVolume(456, 'other', 'run1'),), True)
            with self.assertRaises(PreflightError):
                watch.tick(100)
            self.assertTrue(watch.state['blocked'])
            self.assertNotEqual(watch.state['runs'][0]['phase'], 'confirmed')

    def test_absent_instance_still_retries_storage(self):
        with self.open() as watch:
            self.start(watch)
            self.provider.inventory = Inventory(100, True, (), (OwnedVolume(456, 'owner', 'run1'),), True)
            self.assertEqual(watch.tick(100), 'retry-storage')
            self.assertEqual(self.provider.destroyed_volumes, [456])
            self.provider.inventory = Inventory(100, True, (), (), True)
            self.assertEqual(watch.tick(100), 'confirmed-absent')
            self.assertTrue(watch.state['blocked'])  # sticky admission latch survives cleanup

    def test_partial_stale_read_timeout_and_destroy_failure_keep_intent(self):
        with self.open() as watch:
            self.start(watch)
            for inventory in (Inventory(100, False, (), (), True),
                              Inventory(100, True, (), (), False),
                              Inventory(0, True, (), (), True)):
                self.provider.inventory = inventory
                with self.assertRaises(PreflightError):
                    watch.tick(100)
                self.assertNotEqual(watch.state['runs'][0]['phase'], 'confirmed')
            self.provider.snapshot_fails = True
            with self.assertRaises(PreflightError):
                watch.tick(100)
            self.provider.snapshot_fails = False
            self.provider.inventory = Inventory(100, True, (self.row,), (), True)
            self.provider.destroy_fails = True
            with self.assertRaises(PreflightError):
                watch.tick(100)
        with self.open() as watch:
            self.assertTrue(watch.state['blocked'])
            self.assertEqual(watch.state['runs'][0]['reserve'], 1_000_000)

    def test_owner_mismatch_cannot_destroy_or_confirm_absence(self):
        with self.open() as watch:
            self.start(watch)
            self.provider.inventory = Inventory(100, True,
                (dataclasses.replace(self.row, owner='other'),), (), True)
            with self.assertRaises(PreflightError):
                watch.tick(100)
            self.assertEqual(self.provider.destroyed_instances, [])

    def test_binding_requires_exact_positive_fresh_owner_observation(self):
        with self.open() as watch:
            watch.prepare('owner', 'run1', 100, 200, 100_000)
            watch.mark_submitted()
            for instance_id, now in ((999, 100), (123, 200)):
                with self.assertRaises(PreflightError):
                    watch.bind(instance_id, now)

    def test_corrupt_journal_or_deleted_initialized_journal_blocks_reset(self):
        with self.open():
            pass
        journal = Path(self.tmp.name) / 'journal.json'
        journal.write_bytes(b'{"version":1,"version":1}')
        with self.assertRaises(PreflightError):
            self.open()
        journal.unlink()
        with self.assertRaises(PreflightError):
            self.open()

    def test_crash_pending_file_prevents_any_effect(self):
        with self.open() as watch:
            watch.prepare('owner', 'run1', 100, 200, 100_000)
            (Path(self.tmp.name) / 'journal.pending').write_bytes(b'crash partial')
            with self.assertRaises(PreflightError):
                watch.mark_submitted()
        self.assertEqual(self.provider.destroyed_instances, [])
        self.assertEqual(json.loads((Path(self.tmp.name) / 'journal.json').read_text())['runs'][0]['phase'],
                         'prepared')

    def test_poll_retries_fake_effects_and_never_treats_exhaustion_as_absence(self):
        with self.open() as watch:
            self.start(watch)
            def absent_after_destroy():
                self.provider.inventory = Inventory(100, True, (), (), True)
            self.assertEqual(watch.poll(2, lambda: 100, absent_after_destroy), 'confirmed-absent')
        # Fresh directory for unresolved loop exhaustion.
        with tempfile.TemporaryDirectory() as directory:
            os.chmod(directory, 0o700)
            self.provider.inventory = Inventory(100, True, (self.row,), (), True)
            with LocalWatchdog(directory, self.provider) as watch:
                watch.prepare('owner', 'run1', 100, 200, 100_000)
                watch.mark_submitted()
                watch.bind(123, 100)
                self.assertEqual(watch.poll(2, lambda: 100, lambda: None), 'alert-poll-exhausted')
                self.assertTrue(watch.state['blocked'])
                self.assertEqual(watch.state['runs'][0]['phase'], 'bound')

    def test_scheduler_callback_failure_latches_admission(self):
        def failed_pause():
            raise OSError('synthetic callback failure')
        with self.open() as watch:
            self.start(watch)
            self.assertEqual(watch.poll(2, lambda: 100, failed_pause), 'alert-watchdog-failure')
            self.assertTrue(watch.state['blocked'])

    def test_no_real_provider_or_insecure_directory(self):
        with self.assertRaises(PreflightError):
            LocalWatchdog(self.tmp.name, object())
        os.chmod(self.tmp.name, 0o755)
        with self.assertRaises(PreflightError):
            self.open()


if __name__ == '__main__':
    unittest.main()
