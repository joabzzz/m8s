import hashlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest

HERE = Path(__file__).parents[1]
SPEC = importlib.util.spec_from_file_location('release', HERE / 'release.py')
R = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(R)

DIGEST = 'sha256:' + 'a' * 64


class Release(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.pins = self.root / 'pins'
        self.pins.mkdir()
        for name in R.NATIVE_FILES + R.MODEL_FILES + R.AGENT_FILES:
            (self.pins / name).write_text(json.dumps({'f': name}) if name.endswith('.json') else name + '\n')
        self.tar = self.root / 'm8s-source.tar'
        self.tar.write_bytes(b'source-one')
        self.mac = self.root / 'm8s-macos-arm64'
        self.mac.write_bytes(b'mac')

    def manifest(self, source=b'source-one', images=None):
        self.tar.write_bytes(source)
        return R.build_manifest(self.pins, 'b' * 40, hashlib.sha256(source).hexdigest(),
                                images or {'agent': DIGEST}, {'m8s-macos-arm64': self.mac})

    def release(self, name, **kw):
        manifest = self.manifest(**kw)
        out = self.root / name
        R.write_release(out, manifest, {'m8s-source.tar': self.tar, self.mac.name: self.mac})
        return out

    def test_same_pins_are_byte_identical_and_verify(self):
        a, b = self.release('a'), self.release('b')
        for name in ('release-manifest.json', 'provenance.json', 'SHA256SUMS'):
            self.assertEqual((a / name).read_bytes(), (b / name).read_bytes())
        self.assertEqual(R.verify_release(a, [self.root]), [])
        report = R.compare_releases(a, b)
        self.assertTrue(report['identical'] and report['reproducible'])

    def test_agent_change_keeps_native_and_model_keys(self):
        one = self.manifest()
        two = self.manifest(source=b'source-two')
        self.assertNotEqual(one['layers']['agent']['cache_key'], two['layers']['agent']['cache_key'])
        for layer in ('native', 'model'):
            self.assertEqual(one['layers'][layer], two['layers'][layer])

    def test_native_change_keeps_agent_key(self):
        one = self.manifest()
        (self.pins / 'native-source.json').write_text('{"changed":1}')
        two = self.manifest()
        self.assertNotEqual(one['layers']['native']['cache_key'], two['layers']['native']['cache_key'])
        self.assertEqual(one['layers']['agent'], two['layers']['agent'])

    def test_compare_reports_and_documents_diffs(self):
        a = self.release('a')
        b = self.release('b', images={'agent': 'sha256:' + 'c' * 64})
        report = R.compare_releases(a, b)
        self.assertFalse(report['reproducible'])
        self.assertEqual([d['path'] for d in report['diffs']], ['images.agent'])
        report = R.compare_releases(a, b, ['images.'])
        self.assertTrue(report['reproducible'] and not report['identical'])

    def test_verify_detects_tamper_and_missing(self):
        out = self.release('a')
        self.mac.write_bytes(b'tampered')
        self.assertIn('checksum mismatch: m8s-macos-arm64', R.verify_release(out, [self.root]))
        self.assertIn('missing file: m8s-source.tar', R.verify_release(out))
        (out / 'release-manifest.json').write_text('{"schema":"m8s.release-manifest/v1"}')
        self.assertIn('provenance does not match manifest', R.verify_release(out, [self.root]))

    def test_provenance_is_unsigned_and_lists_subjects(self):
        out = self.release('a')
        statement = json.loads((out / 'provenance.json').read_text())
        names = {s['name'] for s in statement['subject']}
        self.assertEqual(names, {'image/agent', 'mac/m8s-macos-arm64', 'source/m8s-source.tar'})
        self.assertIn('unsigned', statement['predicate']['builder'])

    def test_pair_parsing_refuses_bad_and_duplicate(self):
        for items in (['x=sha256:short'], ['Bad=' + DIGEST], [f'a={DIGEST}', f'a={DIGEST}'], ['nodigest']):
            with self.assertRaises(R.Refusal):
                R.parse_pairs(items, R.DIGEST.fullmatch, 'image')

    def test_context_audit(self):
        ctx = self.root / 'ctx'
        ctx.mkdir()
        (ctx / 'Dockerfile').write_text('FROM x\n')
        self.assertEqual(R.audit_context(ctx), [])
        (ctx / '.env').write_text('x=1')
        (ctx / 'weights.gguf').write_bytes(b'GGUF')
        (ctx / 'notes.txt').write_text('VAST_API_KEY=abcdef123456\n')
        (ctx / 'link').symlink_to(ctx / 'Dockerfile')
        problems = R.audit_context(ctx)
        for expected in ('forbidden path: .env', 'forbidden path: weights.gguf',
                         'secret-like content: notes.txt', 'symlink: link'):
            self.assertIn(expected, problems)

    def test_context_audit_scans_inside_source_archive(self):
        ctx = self.root / 'ctx'
        ctx.mkdir()
        with tarfile.open(ctx / 'm8s-source.tar', 'w') as tar:
            for name, data in (('src/main.rs', b'fn main(){}'), ('.git/config', b'x'),
                               ('src/k.txt', b'-----BEGIN OPENSSH PRIVATE KEY-----')):
                info = tarfile.TarInfo(name)
                info.size = len(data)
                tar.addfile(info, io.BytesIO(data))
        problems = R.audit_context(ctx)
        self.assertIn('forbidden path: m8s-source.tar!.git/config', problems)
        self.assertIn('secret-like content: m8s-source.tar!src/k.txt', problems)
        self.assertEqual(len(problems), 2)

    def test_repository_dockerfiles_are_pinned_and_layered(self):
        self.assertEqual(R.audit_dockerfiles(HERE), [])

    def test_dockerfile_lint_catches_violations(self):
        d = self.root / 'df'
        d.mkdir()
        (d / 'Dockerfile.agent').write_text('FROM rust:latest\nARG HF_TOKEN\nRUN just bundle\n')
        problems = R.audit_dockerfiles(d)
        self.assertEqual(len(problems), 3)

    def test_source_archive_is_reproducible(self):
        repo = self.root / 'repo'
        repo.mkdir()
        env = {'GIT_AUTHOR_NAME': 'a', 'GIT_AUTHOR_EMAIL': 'a@x', 'GIT_COMMITTER_NAME': 'a',
               'GIT_COMMITTER_EMAIL': 'a@x', 'GIT_AUTHOR_DATE': '2020-01-01T00:00:00Z',
               'GIT_COMMITTER_DATE': '2020-01-01T00:00:00Z', 'PATH': '/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin',
               'HOME': str(self.root)}
        for cmd in (['init', '-q'], ['config', 'commit.gpgsign', 'false']):
            subprocess.run(['git', '-C', str(repo), *cmd], check=True, env=env)
        (repo / 'f.txt').write_text('hi')
        subprocess.run(['git', '-C', str(repo), 'add', '.'], check=True, env=env)
        subprocess.run(['git', '-C', str(repo), 'commit', '-qm', 'x'], check=True, env=env)
        commit = R.resolve_commit(repo, 'HEAD')
        one = R.source_archive(repo, commit, self.root / 'one.tar')
        two = R.source_archive(repo, commit, self.root / 'two.tar')
        self.assertEqual(one, two)
        with self.assertRaises(FileExistsError):
            R.source_archive(repo, commit, self.root / 'one.tar')


if __name__ == '__main__':
    unittest.main()
