#!/usr/bin/env python3
"""Normalize one qualified bundle's NVCC target to probed compute capability.

NVIDIA: architecture-specific `a` code runs only on the exact compute capability.
The pinned native probe reports 12.0 as `120`; its resolver compares exact strings.
This does not change any binary, ABI, toolkit, minimum driver, or security gate.
"""
import hashlib
import json
import pathlib
import sys

REFERENCE = 'https://developer.nvidia.com/blog/nvidia-blackwell-and-nvidia-cuda-12-9-introduce-family-specific-architecture-features'


def require(condition):
    if not condition:
        raise ValueError('unexpected qualified runtime')


def file_sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def tree_digest(files):
    """Match MeshLLM product_runtime_tree_sha256, including manifest.json."""
    digest = hashlib.sha256()
    for name, file_digest in sorted(files.items()):
        encoded = name.encode('utf-8')
        digest.update(len(encoded).to_bytes(8, 'big'))
        digest.update(encoded)
        digest.update(bytes.fromhex(file_digest))
    return digest.hexdigest()


def normalize(root):
    root = pathlib.Path(root)
    path = root / 'manifest.json'
    original = path.read_bytes()
    manifest = json.loads(original)
    runtime = manifest['runtime']
    require(runtime['release_version'] == '0.76.1')
    require(runtime['skippy_abi'] == '0.1.71')
    require(runtime['id'] == 'meshllm-native-runtime-linux-x86_64-cuda12')
    require(runtime['backend'] == {'kind': 'cuda', 'cuda': {'toolkit_major': 12, 'gpu_arches': ['120a']}})
    require(root.parent.name == 'native-runtimes' and root.name == runtime['id'])
    product_root = root.parent.parent
    product_path = product_root / 'product-manifest.json'
    product_original = product_path.read_bytes()
    product = json.loads(product_original)
    require(product['schema_version'] == 2 and product['contract'] == 'mesh-llm-product-v2')
    require(product['host']['required_skippy_abi'] == runtime['skippy_abi'])
    require(product['host']['path'] == 'mesh-llm')
    require(file_sha256(product_root / 'mesh-llm') == product['host']['sha256'])
    declared = product['runtime']
    for field in ['id', 'release_version', 'skippy_abi']:
        require(declared[field] == runtime[field])
    require(declared['path'] == 'native-runtimes/' + runtime['id'])
    require(declared['manifest_sha256'] == hashlib.sha256(original).hexdigest())
    files = {}
    for entry in root.rglob('*'):
        require(not entry.is_symlink())
        if entry.is_file():
            files[entry.relative_to(root).as_posix()] = file_sha256(entry)
    require(declared['sha256'] == tree_digest(files))
    # Refuse corruption before any writes. Preserve every native library hash,
    # the host binary, ABI, toolkit, release, and all security enforcement.
    for name, expected in runtime['files'].items():
        require(not pathlib.PurePosixPath(name).is_absolute() and '..' not in pathlib.PurePosixPath(name).parts)
        require(files.get(name) == expected)
    runtime['backend']['cuda']['gpu_arches'] = ['120']
    updated = (json.dumps(manifest, indent=2) + '\n').encode()
    original_tree = declared['sha256']
    files['manifest.json'] = hashlib.sha256(updated).hexdigest()
    declared['manifest_sha256'] = files['manifest.json']
    declared['sha256'] = tree_digest(files)
    product_updated = (json.dumps(product, indent=2) + '\n').encode()
    # This build layer is published only if both writes and native qualification
    # succeed. Updating only the nested manifest breaks the product integrity gate.
    path.write_bytes(updated)
    product_path.write_bytes(product_updated)
    return {'schema': 'm8s.cuda-architecture-normalization/v2',
            'nvcc_target': '120a', 'probed_compute_capability': '120',
            'original_manifest_sha256': hashlib.sha256(original).hexdigest(),
            'normalized_manifest_sha256': hashlib.sha256(updated).hexdigest(),
            'original_product_sha256': hashlib.sha256(product_original).hexdigest(),
            'normalized_product_sha256': hashlib.sha256(product_updated).hexdigest(),
            'original_runtime_tree_sha256': original_tree,
            'normalized_runtime_tree_sha256': declared['sha256'],
            'product_integrity_metadata_updated': True,
            'host_binary_hash_unchanged': True,
            'native_library_hashes_unchanged': True, 'abi_unchanged': '0.1.71',
            'release_unchanged': '0.76.1', 'authority': REFERENCE}


if __name__ == '__main__':
    try:
        receipt = normalize('/opt/mesh/native-runtimes/meshllm-native-runtime-linux-x86_64-cuda12')
        pathlib.Path('/etc/m8s/runtime-architecture-normalization.json').write_text(json.dumps(receipt, indent=2) + '\n')
    except (AssertionError, KeyError, OSError, ValueError):
        sys.exit('qualified CUDA architecture normalization refused')
