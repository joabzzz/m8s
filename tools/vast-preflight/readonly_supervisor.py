"""Hard wall-clock supervisor for a future-authorized read-only child.

No execution on import/default CLI. Fixed child/argv in public run_readonly;
private command injection is solely for standalone local tests. No key argv/logs.
"""
import json
import os
from pathlib import Path
import selectors
import signal
import subprocess
import sys
import time

from preflight import PreflightError, require, uint

MAX_OUTPUT = 32_768
DEADLINE_S = 20


def _sanitize(payload):
    # Decode private child output and whitelist a strict schema; never echo raw bytes.
    from preflight import decode
    result = decode(payload)
    require(result.get('mode') == 'readonly-unqualified')
    require(result.get('live_authorized') is False and result.get('auto_topup') == 'unknown')
    require(result.get('pricing_qualification') == 'blocked')
    allowed = {'mode', 'live_authorized', 'balance_micro_usd', 'balance_observed_s',
               'auto_topup', 'pricing_qualification', 'offers'}
    require(set(result) == allowed)
    uint(result['balance_micro_usd'])
    uint(result['balance_observed_s'])
    require(type(result['offers']) is list and len(result['offers']) <= 10)
    for row in result['offers']:
        require(type(row) is dict and set(row) == {'offer_id', 'diagnostic_hourly_micro_usd',
            'inet_down_mbps', 'country', 'datacenter', 'observed_s', 'unit_resolution'})
        for field in ('offer_id', 'diagnostic_hourly_micro_usd', 'inet_down_mbps', 'observed_s'):
            uint(row[field])
        require(type(row['country']) is str and len(row['country']) == 2
                and row['country'].isascii() and row['country'].isalpha()
                and row['country'].isupper())
        require(type(row['datacenter']) is bool)
        require(row['unit_resolution'] == 'conflicting-unqualified')
    return result


def _observer():
    """Require native non-reaping waitid BEFORE secrets/spawn; no ABI fallback."""
    require(signal.getsignal(signal.SIGCHLD) == signal.SIG_DFL)
    require(callable(getattr(os, 'waitid', None)))
    for name in ('P_PID', 'WEXITED', 'WNOHANG', 'WNOWAIT', 'CLD_EXITED'):
        require(hasattr(os, name))
    flags = os.WEXITED | os.WNOHANG | os.WNOWAIT
    def observe(pid):
        info = os.waitid(os.P_PID, pid, flags)
        if info is None or info.si_pid == 0:
            return None
        require(info.si_pid == pid)
        return (info.si_code, info.si_status)
    return observe


def _exit_without_reaping(observe, pid, deadline):
    while True:
        require(time.monotonic() < deadline)
        result = observe(pid)
        if result is not None:
            return result
        time.sleep(min(0.005, max(0, deadline - time.monotonic())))


def _run(command, secret_supplier, timeout_s):
    """Trusted private seam. Exactly one child, process-group SIGKILL+direct reap.

    No stdout retained on timeout/error. stderr is discarded at OS boundary.
    timeout includes spawn+I/O+parse; OS scheduling/reap latency not hard-real-time.
    Caller must be exclusive reaper: no other thread/handler may wait for this child.
    """
    child = None
    failed = False
    output = bytearray()
    deadline = time.monotonic() + timeout_s
    try:
        require(type(timeout_s) in (int, float) and 0 < timeout_s <= DEADLINE_S)
        observe = _observer()
        secret = secret_supplier()
        require(type(secret) is str and 1 <= len(secret) <= 4096)
        require(all(33 <= ord(c) <= 126 for c in secret))
        # Fresh narrow environment: do not copy proxies, debug/trust overrides,
        # PYTHONPATH, unrelated secrets or daemon capabilities from parent.
        environment = {'PATH': os.defpath, 'LANG': 'C', 'VAST_API_KEY': secret}
        require(time.monotonic() < deadline)
        child = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                 stderr=subprocess.DEVNULL, env=environment,
                                 start_new_session=True, close_fds=True)
        os.set_blocking(child.stdout.fileno(), False)
        with selectors.DefaultSelector() as selector:
            selector.register(child.stdout, selectors.EVENT_READ)
            while True:
                remaining = deadline - time.monotonic()
                require(remaining > 0)
                ready = selector.select(remaining)
                require(bool(ready))
                chunk = os.read(child.stdout.fileno(), min(4096, MAX_OUTPUT + 1 - len(output)))
                if not chunk:
                    break
                output.extend(chunk)
                require(len(output) <= MAX_OUTPUT)
        # Sole reaper: WNOWAIT observes exit but pins leader PID until cleanup.
        exit_code, exit_status = _exit_without_reaping(observe, child.pid, deadline)
        require(exit_code == os.CLD_EXITED and exit_status == 0)
        require(time.monotonic() < deadline)
        # Defense against a malfunctioning child reflecting its injected key.
        require(secret.encode() not in output)
        result = _sanitize(bytes(output))
        require(time.monotonic() < deadline)
    except Exception:
        failed = True
    finally:
        if child is not None:
            # ALWAYS clean the owned group before direct reap, even natural exit.
            # Never signal after reap or on lost ownership / unknown observer.
            try:
                require(type(child.pid) is int and child.pid > 1)
                require(child.pid not in (os.getpid(), os.getpgrp()))
                require(child.returncode is None)
                ownership = observe(child.pid)  # ECHILD/errors => no signal, fail closed
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    # Pinned, exited leader + absent group is definitive no members.
                    require(ownership is not None)
            except Exception:
                failed = True
            try:
                child.wait(timeout=2)
            except Exception:
                failed = True
            if child.stdout is not None:
                child.stdout.close()
        output.clear()
    if failed:
        raise PreflightError()
    return result


def run_readonly(secret_supplier, *, authorized=False):
    """Root authorization REQUIRED. Supplier called only after explicit gate.

    No default environment supplier: root must explicitly inject runtime function
    reading VAST_API_KEY once credential scope is established. Never call from tests
    except with a mocked _run. This local supervisor cannot survive host loss.
    """
    require(authorized is True)
    script = str(Path(__file__).resolve().with_name('readonly_runner.py'))
    return _run([sys.executable, '-E', '-s', '-B', script, '--authorized-readonly'],
                secret_supplier, DEADLINE_S)


if __name__ == '__main__':
    print(json.dumps({'mode': 'offline', 'live_authorized': False}))
