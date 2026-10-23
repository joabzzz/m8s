#!/usr/bin/env python3
"""Bounded public-artifact checks; not admission, native ABI verification or readiness."""
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import stat
import struct
import subprocess
import sys
import tarfile
import time

MAX_MODEL = 2 * 1024**3
MAX_SOURCE = 2 * 1024**3


class Refusal(Exception):
    pass


def require(condition):
    if not condition:
        raise Refusal()


def pairs(items):
    result = {}
    for key, value in items:
        require(key not in result)
        result[key] = value
    return result


def model_contract(path):
    with open(path, 'rb') as stream:
        data = stream.read(4097)
    require(len(data) <= 4096)
    contract = json.loads(data, object_pairs_hook=pairs)
    require(type(contract) is dict and set(contract) == {'schema', 'model', 'sha256', 'bytes', 'format'})
    require(contract['schema'] == 'm8s.local-model/v1' and contract['format'] == 'gguf')
    require(type(contract['model']) is str and re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.:/-]{0,127}', contract['model']))
    require(type(contract['sha256']) is str and re.fullmatch(r'[0-9a-f]{64}', contract['sha256']))
    require(type(contract['bytes']) is int and 24 <= contract['bytes'] <= MAX_MODEL)
    return contract


def identity(info):
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns,
            info.st_uid, info.st_mode, info.st_nlink)


def check_model(path, contract_path):
    contract = model_contract(contract_path)
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, 'rb') as stream:
        before = os.fstat(stream.fileno())
        require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1)
        require(before.st_uid in (0, os.geteuid()) and not before.st_mode & 0o7022)
        require(before.st_size == contract['bytes'])
        require(identity(os.stat(path, follow_symlinks=False)) == identity(before))
        header = stream.read(24)
        require(len(header) == 24 and header[:4] == b'GGUF' and struct.unpack('<I', header[4:8])[0] in (2, 3))
        digest = hashlib.sha256(header)
        count = 24
        while chunk := stream.read(min(1024 * 1024, contract['bytes'] - count + 1)):
            count += len(chunk)
            require(count <= contract['bytes'])
            digest.update(chunk)
        require(count == contract['bytes'] and digest.hexdigest() == contract['sha256'])
        require(identity(os.fstat(stream.fileno())) == identity(before))
        require(identity(os.stat(path, follow_symlinks=False)) == identity(before))


def check_linux_binary(binary):
    with open(binary, 'rb') as stream:
        header = stream.read(64)
    require(len(header) == 64 and header[:6] == b'\x7fELF\x02\x01')
    require(struct.unpack('<H', header[16:18])[0] in (2, 3))
    require(struct.unpack('<H', header[18:20])[0] in (62, 183))


def check_native(binary):
    check_linux_binary(binary)
    check_help(binary)


def check_help(binary):
    # Help is a compatibility check only: it does not start an endpoint or prove enforcement.
    child = subprocess.Popen([binary, '--help-advanced'], stdin=subprocess.DEVNULL,
                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                             env={'PATH': '/usr/bin:/bin', 'MESH_LLM_ANALYTICS': '0', 'DO_NOT_TRACK': '1'})
    output = bytearray()
    deadline = time.monotonic() + 10
    try:
        with selectors.DefaultSelector() as selector:
            os.set_blocking(child.stdout.fileno(), False)
            selector.register(child.stdout, selectors.EVENT_READ)
            while True:
                remaining = deadline - time.monotonic()
                require(remaining > 0)
                require(selector.select(remaining))
                chunk = os.read(child.stdout.fileno(), 4096)
                if not chunk:
                    break
                output.extend(chunk)
                require(len(output) <= 65536)
        require(child.wait(timeout=max(0.001, deadline - time.monotonic())) == 0)
        require(re.search(rb'(?<![A-Za-z0-9_-])--node-ownership-required(?![A-Za-z0-9_-])', output))
    finally:
        if child.poll() is None:
            child.kill()
        child.wait()
        child.stdout.close()


def unpack_source(archive, expected, destination):
    require(re.fullmatch(r'[0-9a-f]{64}', expected))
    digest = hashlib.sha256()
    size = 0
    with open(archive, 'rb') as stream:
        while chunk := stream.read(1024 * 1024):
            size += len(chunk)
            require(size <= MAX_SOURCE)
            digest.update(chunk)
        require(digest.hexdigest() == expected)
        stream.seek(0)
        # Validate every member before writing anything; git archives contain no links here.
        with tarfile.open(fileobj=stream, mode='r:') as tar:
            members = []
            seen = set()
            total = 0
            for member in tar:
                parts = member.name.rstrip('/').split('/')
                require(len(members) < 30000 and len(parts) <= 32)
                require(all(part not in ('', '.', '..', '.git') for part in parts))
                require(member.isfile() or member.isdir())
                require(member.name not in seen and not member.mode & 0o7000)
                require(0 <= member.size <= 256 * 1024**2)
                total += member.size
                require(total <= MAX_SOURCE)
                seen.add(member.name)
                members.append(member)
            root = Path(destination)
            root.mkdir(mode=0o700)  # Exclusive new output, never overwrite an existing tree.
            for member in members:
                target = root / member.name
                if member.isdir():
                    target.mkdir(parents=True, exist_ok=True)
                else:
                    target.parent.mkdir(parents=True, exist_ok=True)
                    with tar.extractfile(member) as source, open(target, 'xb') as output:
                        while chunk := source.read(1024 * 1024):
                            output.write(chunk)
                    target.chmod(0o755 if member.mode & 0o111 else 0o644)


def main(argv):
    if len(argv) == 4 and argv[0] == 'unpack-source':
        unpack_source(*argv[1:])
    elif len(argv) == 3 and argv[0] == 'check-model':
        check_model(*argv[1:])
    elif len(argv) == 2 and argv[0] == 'check-base':
        require(re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._:/-]*@sha256:[0-9a-f]{64}', argv[1]))
    elif len(argv) == 2 and argv[0] == 'check-contract':
        model_contract(argv[1])
    elif len(argv) == 2 and argv[0] == 'check-native':
        check_native(argv[1])
    else:
        raise Refusal()


if __name__ == '__main__':
    try:
        main(sys.argv[1:])
    except Exception:
        print('artifact validation refused', file=sys.stderr)
        sys.exit(1)
