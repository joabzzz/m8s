"""Synthetic fixtures only. No HTTP, environment, secret files or credentials."""
import dataclasses
import json
import os
import pickle
import unittest
from pathlib import Path

from preflight import (AccountFacts, Gates, OwnedInstance, Plan, PreflightError,
                       ReadOnlyClient, Snapshot, UnitEvidence, WatchIntent, cleanup_step,
                       decode, dry_run, offer_facts)

SYNTHETIC_UNITS = UnitEvidence('synthetic-only', 'fixture-v1', 'Mbps',
                               'USD/GB/hour', 'USD/GB', True)

FIXTURE = Path(__file__).parent / 'fixtures' / 'readonly.json'


class PreflightTests(unittest.TestCase):
    def setUp(self):
        # Names-only check: fail before any fixture client runs if invoked unsafely.
        self.assertNotIn('VAST_API_KEY', os.environ)
        self.assertNotIn('CONTAINER_API_KEY', os.environ)
        self.fixture = json.loads(FIXTURE.read_text())
        self.calls = []

        def exchange(method, path, headers, body):
            self.assertEqual(headers['Authorization'], 'Bearer SYNTHETIC-NOT-A-KEY')
            self.calls.append((method, path, body))
            result = self.fixture['account' if method == 'GET' else 'offers']
            return 200, json.dumps(result).encode()

        self.client = ReadOnlyClient(exchange, lambda: 'SYNTHETIC-NOT-A-KEY', SYNTHETIC_UNITS)
        self.account = self.client.account(100)
        self.offer = self.client.offers(100)[0]
        self.plan = Plan(600, 120, 20_000_000_000, 10_000_000_000,
                         1_000_000_000, 100_000)
        self.gates = Gates(100, **{name: True for name in Gates.__dataclass_fields__
                                  if name not in ('verified_s', 'market')})

    def test_sanitized_whitelist(self):
        encoded = repr((self.account, self.offer, self.client))
        self.assertNotIn('DO-NOT-RETURN', encoded)
        self.assertNotIn('SYNTHETIC-NOT-A-KEY', encoded)
        self.assertEqual(self.account.balance_micro_usd, 6_000_000)
        self.assertEqual(self.account.auto_topup, 'unknown')
        self.assertEqual(self.offer.inet_down_mbps, 1250)
        self.assertEqual([(c[0], c[1]) for c in self.calls], [
            ('GET', '/api/v0/users/current'), ('POST', '/api/v0/bundles')])
        self.assertEqual(json.loads(self.calls[1][2])['type'], 'on-demand')
        with self.assertRaises(PreflightError):
            pickle.dumps(self.client)

    def test_raw_failure_never_reaches_error_or_chain(self):
        def fail(*_):
            raise RuntimeError('SYNTHETIC-SECRET-REFLECTED')
        for exchange in (fail, lambda *_: (401, b'SYNTHETIC-SECRET-REFLECTED'),
                         lambda *_: (200, b'{"balance":"SYNTHETIC-SECRET-REFLECTED"}')):
            client = ReadOnlyClient(exchange, lambda: 'SYNTHETIC-NOT-A-KEY', SYNTHETIC_UNITS)
            try:
                client.account(100)
                self.fail('expected opaque failure')
            except PreflightError as error:
                self.assertEqual(str(error), 'preflight unavailable')
                self.assertIsNone(error.__context__)
                self.assertIsNone(error.__cause__)

    def test_invalid_secret_is_not_sent(self):
        for value in ('', 'a\nb', 'a\rb', 'a b', 123):
            client = ReadOnlyClient(lambda *_: self.fail('must not send'), lambda: value)
            with self.assertRaises(PreflightError):
                client.account(100)

    def test_strict_json(self):
        for body in (b'{"balance":1,"balance":6}', b'{"balance":NaN}',
                     b'{"success":false}', b'{"error":"reflected"}',
                     b'[]', b'x' * 1_048_577):
            with self.subTest(body=body[:40]), self.assertRaises(PreflightError):
                decode(body)

    def test_offer_policy_and_units(self):
        row = self.fixture['offers']['offers'][0]
        for key, value in [('geolocation', 'CN'), ('geolocation', 'UA'), ('geolocation', ''),
                           ('geolocation', None), ('geolocation', 'bad secret'),
                           ('inet_down', 999), ('storage_cost', '-1'),
                           ('dph_total', 'NaN'), ('dph_total', True),
                           ('rentable', False), ('verification', 'unverified'),
                           ('rented', True), ('datacenter', 'true')]:
            with self.subTest(key=key, value=value), self.assertRaises(PreflightError):
                offer_facts(dict(row, **{key: value}), 100, SYNTHETIC_UNITS)
        del row['storage_cost']
        with self.assertRaises(PreflightError):
            self.client.offers(100)

    def test_cost_includes_storage_transfer_cleanup_and_reserve(self):
        result = dry_run(self.account, self.offer, self.plan, self.gates, 100)
        # 0.20 compute + 0.004 storage + 0.01 ingress + 0.002 egress + 0.10 reserve
        self.assertEqual(result.estimated_micro_usd, 316_000)
        self.assertEqual((result.deadline_s, result.cleanup_deadline_s), (700, 820))
        self.assertFalse(result.live_authorized)
        exact = dataclasses.replace(self.plan, spent_micro_usd=1_684_000)
        dry_run(self.account, self.offer, exact, self.gates, 100)
        with self.assertRaises(PreflightError):
            dry_run(self.account, self.offer,
                    dataclasses.replace(exact, spent_micro_usd=1_684_001), self.gates, 100)

    def test_official_conflict_is_hard_no_launch_even_with_all_gates_true(self):
        row = self.fixture['offers']['offers'][0]
        diagnostic = offer_facts(row, 100)
        self.assertEqual(diagnostic.inet_down_mbps, 10007)
        self.assertEqual(diagnostic.raw_units.evidence.bandwidth, 'MB/s')
        self.assertFalse(diagnostic.raw_units.evidence.resolved)
        with self.assertRaises(PreflightError):
            dry_run(self.account, diagnostic, self.plan, self.gates, 100)

    def test_explicit_source_bandwidth_conversion_before_floor(self):
        row = dict(self.fixture['offers']['offers'][0], inet_down='125')
        mb_units = dataclasses.replace(SYNTHETIC_UNITS, bandwidth='MB/s')
        self.assertEqual(offer_facts(row, 100, mb_units).inet_down_mbps, 1000)
        with self.assertRaises(PreflightError):
            offer_facts(row, 100, SYNTHETIC_UNITS)
        with self.assertRaises(PreflightError):
            offer_facts(row, 100, dataclasses.replace(mb_units, bandwidth='unknown'))

    def test_every_evidence_gate_is_required(self):
        for field in Gates.__dataclass_fields__:
            if field in ('verified_s', 'market'):
                continue
            for value in (False, None, 1, 'true'):
                with self.subTest(field=field), self.assertRaises(PreflightError):
                    dry_run(self.account, self.offer, self.plan,
                            dataclasses.replace(self.gates, **{field: value}), 100)

    def test_budget_balance_freshness_and_market_fail_closed(self):
        bad_plans = [dict(budget_micro_usd=2_000_001), dict(budget_micro_usd=0),
                     dict(run_s=0), dict(run_s=3601), dict(run_s=True),
                     dict(cleanup_grace_s=0), dict(reserve_micro_usd=0),
                     dict(disk_bytes=-1), dict(max_hourly_micro_usd=999_999),
                     dict(ingress_bytes=10**18), dict(spent_micro_usd=2_000_000)]
        for changes in bad_plans:
            with self.subTest(changes=changes), self.assertRaises(PreflightError):
                dry_run(self.account, self.offer, dataclasses.replace(self.plan, **changes),
                        self.gates, 100)
        for now in (99, 161, True):
            with self.assertRaises(PreflightError):
                dry_run(self.account, self.offer, self.plan, self.gates, now)
        with self.assertRaises(PreflightError):
            dry_run(AccountFacts(1, 100), self.offer, self.plan, self.gates, 100)
        for market in ('vast-secure', 'untrusted'):
            with self.assertRaises(PreflightError):
                dry_run(self.account, self.offer, self.plan,
                        dataclasses.replace(self.gates, market=market), 100)

    def test_round_up_cost_round_down_balance(self):
        self.fixture['account']['balance'] = '0.0000019'
        self.assertEqual(self.client.account(100).balance_micro_usd, 1)
        row = self.fixture['offers']['offers'][0]
        row['dph_total'] = '0.0000011'
        self.assertEqual(offer_facts(row, 100, SYNTHETIC_UNITS).hourly_micro_usd, 2)


class WatcherTests(unittest.TestCase):
    def setUp(self):
        self.intent = WatchIntent('owner', 'run1', 700, 820, 123, True)
        self.row = OwnedInstance(123, 'owner', 'run1', 'running')

    def test_deadline_and_abort_destroy_exact_owner(self):
        other = OwnedInstance(124, 'owner-other', 'run1', 'running')
        snapshot = Snapshot(700, True, (self.row, other), ())
        self.assertEqual(cleanup_step(self.intent, snapshot, 700), ('destroy', (123,)))
        self.assertEqual(cleanup_step(self.intent, snapshot, 699), ('retry-unknown', ()))
        snapshot = dataclasses.replace(snapshot, observed_s=699)
        self.assertEqual(cleanup_step(self.intent, snapshot, 699), ('wait', ()))
        self.assertEqual(cleanup_step(self.intent, snapshot, 699, True), ('destroy', (123,)))

    def test_stopped_not_destroyed_and_retries_do_not_give_up(self):
        row = dataclasses.replace(self.row, state='stopped')
        snapshot = Snapshot(900, True, (row,), ())
        self.assertEqual(cleanup_step(self.intent, snapshot, 900), ('alert-destroy', (123,)))

    def test_unknown_is_not_absence(self):
        for snapshot in (None, Snapshot(900, False, (), ()), Snapshot(800, True, (), ()),
                         Snapshot(901, True, (), ())):
            self.assertEqual(cleanup_step(self.intent, snapshot, 900), ('alert-unknown', ()))
        for volumes in (None, (456,)):
            self.assertEqual(cleanup_step(self.intent, Snapshot(900, True, (), volumes), 900),
                             ('alert-storage', ()))
        self.assertEqual(cleanup_step(self.intent, Snapshot(900, True, (), ()), 900),
                         ('confirmed-absent', ()))

    def test_ambiguous_create_recovered_by_exact_run_not_new_launch(self):
        intent = dataclasses.replace(self.intent, instance_id=None)
        snapshot = Snapshot(700, True, (self.row, OwnedInstance(125, 'owner', 'run1', 'loading')))
        self.assertEqual(cleanup_step(intent, snapshot, 700), ('destroy', (123, 125)))

    def test_empty_snapshot_cannot_resolve_inflight_create(self):
        intent = dataclasses.replace(self.intent, instance_id=None, launch_settled=False)
        self.assertEqual(cleanup_step(intent, Snapshot(900, True, (), ()), 900),
                         ('alert-launch-uncertain', ()))

    def test_forged_negative_rate_is_not_a_discount(self):
        # Public normalized fact objects are still validated at cost-gate boundary.
        fixture = json.loads(FIXTURE.read_text())['offers']['offers'][0]
        offer = dataclasses.replace(offer_facts(fixture, 100, SYNTHETIC_UNITS), hourly_micro_usd=-1)
        gates = Gates(100, **{n: True for n in Gates.__dataclass_fields__
                             if n not in ('verified_s', 'market')})
        with self.assertRaises(PreflightError):
            dry_run(AccountFacts(6_000_000, 100), offer,
                    Plan(600, 120, 10**9, 0, 0, 100_000), gates, 100)

    def test_id_reuse_or_duplicate_snapshot_alerts(self):
        other = dataclasses.replace(self.row, owner='other')
        self.assertEqual(cleanup_step(self.intent, Snapshot(700, True, (other,), ()), 700),
                         ('alert-owner-mismatch', ()))
        with self.assertRaises(PreflightError):
            cleanup_step(self.intent, Snapshot(700, True, (self.row, self.row), ()), 700)


if __name__ == '__main__':
    unittest.main()
