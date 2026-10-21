"""Durable LOCAL simulation harness: accepts the exact in-memory FakeProvider only.

No create operation or network transport. Local process/host loss is NOT contained;
this is not an independently deployed remote/provider watchdog. Live gate is false.
"""
import dataclasses
import fcntl
import json
import os
import re
import stat

from preflight import (INITIAL_BUDGET, OwnedInstance, PreflightError, Snapshot,
                       WatchIntent, cleanup_step, decode, require, uint)


@dataclasses.dataclass(frozen=True)
class OwnedVolume:
    volume_id: int
    owner: str
    run_id: str


@dataclasses.dataclass(frozen=True)
class Inventory:
    observed_s: int
    complete: bool
    instances: tuple = ()
    volumes: tuple = ()
    volumes_complete: bool = False


class FakeProvider:
    """Deterministic in-memory effect seam, never accepts credentials or URLs."""
    def __init__(self, inventory):
        self.inventory = inventory
        self.destroyed_instances = []
        self.destroyed_volumes = []
        self.snapshot_fails = False
        self.destroy_fails = False

    def snapshot(self):
        if self.snapshot_fails:
            raise PreflightError()
        return self.inventory

    def destroy_instance(self, instance_id):
        if self.destroy_fails:
            raise PreflightError()
        self.destroyed_instances.append(instance_id)

    def destroy_volume(self, volume_id):
        if self.destroy_fails:
            raise PreflightError()
        self.destroyed_volumes.append(volume_id)


def component(value):
    require(type(value) is str and re.fullmatch('[A-Za-z0-9_-]{1,64}', value))


class LocalWatchdog:
    """One account-wide flock held for lifetime. Only local trusted directory.

    Atomic replace+file/directory fsync journal; no credentials in state. No row
    deletion: all reservations/outcomes remain. Errors latch future admission off.
    """
    live_authorized = False

    def __init__(self, directory, provider):
        require(type(provider) is FakeProvider)
        self.provider = provider
        self._directory = directory
        self._lock = None
        self._dirfd = None
        failed = False
        try:
            self._dirfd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            info = os.fstat(self._dirfd)
            require(info.st_uid == os.getuid() and stat.S_IMODE(info.st_mode) == 0o700)
            self._lock = os.open('owner.lock', os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW,
                                 0o600, dir_fd=self._dirfd)
            lock_info = os.fstat(self._lock)
            require(stat.S_ISREG(lock_info.st_mode) and lock_info.st_uid == os.getuid()
                    and stat.S_IMODE(lock_info.st_mode) == 0o600 and lock_info.st_nlink == 1)
            fcntl.flock(self._lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            try:
                fd = os.open('journal.json', os.O_RDONLY | os.O_NOFOLLOW, dir_fd=self._dirfd)
            except FileNotFoundError:
                # A previously existing lease without journal is unsafe, not a reset.
                require(os.fstat(self._lock).st_size == 0)
                self.state = {'version': 1, 'spent_micro_usd': 0, 'blocked': False, 'runs': []}
                self._save()
                os.write(self._lock, b'initialized\n')
                os.fsync(self._lock)
            else:
                with os.fdopen(fd, 'rb') as stream:
                    info = os.fstat(stream.fileno())
                    require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid()
                            and stat.S_IMODE(info.st_mode) == 0o600 and info.st_nlink == 1)
                    self.state = decode(stream.read(1_048_577))
                self._validate()
        except Exception:
            failed = True
        if failed:
            self.close()
            raise PreflightError()

    def __repr__(self):
        return '<LocalWatchdog fake-only live_authorized=False>'

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()

    def close(self):
        for name in ('_lock', '_dirfd'):
            fd = getattr(self, name, None)
            if fd is not None:
                os.close(fd)
                setattr(self, name, None)

    def _validate(self):
        require(set(self.state) == {'version', 'spent_micro_usd', 'blocked', 'runs'})
        require(type(self.state['version']) is int and self.state['version'] == 1
                and type(self.state['blocked']) is bool)
        uint(self.state['spent_micro_usd'])
        require(type(self.state['runs']) is list and len(self.state['runs']) <= 4096)
        seen = set()
        for row in self.state['runs']:
            require(set(row) == {'owner', 'run_id', 'deadline_s', 'cleanup_deadline_s',
                                'instance_id', 'launch_settled', 'reserve', 'phase',
                                'bound_volumes', 'status'})
            component(row['owner'])
            component(row['run_id'])
            require(row['run_id'] not in seen)
            seen.add(row['run_id'])
            uint(row['reserve'])
            require(row['reserve'] > 0)
            uint(row['deadline_s'])
            require(uint(row['cleanup_deadline_s']) > row['deadline_s'])
            require(row['instance_id'] is None or uint(row['instance_id']) > 0)
            require(type(row['launch_settled']) is bool)
            require(row['phase'] in ('prepared', 'submitted', 'bound', 'confirmed'))
            require(type(row['bound_volumes']) is list)
            require(all(uint(v) > 0 for v in row['bound_volumes']))
            require(len(set(row['bound_volumes'])) == len(row['bound_volumes']))
            require(type(row['status']) is str and re.fullmatch('[a-z-]{1,40}', row['status']))
            if row['phase'] == 'bound':
                require(row['instance_id'] is not None and row['launch_settled'] is True)
            if row['phase'] == 'submitted':
                require(row['instance_id'] is None and row['launch_settled'] is False)
            if row['phase'] == 'confirmed':
                require(row['launch_settled'] is True and row['status'] == 'confirmed-absent')
        require(sum(r['phase'] != 'confirmed' for r in self.state['runs']) <= 1)
        require(self.state['spent_micro_usd'] >= sum(
            r['reserve'] for r in self.state['runs'] if r['phase'] == 'confirmed'))
        if self.state['spent_micro_usd'] + sum(
                r['reserve'] for r in self.state['runs'] if r['phase'] != 'confirmed') > INITIAL_BUDGET:
            require(self.state['blocked'] is True)
        if self.state['spent_micro_usd'] > INITIAL_BUDGET:
            require(self.state['blocked'] is True)

    def _save(self):
        self._validate()
        failed = False
        fd = None
        try:
            # O_EXCL means crash-leftover temporary files block rather than overwrite.
            fd = os.open('journal.pending', os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                         0o600, dir_fd=self._dirfd)
            payload = json.dumps(self.state, sort_keys=True, separators=(',', ':')).encode()
            require(len(payload) <= 1_048_576)
            with os.fdopen(fd, 'wb') as stream:
                fd = None
                stream.write(payload)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace('journal.pending', 'journal.json', src_dir_fd=self._dirfd,
                       dst_dir_fd=self._dirfd)
            os.fsync(self._dirfd)
        except Exception:
            failed = True
        if fd is not None:
            os.close(fd)
        if failed:
            self.state['blocked'] = True
            raise PreflightError()

    def _active(self):
        require(self._lock is not None)
        rows = [r for r in self.state['runs'] if r['phase'] != 'confirmed']
        require(len(rows) == 1)
        return rows[0]

    def prepare(self, owner, run_id, deadline_s, cleanup_deadline_s, reserve_micro_usd):
        """Durable intent BEFORE hypothetical create; never creates anything."""
        require(self._lock is not None and self.state['blocked'] is False)
        component(owner)
        component(run_id)
        uint(deadline_s)
        require(uint(cleanup_deadline_s) > deadline_s)
        require(uint(reserve_micro_usd) >= 100_000)
        require(all(r['phase'] == 'confirmed' and r['run_id'] != run_id
                    for r in self.state['runs']))
        require(self.state['spent_micro_usd'] + reserve_micro_usd <= INITIAL_BUDGET)
        self.state['runs'].append(dict(owner=owner, run_id=run_id, deadline_s=deadline_s,
            cleanup_deadline_s=cleanup_deadline_s, instance_id=None, launch_settled=False,
            reserve=reserve_micro_usd, phase='prepared', bound_volumes=[], status='prepared'))
        self._save()

    def mark_submitted(self):
        """Persist BEFORE one hypothetical create; NO create retry method exists."""
        row = self._active()
        require(row['phase'] == 'prepared' and row['launch_settled'] is False)
        row['phase'] = 'submitted'
        row['status'] = 'launch-uncertain'
        self._save()

    def bind(self, instance_id, now_s):
        row = self._active()
        require(row['phase'] == 'submitted' and uint(instance_id) > 0)
        inventory = self.provider.snapshot()
        require(type(inventory) is Inventory and inventory.complete is True)
        require(type(inventory.observed_s) is int and 0 <= uint(now_s) - inventory.observed_s <= 30)
        matches = [r for r in inventory.instances
                   if (r.owner, r.run_id) == (row['owner'], row['run_id'])]
        require(len(matches) == 1 and matches[0].instance_id == instance_id)
        row.update(instance_id=instance_id, phase='bound', launch_settled=True, status='bound')
        self._save()

    def tick(self, now_s, abort=False):
        """One bounded fake-provider step; call repeatedly even after alerts/deadline.

        Caller clock must be trusted/injected; total process/network qualification
        is out of scope. Transient failure permanently latches admission blocked.
        """
        row = self._active()
        failed = False
        try:
            uint(now_s)
            inventory = self.provider.snapshot()
            require(type(inventory) is Inventory)
            intent = WatchIntent(row['owner'], row['run_id'], row['deadline_s'],
                                 row['cleanup_deadline_s'], row['instance_id'],
                                 row['launch_settled'])
            fresh = (type(inventory.observed_s) is int
                     and 0 <= now_s - inventory.observed_s <= 30)
            require(inventory.complete is True and inventory.volumes_complete is True and fresh)
            require(type(inventory.instances) is tuple and type(inventory.volumes) is tuple)
            require(all(type(r) is OwnedInstance for r in inventory.instances))
            ids = set()
            owned_volumes = []
            for volume in inventory.volumes:
                require(type(volume) is OwnedVolume and uint(volume.volume_id) > 0)
                require(volume.volume_id not in ids)
                ids.add(volume.volume_id)
                if (volume.owner, volume.run_id) == (row['owner'], row['run_id']):
                    owned_volumes.append(volume.volume_id)
                elif volume.volume_id in row['bound_volumes']:
                    raise PreflightError()
            # Bind discovered volumes durably BEFORE any destroy side effect.
            row['bound_volumes'] = sorted(set(row['bound_volumes']) | set(owned_volumes))
            if row['phase'] == 'prepared':
                # Proven no submission: no hypothetical effect was permitted.
                intent = dataclasses.replace(intent, launch_settled=True)
                row['launch_settled'] = True
            snapshot = Snapshot(inventory.observed_s, True, inventory.instances,
                                tuple(sorted(owned_volumes)))
            status, instances = cleanup_step(intent, snapshot, now_s, abort)
            if status == 'alert-owner-mismatch':
                raise PreflightError()
            row['status'] = status
            if status.startswith(('alert-', 'retry-')):
                self.state['blocked'] = True
            self._save()
            for instance_id in instances:
                self.provider.destroy_instance(instance_id)
            if abort or now_s >= row['deadline_s']:
                for volume_id in owned_volumes:
                    self.provider.destroy_volume(volume_id)
            if status == 'confirmed-absent':
                row['phase'] = 'confirmed'
                # Conservative accounting: never refund reserve on absent billing data.
                self.state['spent_micro_usd'] += row['reserve']
                self._save()
            return status
        except Exception:
            failed = True
        if failed:
            self.state['blocked'] = True
            row['status'] = 'alert-watchdog-failure'
            try:
                self._save()
            except Exception:
                pass  # Original on-disk active intent/reserve remains; no new admission.
            raise PreflightError()

    def poll(self, steps, clock, pause):
        """Bounded scheduling harness with injected time/sleep; fake effects only.

        Exhaustion is alert/block, never absence. Failures retain active journal
        and keep retrying within the supplied step bound. Caller restarts/polls
        again after exhaustion; no loop limit claims that billing has stopped.
        """
        require(type(steps) is int and 1 <= steps <= 4096)
        for _ in range(steps):
            try:
                status = self.tick(clock())
            except Exception:
                row = self._active()
                row['status'] = 'alert-watchdog-failure'
                self.state['blocked'] = True
                self._save()
                status = row['status']
            if status == 'confirmed-absent':
                return status
            failed = False
            try:
                pause()
            except Exception:
                failed = True
            if failed:
                row = self._active()
                row['status'] = 'alert-watchdog-failure'
                self.state['blocked'] = True
                self._save()
                return row['status']
        row = self._active()
        row['status'] = 'alert-poll-exhausted'
        self.state['blocked'] = True
        self._save()
        return row['status']
