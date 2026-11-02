"""Synthetic artifact tests only; not production lifecycle admission evidence."""
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HELPER = Path(__file__).with_name('tentaflake-recovery.py')
SPEC = importlib.util.spec_from_file_location('recovery', HELPER)
recovery = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(recovery)


class Boundary:
    def __init__(self, hold, stop=lambda: None):
        self.hold = hold
        self.action = stop
        self.stopped = False

    def stop(self, units):
        assert self.hold.is_file() and units == ['synthetic.service']
        self.action()
        self.stopped = True

    def verify(self, units):
        assert self.stopped and self.hold.is_file()


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / 'source'
        self.source.mkdir(mode=0o700)
        self.hold = self.root / 'recovery-hold'
        self.snapshot = self.root / 'snapshot'
        self.target = self.root / 'restored'
        self.plan = {'version': 1, 'coverage': 'selected', 'scope': 'tenant:agent-a',
                     'revision': 'policy-revision-1', 'runtime': 'pi-durable:pinned-runtime-1',
                     'units': ['synthetic.service'],
                     'sources': [{'path': str(self.source), 'role': 'workspace', 'schema': 1}]}

    def capture(self, action=lambda: None):
        return recovery.checkpoint(self.plan, self.snapshot, self.hold, Boundary(self.hold, action))

    def test_live_sqlite_wal_commit_and_ids_survive_without_source_checkpoint(self):
        db = sqlite3.connect(self.source / 'actions.sqlite3')
        self.addCleanup(db.close)
        db.execute('PRAGMA journal_mode=WAL')
        db.execute('PRAGMA wal_autocheckpoint=0')
        db.execute('CREATE TABLE operations(id TEXT PRIMARY KEY, dispatched INTEGER)')
        db.commit()
        # The synthetic boundary commits an in-flight writer, but deliberately
        # keeps its idle connection open so that required WAL bytes remain.
        db.execute("INSERT INTO operations VALUES('spent-operation-1', 1)")
        checksum = self.capture(db.commit)
        self.assertTrue((self.snapshot / '0/actions.sqlite3-wal').is_file())
        recovery.restore(self.snapshot, self.target, checksum)
        with sqlite3.connect(self.target / '0/actions.sqlite3') as restored:
            self.assertEqual(restored.execute('SELECT * FROM operations').fetchall(), [('spent-operation-1', 1)])
            with self.assertRaises(sqlite3.IntegrityError):
                restored.execute("INSERT INTO operations VALUES('spent-operation-1', 1)")
        self.assertTrue(self.hold.exists())
        self.assertTrue((self.target / 'recovery-hold').exists())

    def test_writer_interruption_preserves_hold_without_commit(self):
        def interrupted():
            raise InterruptedError('synthetic writer interruption')
        with self.assertRaises(InterruptedError):
            self.capture(interrupted)
        self.assertTrue(self.hold.exists())
        self.assertFalse(self.snapshot.exists())

    def test_backup_interruption_keeps_private_uncommitted_evidence(self):
        (self.source / 'receipt').write_text('durable')
        with patch.object(recovery, 'sqlite_evidence', side_effect=InterruptedError):
            with self.assertRaises(InterruptedError):
                self.capture()
        self.assertTrue(self.hold.exists())
        self.assertEqual(self.snapshot.stat().st_mode & 0o777, 0o700)
        self.assertFalse((self.snapshot / 'manifest.json').exists())
        with self.assertRaises(OSError):
            recovery.restore(self.snapshot, self.target, '0' * 64)
        self.assertFalse(self.target.exists())

    def test_missing_authority_receipts_refuse_commit(self):
        self.plan['sources'][0]['role'] = 'pi-durable'
        with self.assertRaisesRegex(recovery.Refusal, 'missing authoritative receipt'):
            self.capture()
        self.assertTrue(self.hold.exists())
        self.assertFalse((self.snapshot / 'manifest.json').exists())

    def test_tamper_or_removed_receipt_refuses_before_creating_target(self):
        (self.source / 'receipt').write_text('request-id-1')
        checksum = self.capture()
        receipt = self.snapshot / '0/receipt'
        receipt.write_text('request-id-2')
        with self.assertRaises(recovery.Refusal):
            recovery.restore(self.snapshot, self.target, checksum)
        receipt.unlink()
        with self.assertRaises(recovery.Refusal):
            recovery.restore(self.snapshot, self.target, checksum)
        self.assertFalse(self.target.exists())

    def test_manifest_scope_revision_runtime_tamper(self):
        checksum = self.capture()
        path = self.snapshot / 'manifest.json'
        original = path.read_bytes()
        for key in ('scope', 'revision', 'runtime'):
            manifest = json.loads(original)
            manifest['plan'][key] = 'other-identity'
            path.write_bytes(recovery.encoded(manifest))
            with self.assertRaisesRegex(recovery.Refusal, 'digest mismatch'):
                recovery.restore(self.snapshot, self.target, checksum)
        self.assertFalse(self.target.exists())

    def test_restore_is_create_only_private_and_never_targets_live_source(self):
        (self.source / 'receipt').write_text('original')
        checksum = self.capture()
        for target in (self.source, self.root, self.target):
            if target == self.target:
                target.mkdir(mode=0o700)
            with self.assertRaises(recovery.Refusal):
                recovery.restore(self.snapshot, target, checksum)
        self.target.rmdir()
        self.target.symlink_to(self.source, target_is_directory=True)
        with self.assertRaises(recovery.Refusal):
            recovery.restore(self.snapshot, self.target, checksum)
        self.target.unlink()
        recovery.restore(self.snapshot, self.target, checksum)
        self.assertEqual(self.target.stat().st_mode & 0o777, 0o700)
        self.assertEqual((self.target / '0/receipt').stat().st_mode & 0o777, 0o600)
        self.assertEqual((self.source / 'receipt').read_text(), 'original')

    def test_coherent_older_budget_never_refunds_live_spend_or_authorizes_replay(self):
        self.plan['sources'][0]['role'] = 'budget'
        for name in recovery.REQUIRED['budget']:
            (self.source / name).write_text('{}')
        old = {'day': 1, 'requests': 10, 'cost_microusd': 100, 'unsettled': 2}
        budget = self.source / 'budget.json'
        budget.write_text(json.dumps(old))
        checksum = self.capture()
        newer = dict(old, requests=20, cost_microusd=200, unsettled=3)
        budget.write_text(json.dumps(newer))
        recovery.restore(self.snapshot, self.target, checksum)
        self.assertEqual(json.loads(budget.read_text()), newer)
        self.assertEqual(json.loads((self.target / '0/budget.json').read_text()), old)
        receipt = json.loads((self.target / 'restore-receipt.json').read_text())
        self.assertEqual(receipt['state'], 'quarantined')
        for field in ('activation', 'budget_rollback', 'operation_replay'):
            self.assertEqual(receipt[field], 'forbidden')
        # No command accepts an operator/agent request to clear or reconcile holds.
        result = subprocess.run([sys.executable, str(HELPER), 'reconcile'], capture_output=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue((self.target / 'recovery-hold').exists())

    def test_whole_system_and_unintegrated_production_capture_fail_closed(self):
        self.plan['coverage'] = 'whole-system'
        with self.assertRaisesRegex(recovery.Refusal, 'board service'):
            self.capture()
        self.assertFalse(self.hold.exists())
        result = subprocess.run([sys.executable, str(HELPER), 'checkpoint', '--plan',
                                 str(self.root / 'absent-plan'), '--target', str(self.snapshot)],
                                capture_output=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b'NOT IMPLEMENTED', result.stderr)
        self.assertFalse(self.snapshot.exists())


if __name__ == '__main__':
    unittest.main()
