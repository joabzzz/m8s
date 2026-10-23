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
    def fixture(self, product_root):
        root = product_root / 'native-runtimes/meshllm-native-runtime-linux-x86_64-cuda12'
        root.mkdir(parents=True)
        (root / 'lib').mkdir()
        (root / 'lib/runtime.so').write_bytes(b'synthetic-library')
        runtime = {'release_version': '0.76.1', 'skippy_abi': '0.1.71',
                   'id': 'meshllm-native-runtime-linux-x86_64-cuda12',
                   'backend': {'kind': 'cuda', 'cuda': {'toolkit_major': 12, 'gpu_arches': ['120a']}},
                   'files': {'lib/runtime.so': hashlib.sha256(b'synthetic-library').hexdigest()}}
        (root / 'manifest.json').write_text(json.dumps({'runtime': runtime}))
        (product_root / 'mesh-llm').write_bytes(b'synthetic-host')
        files = {p.relative_to(root).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
                 for p in root.rglob('*') if p.is_file()}
        product = {'schema_version': 2, 'contract': 'mesh-llm-product-v2',
                   'host': {'path': 'mesh-llm', 'required_skippy_abi': '0.1.71',
                            'sha256': hashlib.sha256(b'synthetic-host').hexdigest()},
                   'runtime': {'id': runtime['id'], 'release_version': '0.76.1',
                               'skippy_abi': '0.1.71', 'path': 'native-runtimes/' + runtime['id'],
                               'manifest_sha256': files['manifest.json'],
                               'sha256': MODULE.tree_digest(files)}}
        (product_root / 'product-manifest.json').write_text(json.dumps(product))
        return root


    def test_exact_target_normalizes_without_modifying_library_or_abi(self):
        with tempfile.TemporaryDirectory() as directory:
            root = self.fixture(pathlib.Path(directory))
            receipt = normalize(root)
            self.assertTrue(receipt['native_library_hashes_unchanged'])
            self.assertTrue(receipt['product_integrity_metadata_updated'])
            product = json.loads((root.parent.parent / 'product-manifest.json').read_text())
            self.assertEqual(product['runtime']['manifest_sha256'],
                             hashlib.sha256((root / 'manifest.json').read_bytes()).hexdigest())
            self.assertNotEqual(receipt['original_runtime_tree_sha256'], product['runtime']['sha256'])
            self.assertEqual((root.parent.parent / 'mesh-llm').read_bytes(), b'synthetic-host')
            self.assertEqual((root / 'lib/runtime.so').read_bytes(), b'synthetic-library')
            runtime = json.loads((root / 'manifest.json').read_text())['runtime']
            self.assertEqual(runtime['skippy_abi'], '0.1.71')
            self.assertEqual(runtime['backend']['cuda']['gpu_arches'], ['120'])

    def test_abi_changes_other_architectures_and_corruption_refuse_before_write(self):
        for mutation in ['abi', 'arch', 'library']:
            with tempfile.TemporaryDirectory() as directory:
                root = self.fixture(pathlib.Path(directory))
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

    def test_stale_product_digest_corrupt_host_and_unsafe_path_refuse_before_writes(self):
        for mutation in ['manifest', 'tree', 'host', 'path']:
            with tempfile.TemporaryDirectory() as directory:
                root = self.fixture(pathlib.Path(directory))
                path = root.parent.parent / 'product-manifest.json'
                product = json.loads(path.read_text())
                if mutation == 'manifest':
                    product['runtime']['manifest_sha256'] = '00' * 32
                elif mutation == 'tree':
                    product['runtime']['sha256'] = '00' * 32
                elif mutation == 'host':
                    (root.parent.parent / 'mesh-llm').write_bytes(b'corrupt-host')
                else:
                    product['runtime']['path'] = '../escape'
                path.write_text(json.dumps(product))
                before_product = path.read_bytes()
                before_runtime = (root / 'manifest.json').read_bytes()
                with self.assertRaises(ValueError):
                    normalize(root)
                self.assertEqual(path.read_bytes(), before_product)
                self.assertEqual((root / 'manifest.json').read_bytes(), before_runtime)
