#!/usr/bin/env python3
"""Deterministic release records for the runtime images and Mac artifacts.

Offline and credential-free: it hashes local inputs, writes a layered manifest,
SHA256SUMS and an unsigned provenance statement, and diffs two such records.
It never builds, pushes or signs anything, and records no wall-clock time.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile

import importlib.util

_SPEC = importlib.util.spec_from_file_location('artifacts', Path(__file__).with_name('artifacts.py'))
artifacts = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(artifacts)

MANIFEST_SCHEMA = 'm8s.release-manifest/v1'
PROVENANCE_SCHEMA = 'https://in-toto.io/Statement/v1'
PROVENANCE_PREDICATE = 'https://m8s.dev/provenance/unsigned-local/v1'
DIGEST = re.compile(r'sha256:[0-9a-f]{64}')
NAME = re.compile(r'[a-z0-9][a-z0-9._-]{0,63}')
SHA = re.compile(r'[0-9a-f]{64}')

# Inputs that decide each layer. The lightweight `agent` layer must never depend
# on native or model inputs, so an agent-only change keeps their cache keys.
NATIVE_FILES = ('native-source.json', 'artifact-sources.json', 'native-client-required-key.patch',
                'Dockerfile.source', 'Dockerfile.flint', 'normalize_cuda_arch.py')
MODEL_FILES = ('model-artifact.json', 'qwen38-model-artifact.json', 'flint-model-artifact.json')
AGENT_FILES = ('Dockerfile.agent', 'Dockerfile.agent-update', 'artifacts.py')

# Build contexts must never hold these. Names are matched per path component.
FORBIDDEN_NAMES = re.compile(
    r'(^\.git$|^\.env($|\.)|^\.netrc$|^\.npmrc$|^\.pypirc$|^\.docker$|^id_(rsa|ed25519|ecdsa|dsa)($|\.)|'
    r'^credentials(\.|$)|\.(pem|key|p12|pfx|gguf|safetensors|bin|pt|ckpt)$)', re.I)
SECRET_CONTENT = re.compile(
    rb'(-----BEGIN [A-Z ]*PRIVATE KEY-----|\b(VAST_API_KEY|HF_TOKEN|GITHUB_TOKEN|GH_TOKEN|'
    rb'AWS_SECRET_ACCESS_KEY)\s*[=:]\s*["\']?[A-Za-z0-9_\-]{8,}|\bgh[pousr]_[A-Za-z0-9]{30,}|'
    rb'\bhf_[A-Za-z0-9]{30,}|\bsk-[A-Za-z0-9]{32,})')
MAX_SCAN = 4 * 1024 * 1024
SECRET_DOCKER = re.compile(r'^\s*(ARG|ENV)\s+\S*(KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL)\S*', re.I | re.M)
# Native/model work an agent-only Dockerfile must not perform.
AGENT_FORBIDDEN = re.compile(r'(\bjust\s|mesh-source|llama|MODEL_CONTRACT|\.gguf|nvcc|release-build)')


class Refusal(Exception):
    pass


def require(condition, message='refused'):
    if not condition:
        raise Refusal(message)


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, 'rb') as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':')).encode() + b'\n'


def pretty(value):
    return (json.dumps(value, sort_keys=True, indent=2) + '\n').encode()


def key_of(value):
    return hashlib.sha256(canonical(value)).hexdigest()


# --- deterministic source archive -------------------------------------------------

def git(root, *args):
    return subprocess.run(['git', '-C', str(root), *args], check=True, capture_output=True).stdout


def resolve_commit(root, rev):
    commit = git(root, 'rev-parse', '--verify', rev + '^{commit}').decode().strip()
    require(re.fullmatch(r'[0-9a-f]{40}', commit), 'commit')
    return commit


def source_archive(root, commit, output):
    """`git archive` is a pure function of the tree plus commit time; verify by running it twice."""
    first = git(root, 'archive', '--format=tar', commit)
    second = git(root, 'archive', '--format=tar', commit)
    require(first == second, 'git archive is not reproducible')
    out = Path(output)
    with open(out, 'xb') as stream:
        stream.write(first)
    return hashlib.sha256(first).hexdigest()


# --- build-context audit -----------------------------------------------------------

def audit_names_and_content(name, data, problems, label=''):
    parts = [p for p in name.split('/') if p]
    for part in parts:
        if FORBIDDEN_NAMES.search(part):
            problems.append(f'forbidden path: {label}{name}')
            return
    if SECRET_CONTENT.search(data[:MAX_SCAN]):
        problems.append(f'secret-like content: {label}{name}')


def audit_tar(path, problems, label):
    with tarfile.open(path, 'r:') as tar:
        for member in tar:
            if member.isfile():
                data = tar.extractfile(member).read(MAX_SCAN)
                audit_names_and_content(member.name, data, problems, label + '!')
            elif not member.isdir():
                problems.append(f'non-regular member: {label}!{member.name}')


def audit_context(context):
    """Return a sorted list of problems; empty means the context looks clean."""
    root = Path(context)
    require(root.is_dir(), 'context')
    problems = []
    for path in sorted(root.rglob('*')):
        rel = path.relative_to(root).as_posix()
        if path.is_symlink():
            problems.append(f'symlink: {rel}')
        elif path.is_file():
            with open(path, 'rb') as stream:
                data = b'' if path.suffix == '.tar' else stream.read(MAX_SCAN)
            audit_names_and_content(rel, data, problems)
            if path.suffix == '.tar':
                try:
                    audit_tar(path, problems, rel)
                except tarfile.TarError:
                    problems.append(f'unreadable archive: {rel}')
    return sorted(set(problems))


def audit_dockerfiles(directory):
    """Static layering and pinning rules for the Dockerfiles."""
    problems = []
    for path in sorted(Path(directory).glob('Dockerfile*')):
        text = path.read_text()
        code = '\n'.join(l for l in text.splitlines() if not l.lstrip().startswith('#'))
        for number, line in enumerate(text.splitlines(), 1):
            if line.startswith('FROM '):
                ref = line.split()[1]
                if not (DIGEST.search(ref) or re.fullmatch(r'\$\{[A-Z_]+\}', ref)):
                    problems.append(f'{path.name}:{number}: unpinned base {ref}')
            if '--secret' in line or re.search(r'\bADD\s+https?://', line):
                problems.append(f'{path.name}:{number}: secret mount or remote ADD')
        if SECRET_DOCKER.search(code):
            problems.append(f'{path.name}: secret-named ARG/ENV')
        if path.name in ('Dockerfile.agent', 'Dockerfile.agent-update') and AGENT_FORBIDDEN.search(code):
            problems.append(f'{path.name}: agent-only file touches native/model layers')
    return problems


# --- manifest, checksums, provenance ----------------------------------------------

def file_entry(directory, name):
    path = Path(directory) / name
    return {'path': name, 'sha256': sha256_file(path)} if path.is_file() else None


def layer_inputs(directory, names):
    entries = [file_entry(directory, name) for name in names]
    return [entry for entry in entries if entry]


def parse_pairs(items, value_check, what):
    result = {}
    for item in items or []:
        name, sep, value = item.partition('=')
        require(sep and NAME.fullmatch(name) and name not in result, what)
        require(value_check(value), what)
        result[name] = value
    return dict(sorted(result.items()))


def build_manifest(directory, commit, source_sha256, images, mac_artifacts):
    """`mac_artifacts` maps name -> path; only digests and sizes are recorded."""
    require(SHA.fullmatch(source_sha256), 'source sha')
    native = {'inputs': layer_inputs(directory, NATIVE_FILES)}
    model = {'inputs': layer_inputs(directory, MODEL_FILES)}
    agent = {'inputs': layer_inputs(directory, AGENT_FILES), 'm8s_source_sha256': source_sha256,
             'm8s_source_revision': commit}
    sources = Path(directory) / 'artifact-sources.json'
    pins = json.loads(sources.read_text()) if sources.is_file() else {}
    bases = {k: pins[k] for k in ('cuda_devel_base', 'cuda_runtime_base') if k in pins}
    for layer in (native, model, agent):
        layer['cache_key'] = key_of({k: v for k, v in layer.items()})
    macs = []
    for name, path in sorted(mac_artifacts.items()):
        macs.append({'name': name, 'sha256': sha256_file(path), 'bytes': os.stat(path).st_size})
    return {
        'schema': MANIFEST_SCHEMA,
        'source': {'revision': commit, 'archive_sha256': source_sha256},
        'bases': bases,
        'layers': {'native': native, 'model': model, 'agent': agent},
        'images': images,
        'mac_artifacts': macs,
        'network_inputs': ['cargo fetch --locked (registry crates, checksummed by Cargo.lock)',
                           'apt-get packages in agent-build stage (unpinned versions)'],
    }


def build_provenance(manifest, manifest_sha256):
    subjects = [{'name': f'image/{n}', 'digest': {'sha256': d[len('sha256:'):]}}
                for n, d in manifest['images'].items()]
    subjects += [{'name': f"mac/{m['name']}", 'digest': {'sha256': m['sha256']}}
                 for m in manifest['mac_artifacts']]
    subjects.append({'name': 'source/m8s-source.tar', 'digest': {'sha256': manifest['source']['archive_sha256']}})
    return {
        '_type': PROVENANCE_SCHEMA,
        'subject': subjects,
        'predicateType': PROVENANCE_PREDICATE,
        'predicate': {
            'builder': 'local operator; unsigned, not a hosted-builder attestation',
            'materials': {'source_revision': manifest['source']['revision'],
                          'bases': manifest['bases'],
                          'layer_cache_keys': {k: v['cache_key'] for k, v in manifest['layers'].items()}},
            'manifest_sha256': manifest_sha256,
            'reproducibility': 'Same pins should yield matching digests; use `compare` to document diffs.',
        },
    }


def checksum_lines(entries):
    return ''.join(f'{digest}  {name}\n' for name, digest in sorted(entries.items()))


def write_release(output, manifest, extra_files):
    out = Path(output)
    out.mkdir(parents=True)
    manifest_bytes = pretty(manifest)
    provenance_bytes = pretty(build_provenance(manifest, hashlib.sha256(manifest_bytes).hexdigest()))
    (out / 'release-manifest.json').write_bytes(manifest_bytes)
    (out / 'provenance.json').write_bytes(provenance_bytes)
    sums = {'release-manifest.json': hashlib.sha256(manifest_bytes).hexdigest(),
            'provenance.json': hashlib.sha256(provenance_bytes).hexdigest()}
    for name, path in extra_files.items():
        sums[name] = sha256_file(path)
    (out / 'SHA256SUMS').write_text(checksum_lines(sums))


def verify_release(directory, search_dirs=()):
    """Check SHA256SUMS against files in the release dir or the given search dirs."""
    root = Path(directory)
    problems = []
    lines = (root / 'SHA256SUMS').read_text().splitlines()
    for line in lines:
        match = re.fullmatch(r'([0-9a-f]{64})  ([A-Za-z0-9._-]+)', line)
        if not match:
            problems.append(f'malformed line: {line!r}')
            continue
        digest, name = match.groups()
        for base in (root, *map(Path, search_dirs)):
            candidate = base / name
            if candidate.is_file():
                if sha256_file(candidate) != digest:
                    problems.append(f'checksum mismatch: {name}')
                break
        else:
            problems.append(f'missing file: {name}')
    manifest = json.loads((root / 'release-manifest.json').read_text())
    provenance = json.loads((root / 'provenance.json').read_text())
    if manifest.get('schema') != MANIFEST_SCHEMA:
        problems.append('manifest schema')
    if provenance['predicate']['manifest_sha256'] != sha256_file(root / 'release-manifest.json'):
        problems.append('provenance does not match manifest')
    return problems


# --- reproducibility diff ----------------------------------------------------------

def flatten(value, prefix=''):
    if isinstance(value, dict):
        for key in sorted(value):
            yield from flatten(value[key], f'{prefix}{key}.')
    elif isinstance(value, list):
        for index, item in enumerate(value):
            yield from flatten(item, f'{prefix}{index}.')
    else:
        yield prefix.rstrip('.'), value


def compare_releases(first, second, documented=()):
    """Diff two release dirs. `documented` are path prefixes accepted with a recorded reason."""
    a = dict(flatten(json.loads((Path(first) / 'release-manifest.json').read_text())))
    b = dict(flatten(json.loads((Path(second) / 'release-manifest.json').read_text())))
    diffs = []
    for path in sorted(set(a) | set(b)):
        if a.get(path) != b.get(path):
            diffs.append({'path': path, 'first': a.get(path), 'second': b.get(path),
                          'documented': any(path.startswith(p) for p in documented)})
    undocumented = [d for d in diffs if not d['documented']]
    return {'identical': not diffs, 'reproducible': not undocumented, 'diffs': diffs}


# --- CLI --------------------------------------------------------------------------

def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    p = sub.add_parser('source', help='write a deterministic source archive and print its sha256')
    p.add_argument('--repo', default='.')
    p.add_argument('--rev', default='HEAD')
    p.add_argument('--output', required=True)
    p = sub.add_parser('audit', help='refuse secrets/weights/.git in a context; lint Dockerfiles')
    p.add_argument('--context')
    p.add_argument('--dockerfiles')
    p = sub.add_parser('manifest', help='write manifest, provenance and SHA256SUMS')
    p.add_argument('--container-dir', default=str(Path(__file__).parent))
    p.add_argument('--repo', default='.')
    p.add_argument('--rev', default='HEAD')
    p.add_argument('--source-tar', required=True)
    p.add_argument('--image', action='append', metavar='NAME=sha256:DIGEST')
    p.add_argument('--mac-artifact', action='append', metavar='NAME=PATH')
    p.add_argument('--output', required=True)
    p = sub.add_parser('verify', help='check SHA256SUMS and provenance linkage')
    p.add_argument('release')
    p.add_argument('--search', action='append', default=[])
    p = sub.add_parser('compare', help='digest-diff two release directories')
    p.add_argument('first')
    p.add_argument('second')
    p.add_argument('--documented', action='append', default=[], metavar='PATH_PREFIX')
    args = parser.parse_args(argv)

    if args.command == 'source':
        print(source_archive(args.repo, resolve_commit(args.repo, args.rev), args.output))
    elif args.command == 'audit':
        require(args.context or args.dockerfiles, 'nothing to audit')
        problems = (audit_context(args.context) if args.context else []) + \
                   (audit_dockerfiles(args.dockerfiles) if args.dockerfiles else [])
        for problem in problems:
            print(problem, file=sys.stderr)
        return 1 if problems else 0
    elif args.command == 'manifest':
        commit = resolve_commit(args.repo, args.rev)
        require(Path(args.source_tar).name == 'm8s-source.tar', 'source tar must be named m8s-source.tar')
        images = parse_pairs(args.image, DIGEST.fullmatch, 'image')
        macs = parse_pairs(args.mac_artifact, lambda v: Path(v).is_file(), 'mac artifact')
        digest = sha256_file(args.source_tar)
        manifest = build_manifest(args.container_dir, commit, digest, images, macs)
        extra = {'m8s-source.tar': args.source_tar, **{Path(v).name: v for v in macs.values()}}
        write_release(args.output, manifest, extra)
    elif args.command == 'verify':
        problems = verify_release(args.release, args.search)
        for problem in problems:
            print(problem, file=sys.stderr)
        return 1 if problems else 0
    elif args.command == 'compare':
        report = compare_releases(args.first, args.second, args.documented)
        sys.stdout.buffer.write(pretty(report))
        return 0 if report['reproducible'] else 1
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main(sys.argv[1:]))
    except Refusal as error:
        print(f'release refused: {error}', file=sys.stderr)
        sys.exit(2)
