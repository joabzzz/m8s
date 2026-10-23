import hashlib
import json
import pathlib
import tempfile
import unittest
import importlib.util

SPEC = importlib.util.spec_from_file_location('normalize_cuda_arch', pathlib.Path(__file__).parents[1] / 'normalize_cuda_arch.py')
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
normalize = MODULE.normalize


class ArchitectureNormalization(unittest.TestCase):
    def fixture(self, root):
        (root / 'lib').mkdir()
        (root / 'lib/runtime.so').write_bytes(b'synthetic-library')
        runtime = {'release_version': '0.76.1', 'skippy_abi': '0.1.71',
                   'id': 'meshllm-native-runtime-linux-x86_64-cuda12',
                   'backend': {'kind': 'cuda', 'cuda': {'toolkit_major': 12, 'gpu_arches': ['120a']}},
                   'files': {'lib/runtime.so': hashlib.sha256(b'synthetic-library').hexdigest()}}
        (root / 'manifest.json').write_text(json.dumps({'runtime': runtime}))

    def test_exact_target_normalizes_without_modifying_library_or_abi(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            self.fixture(root)
            receipt = normalize(root)
            self.assertTrue(receipt['native_library_hashes_unchanged'])
            self.assertEqual((root / 'lib/runtime.so').read_bytes(), b'synthetic-library')
            runtime = json.loads((root / 'manifest.json').read_text())['runtime']
            self.assertEqual(runtime['skippy_abi'], '0.1.71')
            self.assertEqual(runtime['backend']['cuda']['gpu_arches'], ['120'])

    def test_abi_changes_other_architectures_and_corruption_refuse_before_write(self):
        for mutation in ['abi', 'arch', 'library']:
            with tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                self.fixture(root)
                path = root / 'manifest.json'
                manifest = json.loads(path.read_text())
                if mutation == 'abi':
                    manifest['runtime']['skippy_abi'] = '0.1.66'
                elif mutation == 'arch':
                    manifest['runtime']['backend']['cuda']['gpu_arches'] = ['100a']
                else:
                    (root / 'lib/runtime.so').write_bytes(b'corrupt')
                path.write_text(json.dumps(manifest))
                before = path.read_bytes()
                with self.assertRaises(ValueError):
                    normalize(root)
                self.assertEqual(path.read_bytes(), before)
