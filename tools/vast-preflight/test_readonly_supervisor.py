"""Local synthetic child programs only; never invoke real readonly runner/auth."""
import json
import os
import sys
import signal
import tempfile
import subprocess
from pathlib import Path
import time
import unittest
from unittest.mock import Mock, patch

from preflight import PreflightError
import readonly_supervisor as supervisor

SENTINEL = 'SYNTHETIC-SUPERVISOR-ONLY'
RESULT = dict(mode='readonly-unqualified', live_authorized=False, balance_micro_usd=6_000_000,
              balance_observed_s=100, auto_topup='unknown', pricing_qualification='blocked', offers=[])


class SupervisorTests(unittest.TestCase):
    def run_child(self, code, timeout=1):
        self.assertNotIn('VAST_API_KEY', os.environ)
        # Actual synthetic descendant closes stdout/stderr; must be cleaned even
        # on natural leader exit. Also leaves a live group member for Darwin's
        # sandbox (signaling a zombie-only group can return EPERM, fail-closed).
        prefix = ("import subprocess,sys; subprocess.Popen([sys.executable,'-c',"
                  "'import time;time.sleep(20)'],stdout=subprocess.DEVNULL,"
                  "stderr=subprocess.DEVNULL); ")
        return supervisor._run([sys.executable, '-E', '-s', '-B', '-c', prefix + code],
                               lambda: SENTINEL, timeout)

    def opaque(self, code, timeout=1):
        with self.assertRaises(PreflightError) as raised:
            self.run_child(code, timeout)
        self.assertIsNone(raised.exception.__context__)
        self.assertNotIn(SENTINEL, str(raised.exception))

    def test_success_whitelist(self):
        self.assertEqual(self.run_child('print(' + repr(json.dumps(RESULT)) + ')'), RESULT)

    def test_timeout_kills_group_reaps_direct_child_drops_partial_output(self):
        start = time.monotonic()
        # Parent and grandchild share supervisor-created session/process group.
        code = "import subprocess,sys,time; subprocess.Popen([sys.executable,'-c','import time; time.sleep(20)']); print('partial',flush=True); time.sleep(20)"
        captured = []
        original = supervisor.subprocess.Popen
        def spawn(*args, **kwargs):
            child = original(*args, **kwargs)
            captured.append(child)
            return child
        with patch.object(supervisor.subprocess, 'Popen', side_effect=spawn), \
                patch.object(supervisor.os, 'killpg', wraps=supervisor.os.killpg) as kill:
            self.opaque(code, 0.15)
            self.assertEqual(len(captured), 1)
            kill.assert_called_once_with(captured[0].pid, signal.SIGKILL)
        self.assertEqual(captured[0].returncode, -signal.SIGKILL)
        self.assertLess(time.monotonic() - start, 3)

    def test_output_limit_and_malformed_output_never_return_raw(self):
        for code in ("print('x'*40000)", "print('not-json')", "print('{}')", 'raise SystemExit(3)',
                     "import os; print(os.environ['VAST_API_KEY'])"):
            self.opaque(code)

    def test_stderr_discarded_and_parent_env_not_inherited(self):
        code = ("import os,sys; assert not any(k in os.environ for k in ('HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','SSL_CERT_FILE','SSL_CERT_DIR','PYTHONPATH','CONTAINER_API_KEY')); "
                "assert os.environ['VAST_API_KEY']=='SYNTHETIC-SUPERVISOR-ONLY'; "
                "print('secret error',file=sys.stderr); print(" + repr(json.dumps(RESULT)) + ")")
        self.assertEqual(self.run_child(code), RESULT)

    def test_no_implicit_authorization_and_fixed_runner_command(self):
        supplier = Mock(return_value=SENTINEL)
        with self.assertRaises(PreflightError):
            supervisor.run_readonly(supplier)
        supplier.assert_not_called()
        with patch.object(supervisor, '_run', return_value=RESULT) as run:
            self.assertEqual(supervisor.run_readonly(supplier, authorized=True), RESULT)
        command, injected, deadline = run.call_args.args
        self.assertEqual(command[:4], [sys.executable, '-E', '-s', '-B'])
        self.assertTrue(command[-2].endswith('/readonly_runner.py'))
        self.assertEqual(command[-1], '--authorized-readonly')
        self.assertNotIn(SENTINEL, command)
        self.assertIs(injected, supplier)
        self.assertEqual(deadline, 20)

    def test_secret_reflection_in_valid_json_rejected(self):
        altered = dict(RESULT, balance_micro_usd=SENTINEL)
        self.opaque('print(' + repr(json.dumps(altered)) + ')')

    def test_natural_exit_kills_stdout_closed_descendant_before_reap(self):
        # FIFO EOF is direct kernel evidence the surviving descendant closed its
        # writer after group kill, not a timing-only heartbeat or host PID scan.
        with tempfile.TemporaryDirectory() as directory:
            fifo = str(Path(directory) / 'child.fifo')
            ready = str(Path(directory) / 'ready')
            os.mkfifo(fifo, 0o600)
            reader = os.open(fifo, os.O_RDONLY | os.O_NONBLOCK)
            try:
                descendant = ("import os,time;fd=os.open(" + repr(fifo) +
                    ",os.O_WRONLY);os.write(fd,b'ready');open(" + repr(ready) +
                    ",'w').close();time.sleep(20)")
                leader = ("import subprocess,sys,time,os;subprocess.Popen([sys.executable,'-c'," +
                    repr(descendant) + "],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);"
                    "exec(" + repr("while not os.path.exists(" + repr(ready) + "): time.sleep(.001)") + ");"
                    "print(" + repr(json.dumps(RESULT)) + ")")
                events = []
                original_spawn = supervisor.subprocess.Popen
                original_kill = supervisor.os.killpg
                def spawn(*args, **kwargs):
                    child = original_spawn(*args, **kwargs)
                    wait = child.wait
                    def reap(*a, **kw):
                        self.assertIn('kill', events)
                        events.append('reap')
                        return wait(*a, **kw)
                    child.wait = reap
                    return child
                def kill(pid, sig):
                    self.assertNotIn('reap', events)
                    events.append('kill')
                    return original_kill(pid, sig)
                with patch.object(supervisor.subprocess, 'Popen', side_effect=spawn), \
                        patch.object(supervisor.os, 'killpg', side_effect=kill):
                    self.assertEqual(self.run_child(leader), RESULT)
                self.assertEqual(os.read(reader, 20), b'ready')
                # EOF may follow signal delivery a little after parent reap.
                limit = time.monotonic() + 1
                while True:
                    try:
                        chunk = os.read(reader, 20)
                    except BlockingIOError:
                        self.assertLess(time.monotonic(), limit)
                        time.sleep(.005)
                        continue
                    self.assertEqual(chunk, b'')
                    break
            finally:
                os.close(reader)

    def test_cleanup_failure_discards_valid_output(self):
        original = supervisor.os.killpg
        def unknown_outcome(pid, sig):
            original(pid, sig)  # clean test-created group but report uncertainty
            raise PermissionError('synthetic cleanup unknown')
        with patch.object(supervisor.os, 'killpg', side_effect=unknown_outcome):
            self.opaque('print(' + repr(json.dumps(RESULT)) + ')')

    def test_never_signal_own_process_group_or_already_reaped_pid(self):
        for pid, returncode in ((os.getpgrp(), None), (12345, 0)):
            child = Mock(pid=pid, returncode=returncode, stdout=None)
            child.wait.return_value = 0
            with patch.object(supervisor, '_observer', return_value=Mock()), \
                    patch.object(supervisor.subprocess, 'Popen', return_value=child), \
                    patch.object(supervisor.os, 'killpg') as kill:
                with self.assertRaises(PreflightError):
                    supervisor._run(['synthetic-no-exec'], lambda: SENTINEL, 1)
            kill.assert_not_called()

    def test_missing_native_waitid_fails_before_supplier_or_spawn(self):
        supplier = Mock(return_value=SENTINEL)
        with patch.object(supervisor.os, 'waitid', None), \
                patch.object(supervisor.subprocess, 'Popen') as spawn:
            with self.assertRaises(PreflightError):
                supervisor._run(['never-executed'], supplier, 1)
        supplier.assert_not_called()
        spawn.assert_not_called()

    def test_actual_system_python39_refuses_before_supplier_or_spawn(self):
        interpreter = '/usr/bin/python3'
        if sys.platform != 'darwin' or not os.path.exists(interpreter):
            self.skipTest('actual unsupported system Python3.9 check is Darwin-specific')
        code = """
import os,sys
sys.path.insert(0,sys.argv[1])
import readonly_supervisor as module
from preflight import PreflightError
assert sys.version_info[:2] == (3,9), 'system interpreter changed; review explicit refusal test'
assert not hasattr(os,'waitid')
calls=[]
def supplier():
    calls.append('secret')
    raise AssertionError('must not consume supplier')
def spawn(*a,**k):
    calls.append('spawn')
    raise AssertionError('must not spawn')
module.subprocess.Popen=spawn
try:
    module._run(['never-executed'],supplier,1)
except PreflightError:
    assert not calls
    print('unsupported-native-backend-refused-before-secret-or-spawn')
else:
    raise AssertionError('unsupported interpreter must refuse')
"""
        result = subprocess.run([interpreter, '-E', '-s', '-B', '-c', code,
                    str(Path(__file__).resolve().parent)], stdin=subprocess.DEVNULL,
                    capture_output=True, timeout=5, env={'PATH': os.defpath, 'LANG': 'C'})
        self.assertEqual(result.returncode, 0)
        self.assertEqual(result.stdout.strip(),
                         b'unsupported-native-backend-refused-before-secret-or-spawn')
        self.assertEqual(result.stderr, b'')

    def test_unsupported_observer_fails_before_supplier_or_spawn(self):
        supplier = Mock(return_value=SENTINEL)
        with patch.object(supervisor, '_observer', side_effect=PreflightError()), \
                patch.object(supervisor.subprocess, 'Popen') as spawn:
            with self.assertRaises(PreflightError):
                supervisor._run(['never-executed'], supplier, 1)
        supplier.assert_not_called()
        spawn.assert_not_called()

    def test_timeout_after_child_exits_with_descendant_holding_pipe(self):
        code = "import subprocess,sys; subprocess.Popen([sys.executable,'-c','import time; time.sleep(20)'])"
        start = time.monotonic()
        self.opaque(code, 0.15)
        self.assertLess(time.monotonic() - start, 3)


if __name__ == '__main__':
    unittest.main()
