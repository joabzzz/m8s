#!/usr/bin/python3
"""Restricted SSH provisioning for one Vast node; never an owner signer.

Public SSH host identity is emitted for exact-instance authenticated log lookup.
The caller must pin that identity before opening the provisioning connection.
Only the public config is JSON; nonce and node key are separate raw frames.
"""
import base64
import json
import os
from pathlib import Path
import signal
import struct
import subprocess
import sys

UID = 65532
GID = 65532
ROOT = Path('/var/lib/m8s')
SSHD = Path('/run/m8s-sshd')
MAGIC = b'M8S-PROVISION-V1\n'
MODEL = 'qwen2.5-0.5b-instruct-q4_k_m'
CONFIG_FIELDS = {'schema', 'instance_id', 'container_id', 'image_sha256',
                 'owner_endpoint', 'owner_signing_key', 'mesh_owner_id',
                 'owner_address', 'bind_address', 'trust', 'program', 'gguf',
                 'material_root', 'model', 'heartbeat_ttl_seconds',
                 'max_lifetime_seconds', 'port', 'console_port'}


def require(value):
    if not value:
        raise ValueError('provisioning refused')


def public_key(value):
    parts = value.split(' ')
    require(len(parts) == 2 and parts[0] == 'ssh-ed25519')
    raw = base64.b64decode(parts[1], validate=True)
    require(len(raw) == 51 and raw[:19] == b'\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20')
    require(base64.b64encode(raw).decode('ascii') == parts[1])
    return value


def unique(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result)
        result[key] = value
    return result


def write_new(path, data, uid=UID, gid=GID):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'wb') as stream:
        stream.write(data)
        stream.flush()
        if os.geteuid() == 0:
            os.fchown(stream.fileno(), uid, gid)
        os.fsync(stream.fileno())


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def serve():
    require(os.geteuid() == 0)
    key = public_key(os.environ.get('M8S_PROVISION_SSH_KEY', ''))
    instance = os.environ.get('CONTAINER_ID', '')
    require(instance.isascii() and instance.isdecimal() and 0 < int(instance) < 2**64)
    credential = os.environ.get('CONTAINER_API_KEY', '').encode('ascii')
    require(0 < len(credential) <= 4096 and all(33 <= b <= 126 for b in credential))
    # All directories and files belong to this new container. Existing state
    # refuses startup; no credential reset or destructive recovery is attempted.
    ROOT.mkdir(mode=0o700)
    os.chown(ROOT, UID, GID)
    SSHD.mkdir(mode=0o755)
    write_new(ROOT / 'instance-id', instance.encode('ascii'))
    write_new(ROOT / 'instance-api-key', credential)
    del credential
    write_new(SSHD / 'authorized_keys',
              ('restrict,command="/usr/bin/python3 /usr/local/lib/m8s/vast_provision.py receive" '
               + key + '\n').encode('ascii'), 0, 0)
    # sshd reads authorized_keys under the target user's identity. This file is
    # public and root-owned; host private keys in the same directory stay 0600.
    os.chmod(SSHD / 'authorized_keys', 0o444)
    # Host key is generated at runtime, never baked into a distributable image.
    subprocess.run(['/usr/bin/ssh-keygen', '-q', '-t', 'ed25519', '-N', '',
                    '-f', str(SSHD / 'host_key')], check=True,
                   stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL, env={'PATH': '/usr/bin:/bin'})
    host = (SSHD / 'host_key.pub').read_text().split()
    require(len(host) >= 2)
    host = public_key(' '.join(host[:2]))
    config = '''Port 2222
ListenAddress 0.0.0.0
HostKey /run/m8s-sshd/host_key
PidFile /run/m8s-sshd/sshd.pid
AuthorizedKeysFile /run/m8s-sshd/authorized_keys
StrictModes yes
AllowUsers m8s
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitEmptyPasswords no
PermitRootLogin no
UsePAM no
AllowTcpForwarding no
AllowAgentForwarding no
X11Forwarding no
PermitTunnel no
PermitTTY no
PermitUserEnvironment no
MaxAuthTries 2
MaxSessions 1
LoginGraceTime 20
LogLevel ERROR
ForceCommand /usr/bin/python3 /usr/local/lib/m8s/vast_provision.py receive
'''
    write_new(SSHD / 'sshd_config', config.encode('ascii'), 0, 0)
    sync_directory(ROOT)
    sync_directory(SSHD)
    print('M8S_PROVISION_HOST_KEY_V1 ' + host, flush=True)
    os.execve('/usr/sbin/sshd', ['sshd', '-D', '-e', '-f', str(SSHD / 'sshd_config')],
              {'PATH': '/usr/bin:/bin'})


def exact(stream, size):
    data = bytearray()
    while len(data) < size:
        part = stream.read(size - len(data))
        require(part)
        data.extend(part)
    return bytes(data)


def packet(stream):
    require(exact(stream, len(MAGIC)) == MAGIC)
    length = struct.unpack('>I', exact(stream, 4))[0]
    require(0 < length <= 8192)
    config_bytes = exact(stream, length)
    config = json.loads(config_bytes, object_pairs_hook=unique)
    require(type(config) is dict and set(config) == CONFIG_FIELDS
            and config.get('schema') == 'm8s.vast-node/v1')
    # The full schema is additionally checked by the Rust node before bootstrap.
    require(config.get('program') == '/opt/mesh/mesh-llm')
    require(config.get('gguf') == '/models/' + MODEL + '.gguf')
    require(config.get('model') == MODEL)
    require(config.get('material_root') == str(ROOT / 'material'))
    nonce = exact(stream, 32)
    node_key = exact(stream, 64)
    require(all(b in b'0123456789abcdef' for b in node_key))
    require(stream.read(1) == b'')
    return config, config_bytes, nonce, node_key


def receive():
    require(os.geteuid() == UID and os.environ.get('SSH_ORIGINAL_COMMAND') == 'provision-v1')
    # Bound incomplete framing without putting any secret bytes in argv/logs.
    signal.alarm(90)
    write_new(ROOT / 'provision-consumed', b'consumed\n')
    sync_directory(ROOT)
    config, raw, nonce, node_key = packet(sys.stdin.buffer)
    instance = (ROOT / 'instance-id').read_text()
    require(type(config.get('instance_id')) is int and str(config['instance_id']) == instance)
    require(config.get('container_id') == instance)
    write_new(ROOT / 'config.json', raw)
    write_new(ROOT / 'nonce', nonce)
    write_new(ROOT / 'node-key', node_key)
    sync_directory(ROOT)
    signal.alarm(0)
    credential = (ROOT / 'instance-api-key').read_text()
    require(0 < len(credential) <= 4096 and all(33 <= ord(b) <= 126 for b in credential))
    # Agent clears the native model child's environment. Only the scoped
    # instance credential reaches this agent, never the owner's account key.
    os.execve('/usr/local/bin/m8s-agent', ['m8s-agent', '--vast-node',
              str(ROOT / 'config.json'), str(ROOT / 'nonce'), str(ROOT / 'node-key')],
              {'PATH': '/usr/bin:/bin', 'CONTAINER_ID': instance,
               'CONTAINER_API_KEY': credential})


if __name__ == '__main__':
    try:
        require(len(sys.argv) == 2 and sys.argv[1] in ('serve', 'receive'))
        {'serve': serve, 'receive': receive}[sys.argv[1]]()
    except Exception:
        print('Vast provisioning refused; owner cleanup required', file=sys.stderr)
        sys.exit(1)
