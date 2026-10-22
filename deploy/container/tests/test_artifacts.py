import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import tarfile
import unittest

SPEC = importlib.util.spec_from_file_location('artifacts', Path(__file__).parents[1] / 'artifacts.py')
A = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(A)


class Artifacts(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.model = self.root / 'model.gguf'
        self.data = b'GGUF\x03\x00\x00\x00' + bytes(16)
        self.model.write_bytes(self.data)
        self.contract = self.root / 'model.json'
        self.metadata = dict(schema='m8s.local-model/v1', model='synthetic/test',
                             sha256=hashlib.sha256(self.data).hexdigest(), bytes=24, format='gguf')
        self.save()

    def save(self):
        self.contract.write_text(json.dumps(self.metadata))

    def test_model_positive_and_digest_refusal(self):
        A.check_model(self.model, self.contract)
        self.model.write_bytes(self.data[:-1] + b'x')
        with self.assertRaises(A.Refusal):
            A.check_model(self.model, self.contract)

    def test_strict_contract(self):
        for field, value in [('bytes', True), ('bytes', A.MAX_MODEL + 1),
                             ('sha256', 'a' * 63), ('model', '\n'), ('extra', 1)]:
            original = self.metadata.copy()
            self.metadata[field] = value
            self.save()
            with self.assertRaises(A.Refusal):
                A.model_contract(self.contract)
            self.metadata = original
        self.contract.write_text('{"bytes":24,"bytes":24}')
        with self.assertRaises(A.Refusal):
            A.model_contract(self.contract)

    def test_links_and_permissions(self):
        link = self.root / 'link'
        link.symlink_to(self.model)
        with self.assertRaises(OSError):
            A.check_model(link, self.contract)
        self.model.chmod(0o666)
        with self.assertRaises(A.Refusal):
            A.check_model(self.model, self.contract)

    def test_base_digest_and_recipe_contract(self):
        A.main(['check-base', 'example.invalid/builder@sha256:' + 'a' * 64])
        for value in ('ubuntu:latest', 'builder@sha256:' + 'A' * 64, '', 'repo@sha256:abc'):
            with self.assertRaises(A.Refusal):
                A.main(['check-base', value])
        recipe = (Path(__file__).parents[1] / 'Dockerfile.source').read_text()
        self.assertIn('just bundle /out/mesh-bundle.tar.gz', recipe)
        self.assertIn('FROM ${BUILDER_BASE} AS mesh-build\nARG NATIVE_BACKEND=cpu', recipe)
        self.assertIn('cuda) just release-build-cuda', recipe)
        self.assertIn('just release-bundle "v${version#v}" /out/release cuda x86_64', recipe)
        self.assertIn('check-native /opt/mesh/mesh-llm', recipe)
        self.assertIn('check-contract /etc/m8s/model-artifact.json', recipe)
        self.assertIn('ENTRYPOINT ["/usr/local/bin/m8s-agent"]', recipe)
        self.assertNotIn('local-model-only', recipe)
        for line in recipe.splitlines():
            if line.startswith('RUN '):
                self.assertTrue(line.startswith('RUN --network=none '))

    def test_selected_public_artifact_contract(self):
        root = Path(__file__).parents[1]
        model = A.model_contract(root / 'model-artifact.json')
        sources = json.loads((root / 'artifact-sources.json').read_text())
        self.assertEqual(model['model'], sources['model']['repository'])
        self.assertEqual(len(sources['model']['revision']), 40)
        self.assertIn('/resolve/' + sources['model']['revision'] + '/', sources['model']['url'])
        self.assertTrue(sources['model']['url'].endswith('/' + sources['model']['filename']))
        self.assertEqual(sources['platform'], 'linux/amd64')
        for name in ('cuda_devel_base', 'cuda_runtime_base'):
            A.main(['check-base', sources[name]])

    def test_header_version(self):
        self.model.write_bytes(b'GGUF\x04\x00\x00\x00' + bytes(16))
        self.metadata['sha256'] = hashlib.sha256(self.model.read_bytes()).hexdigest()
        self.save()
        with self.assertRaises(A.Refusal):
            A.check_model(self.model, self.contract)

    def native(self, body):
        binary = self.root / 'fake-native'
        binary.write_text('#!/bin/sh\n' + body)
        binary.chmod(0o700)
        return str(binary)

    def test_native_capability_and_failures(self):
        A.check_help(self.native('printf "%s\\n" --node-ownership-required\n'))
        for body in ('printf "%s\\n" --node-ownership-required-extra\n',
                     'printf "%s\\n" --node-ownership-required; exit 1\n',
                     'exit 0\n'):
            with self.assertRaises(A.Refusal):
                A.check_help(self.native(body))

    def test_linux_binary_header(self):
        binary = self.root / 'synthetic-elf'
        header = bytearray(64)
        header[:6] = b'\x7fELF\x02\x01'
        header[16] = 3
        for machine in (62, 183):
            header[18] = machine
            binary.write_bytes(header)
            A.check_linux_binary(binary)
        for corrupt in (b'MZ' + bytes(62), bytes(64), b'\x7fELF'):
            binary.write_bytes(corrupt)
            with self.assertRaises(A.Refusal):
                A.check_linux_binary(binary)

    def test_native_output_cap(self):
        with self.assertRaises(A.Refusal):
            A.check_help(self.native('while :; do printf "%0100d" 1; done\n'))

    def archive(self, name='src/file', link=False):
        path = self.root / 'source.tar'
        with tarfile.open(path, 'w') as tar:
            member = tarfile.TarInfo(name)
            if link:
                member.type = tarfile.SYMTYPE
                member.linkname = '/outside'
                tar.addfile(member)
            else:
                member.size = 3
                tar.addfile(member, io.BytesIO(b'abc'))
        return path, hashlib.sha256(path.read_bytes()).hexdigest()

    def test_source_hash_and_exclusive_destination(self):
        archive, digest = self.archive()
        out = self.root / 'out'
        with self.assertRaises(A.Refusal):
            A.unpack_source(archive, '0' * 64, out)
        self.assertFalse(out.exists())
        A.unpack_source(archive, digest, out)
        self.assertEqual((out / 'src/file').read_bytes(), b'abc')
        with self.assertRaises(FileExistsError):
            A.unpack_source(archive, digest, out)

    def test_source_traversal_and_link_refusal_before_output(self):
        for name, link in [('../escape', False), ('/absolute', False),
                           ('.git/config', False), ('symlink', True)]:
            archive, digest = self.archive(name, link)
            out = self.root / 'out'
            with self.assertRaises(A.Refusal):
                A.unpack_source(archive, digest, out)
            self.assertFalse(out.exists())


if __name__ == '__main__':
    unittest.main()
