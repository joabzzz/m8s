"""Offline-first Vast preflight. No launch/destroy implementation or live CLI.

Transport and runtime secret supplier must be injected by a separately authorized
runner. Raw replies, headers and exceptions are never part of public results.
"""
import json
import re
from dataclasses import dataclass
from decimal import Decimal, ROUND_CEILING, ROUND_FLOOR

MAX_BODY = 1_048_576
INITIAL_BUDGET = 2_000_000  # micro-USD, aggregate envelope, NOT account credit
DENIED = frozenset(('CU', 'IR', 'KP', 'SY', 'RU', 'BY', 'CN', 'UA'))
# UA is conservatively excluded here: country-only facts cannot exclude occupied regions.


class PreflightError(Exception):
    def __init__(self):
        super().__init__('preflight unavailable')


def require(condition):
    if not condition:
        raise PreflightError()


def uint(value, maximum=10**18):
    require(type(value) is int and 0 <= value <= maximum)
    return value


def number(value):
    require(type(value) in (int, str, Decimal))
    require(len(str(value)) <= 64)
    try:
        result = Decimal(value)
        require(result.is_finite() and 0 <= result <= 10**15)
        return result
    except Exception:
        raise PreflightError() from None


def micros(value, rounding=ROUND_CEILING):
    return int((number(value) * 1_000_000).to_integral_value(rounding=rounding))


def unique(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result)
        result[key] = value
    return result


def decode(body):
    try:
        require(type(body) is bytes and len(body) <= MAX_BODY)
        result = json.loads(body, parse_float=Decimal, object_pairs_hook=unique,
                            parse_constant=lambda _: require(False))
        require(type(result) is dict)
        require('error' not in result and 'errors' not in result)
        require('success' not in result or result['success'] is True)
        return result
    except Exception:
        raise PreflightError() from None


@dataclass(frozen=True)
class AccountFacts:
    balance_micro_usd: int
    observed_s: int
    # show-user contract documents balance, NOT authoritative autobilling state.
    auto_topup: str = 'unknown'


@dataclass(frozen=True)
class UnitEvidence:
    source: str
    version: str
    bandwidth: str
    storage: str
    transfer: str
    resolved: bool = False


# Retrieval date is NOT a provider schema version; official pages are unversioned.
OFFICIAL_UNITS = UnitEvidence(
    'https://docs.vast.ai/api-reference/search/search-offers',
    'unversioned; retrieved 2026-10-21; conflicts with billing guide',
    'MB/s', 'USD/GB/month', 'USD/GB', False)


@dataclass(frozen=True)
class RawOfferUnits:
    inet_down: Decimal
    storage_cost: Decimal
    inet_down_cost: Decimal
    inet_up_cost: Decimal
    evidence: UnitEvidence


@dataclass(frozen=True)
class OfferFacts:
    offer_id: int
    hourly_micro_usd: int
    storage_micro_usd_per_gb_hour: int
    ingress_micro_usd_per_gb: int
    egress_micro_usd_per_gb: int
    inet_down_mbps: int
    country: str
    datacenter: bool
    observed_s: int
    raw_units: RawOfferUnits


def account_facts(raw, now_s):
    uint(now_s)
    return AccountFacts(micros(raw['balance'], ROUND_FLOOR), now_s)


def offer_facts(raw, now_s, units=OFFICIAL_UNITS):
    """Whitelist numbers/enums only; never echo provider strings/URLs/keys.

    Unresolved official rates yield DIAGNOSTIC upper bounds only. Raw numeric
    values and explicit source units/provenance remain separate from bounds.
    No unresolved/conflicting source can pass dry_run, even with all Gates true.
    """
    uint(now_s)
    require(raw['rentable'] is True and raw['rented'] is False)
    require(raw['verification'] == 'verified' and type(raw['datacenter']) is bool)
    country = raw['geolocation']
    require(type(country) is str and re.fullmatch('[A-Z]{2}', country) is not None)
    require(country not in DENIED)
    require(units.bandwidth in ('Mbps', 'MB/s'))
    require(units.storage in ('USD/GB/hour', 'USD/GB/month'))
    require(units.transfer in ('USD/GB', 'USD/TB'))
    source_down = number(raw['inet_down'])
    # Explicit source-unit conversion BEFORE policy threshold comparison.
    bandwidth = int((source_down * (8 if units.bandwidth == 'MB/s' else 1))
                    .to_integral_value(rounding=ROUND_FLOOR))
    provenance = RawOfferUnits(source_down, number(raw['storage_cost']),
                               number(raw['inet_down_cost']), number(raw['inet_up_cost']), units)
    # Month length is not established; no implicit monthly->hourly qualification.
    # Diagnostic upper bound treats raw monthly number as hourly; never admissible.
    require(units.resolved is not True or units.storage == 'USD/GB/hour')
    transfer_divisor = 1000 if units.resolved is True and units.transfer == 'USD/TB' else 1
    require(bandwidth >= 1000)
    offer_id = uint(raw['id'])
    require(offer_id > 0)
    return OfferFacts(offer_id, micros(raw['dph_total']), micros(raw['storage_cost']),
                      micros(provenance.inet_down_cost / transfer_divisor),
                      micros(provenance.inet_up_cost / transfer_divisor),
                      bandwidth, country, raw['datacenter'], now_s, provenance)


class ReadOnlyClient:
    """No default transport and no environment/file access.

    exchange(method, path, headers, body) -> (HTTP status, bytes). Injected exchange
    is a TRUSTED secret-handling boundary, must not log inputs or follow redirects.
    secret_supplier is called only at runtime and must never be an argv supplier.
    No repr, serialize, public header accessor or raw-response API.
    """
    __slots__ = ('__exchange', '__secret_supplier', '__units')

    def __init__(self, exchange, secret_supplier, units=OFFICIAL_UNITS):
        self.__units = units
        self.__exchange = exchange
        self.__secret_supplier = secret_supplier

    def __repr__(self):
        return '<ReadOnlyClient redacted>'

    def __getstate__(self):
        raise PreflightError()

    def _read(self, operation):
        # Fixed routes/payloads; cannot turn into a create, payment, key-list, or delete.
        routes = {
            'account': ('GET', '/api/v0/users/current', None),
            'offers': ('POST', '/api/v0/bundles', json.dumps({
                'limit': 10, 'type': 'on-demand', 'verified': {'eq': True},
                'rentable': {'eq': True}, 'rented': {'eq': False}}).encode()),
        }
        failed = False
        try:
            method, path, body = routes[operation]
            secret = self.__secret_supplier()
            require(type(secret) is str and 1 <= len(secret) <= 4096)
            require(all(33 <= ord(char) <= 126 for char in secret))
            status, reply = self.__exchange(method, path, {
                'Authorization': 'Bearer ' + secret,
                'Content-Type': 'application/json'}, body)
            require(type(status) is int and status == 200)
            result = decode(reply)
        except Exception:
            failed = True
        # Raise OUTSIDE handler: do not retain raw transport exceptions as context.
        if failed:
            raise PreflightError()
        return result

    def account(self, now_s):
        failed = False
        try:
            result = account_facts(self._read('account'), now_s)
        except Exception:
            failed = True
        if failed:
            raise PreflightError()
        return result

    def offers(self, now_s):
        failed = False
        try:
            raw = self._read('offers')
            require(type(raw['offers']) is list and len(raw['offers']) <= 10)
            result = tuple(offer_facts(row, now_s, self.__units) for row in raw['offers'])
            require(len({row.offer_id for row in result}) == len(result))
        except Exception:
            failed = True
        if failed:
            raise PreflightError()
        return result


@dataclass(frozen=True)
class Plan:
    run_s: int
    cleanup_grace_s: int
    disk_bytes: int
    ingress_bytes: int
    egress_bytes: int
    reserve_micro_usd: int
    spent_micro_usd: int = 0
    budget_micro_usd: int = INITIAL_BUDGET
    max_hourly_micro_usd: int = 1_000_000


@dataclass(frozen=True)
class Gates:
    # Inputs are evidence assertions from the designated owner, NOT autodetection.
    verified_s: int
    auto_topup_off: bool = False
    negative_balance_autocharge_excluded: bool = False
    pricing_units_verified: bool = False
    signed_image_verified: bool = False
    secure_admission_verified: bool = False
    scoped_self_destroy_verified: bool = False
    independent_watcher_armed: bool = False
    exclusive_owner_lock: bool = False
    no_existing_billable_resources: bool = False
    transfer_bounds_enforced: bool = False
    market: str = 'vast-community'


@dataclass(frozen=True)
class DryRun:
    estimated_micro_usd: int
    deadline_s: int
    cleanup_deadline_s: int
    # A passing simulation is never authority to launch or a claim of Ready.
    live_authorized: bool = False


def ceildiv(n, d):
    return (n + d - 1) // d


def dry_run(account, offer, plan, gates, now_s):
    for value in (now_s, account.observed_s, offer.observed_s, gates.verified_s):
        uint(value)
        require(0 <= now_s - value <= 60)
    for field in plan.__dataclass_fields__:
        uint(getattr(plan, field))
    require(0 < plan.run_s <= 3600 and 0 < plan.cleanup_grace_s <= 600)
    require(plan.disk_bytes > 0 and plan.reserve_micro_usd >= 100_000)
    require(0 < plan.budget_micro_usd <= INITIAL_BUDGET)
    for name in Gates.__dataclass_fields__:
        if name not in ('verified_s', 'market'):
            require(getattr(gates, name) is True)
    require(gates.market in ('vast-community', 'vast-secure'))
    require(gates.market != 'vast-secure' or offer.datacenter is True)
    # Cannot override a conflicted/unknown source by toggling the evidence gate.
    require(offer.raw_units.evidence.resolved is True)
    uint(account.balance_micro_usd)
    for name in ('offer_id', 'hourly_micro_usd', 'storage_micro_usd_per_gb_hour',
                 'ingress_micro_usd_per_gb', 'egress_micro_usd_per_gb', 'inet_down_mbps'):
        uint(getattr(offer, name))
    require(offer.offer_id > 0 and offer.hourly_micro_usd > 0)
    require(type(offer.country) is str and re.fullmatch('[A-Z]{2}', offer.country))
    require(offer.country not in DENIED and offer.inet_down_mbps >= 1000)
    require(type(offer.datacenter) is bool)
    require(offer.hourly_micro_usd <= plan.max_hourly_micro_usd)
    # Compute AND disk accrue throughout grace, even if destroy is slow/stopped.
    seconds = plan.run_s + plan.cleanup_grace_s
    compute = ceildiv(offer.hourly_micro_usd * seconds, 3600)
    # Decimal GB assumed only after explicit pricing_units_verified evidence.
    storage = ceildiv(offer.storage_micro_usd_per_gb_hour * plan.disk_bytes * seconds,
                      1_000_000_000 * 3600)
    ingress = ceildiv(offer.ingress_micro_usd_per_gb * plan.ingress_bytes, 10**9)
    egress = ceildiv(offer.egress_micro_usd_per_gb * plan.egress_bytes, 10**9)
    estimate = compute + storage + ingress + egress + plan.reserve_micro_usd
    require(plan.spent_micro_usd + estimate <= plan.budget_micro_usd)
    require(estimate <= account.balance_micro_usd)
    return DryRun(estimate, now_s + plan.run_s, now_s + seconds)


@dataclass(frozen=True)
class OwnedInstance:
    instance_id: int
    owner: str
    run_id: str
    state: str  # stopped remains present and billable


@dataclass(frozen=True)
class Snapshot:
    observed_s: int
    complete: bool
    instances: tuple
    # IDs of residual volumes explicitly bound to this run; None = unknown.
    residual_volume_ids: object = None


@dataclass(frozen=True)
class WatchIntent:
    owner: str
    run_id: str
    deadline_s: int
    cleanup_deadline_s: int
    # None covers crash/ambiguous create before ID binding. Never launch again.
    instance_id: object = None
    # True only after no create can still materialize (ID bound or reconciled).
    launch_settled: bool = False


def cleanup_step(intent, snapshot, now_s, abort=False):
    """Pure watcher decision, no effects: (status, exact instance IDs to destroy).

    Caller persists intent before create and outcomes after every operation. Only
    explicit complete fresh provider snapshots can conclude absence. HTTP errors,
    partial lists, timeouts and destroy acknowledgements cannot delete a row.
    """
    for value in (now_s, intent.deadline_s, intent.cleanup_deadline_s):
        uint(value)
    require(intent.deadline_s < intent.cleanup_deadline_s)
    for text in (intent.owner, intent.run_id):
        require(type(text) is str and re.fullmatch('[A-Za-z0-9_-]{1,64}', text))
    if intent.instance_id is not None:
        require(uint(intent.instance_id) > 0)
    overdue = now_s >= intent.cleanup_deadline_s
    if (snapshot is None or snapshot.complete is not True
            or type(snapshot.observed_s) is not int
            or not 0 <= now_s - snapshot.observed_s <= 30):
        return ('alert-unknown' if overdue else 'retry-unknown', ())
    ids = set()
    owned = []
    for row in snapshot.instances:
        require(uint(row.instance_id) > 0 and row.instance_id not in ids)
        ids.add(row.instance_id)
        if row.owner == intent.owner and row.run_id == intent.run_id:
            owned.append(row.instance_id)
        elif row.instance_id == intent.instance_id:
            return ('alert-owner-mismatch', ())
    if owned:
        if abort or now_s >= intent.deadline_s:
            return ('alert-destroy' if overdue else 'destroy', tuple(sorted(owned)))
        return ('wait', ())
    if intent.launch_settled is not True:
        return ('alert-launch-uncertain' if overdue else 'retry-launch-uncertain', ())
    if snapshot.residual_volume_ids != ():
        return ('alert-storage' if overdue else 'retry-storage', ())
    return ('confirmed-absent', ())
