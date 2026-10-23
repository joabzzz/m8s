"""Offline valid-frame/agent input qualification, no mesh/provider calls.

Run only in a disposable build stage of Dockerfile.vast with --network=none.
Generated SSH keys and the instance fixture are synthetic and remain in that
discarded stage; never copy its filesystem into the distributed image.
"""
import os
import json
import struct
from pathlib import Path
import select
import signal
import subprocess
import tempfile


def check(value):
    if not value:
        raise RuntimeError('synthetic provisioning qualification failed')


def main():
    check(os.geteuid() == 0 and not Path('/var/lib/m8s').exists())
    with tempfile.TemporaryDirectory(prefix='m8s-ssh-fixture-') as temp:
        root = Path(temp)
        env = {'PATH': '/usr/bin:/bin'}
        for name in ['client', 'wrong-host']:
            subprocess.run(['/usr/bin/ssh-keygen', '-q', '-t', 'ed25519', '-N', '',
                            '-f', str(root/name)], check=True, env=env,
                           stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, timeout=10)
        key = ' '.join((root/'client.pub').read_text().split()[:2])
        server = subprocess.Popen(['/usr/bin/python3', '/usr/local/lib/m8s/vast_provision.py', 'serve'],
            env=dict(env, M8S_PROVISION_SSH_KEY=key, CONTAINER_ID='42',
                     CONTAINER_API_KEY='synthetic-not-a-provider-credential'),
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            start_new_session=True)
        try:
            check(select.select([server.stdout], [], [], 10)[0])
            line = server.stdout.readline(1024).decode('ascii').strip()
            prefix = 'M8S_PROVISION_HOST_KEY_V1 '
            check(line.startswith(prefix))
            host = line[len(prefix):]
            known = root/'known_hosts'
            args = ['/usr/bin/ssh', '-F', '/dev/null', '-T', '-p', '2222',
                    '-i', str(root/'client'), '-o', 'BatchMode=yes',
                    '-o', 'IdentitiesOnly=yes', '-o', 'StrictHostKeyChecking=yes',
                    '-o', 'UserKnownHostsFile='+str(known),
                    '-o', 'GlobalKnownHostsFile=/dev/null', '-o', 'ConnectTimeout=5',
                    '-o', 'ConnectionAttempts=1', 'm8s@127.0.0.1', 'provision-v1']
            # Wait for listener, with a fixed bound. Never retry provisioning.
            import socket, time
            until = time.monotonic()+5
            while True:
                if server.poll() is not None:
                    # Only synthetic keys/credential exist in this disposable
                    # offline stage. The production entrypoint remains opaque.
                    raise RuntimeError('synthetic sshd exited: '+server.stderr.read(4096).decode(errors='replace'))
                try:
                    with socket.create_connection(('127.0.0.1',2222), timeout=0.2):
                        break
                except OSError:
                    check(time.monotonic()<until)
                    time.sleep(0.05)
            known.write_text('[127.0.0.1]:2222 '+host+'\n')
            # Public RFC 8032 fixture, not an owner credential.
            public = 'd75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a'
            config = dict(schema='m8s.vast-node/v1', instance_id=42,
                container_id='42', image_sha256='11'*32,
                owner_endpoint=public, owner_signing_key=public,
                mesh_owner_id='22'*32, owner_address='127.0.0.1:1',
                bind_address='0.0.0.0:40100', trust=dict(version=1, policy='off'),
                program='/opt/mesh/mesh-llm',
                gguf='/models/qwen2.5-0.5b-instruct-q4_k_m.gguf',
                material_root='/var/lib/m8s/material',
                model='qwen2.5-0.5b-instruct-q4_k_m',
                heartbeat_ttl_seconds=30, max_lifetime_seconds=600,
                port=9337, console_port=3131)
            raw=json.dumps(config).encode('ascii')
            frame=b'M8S-PROVISION-V1\n'+struct.pack('>I',len(raw))+raw+b'N'*32+b'11'*32
            result=subprocess.run(args,input=frame,capture_output=True,timeout=20,env=env)
            check(result.returncode != 0)
            check(result.stdout == b'M8S_NODE_CONFIG_V1\nM8S_NODE_IDENTITY_V1\n')
            check(b'secure Vast node refused' in result.stderr)
            check(not Path('/var/lib/m8s/material').exists())
            print('PASS: pinned SSH valid frame reaches real agent config/identity validation; offline synthetic credential refuses before listener or model startup')

        finally:
            try:
                os.killpg(server.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                server.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(server.pid, signal.SIGKILL)
                server.communicate(timeout=5)


if __name__ == '__main__':
    main()
