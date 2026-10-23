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
    # Refuse a corrupt/unexpected image before changing metadata. Preserve and
    # verify every native library hash, including the packaged CUDA libraries.
    for name, expected in runtime['files'].items():
        require(not pathlib.PurePosixPath(name).is_absolute() and '..' not in pathlib.PurePosixPath(name).parts)
        require(file_sha256(root / name) == expected)
    runtime['backend']['cuda']['gpu_arches'] = ['120']
    updated = (json.dumps(manifest, indent=2) + '\n').encode()
    path.write_bytes(updated)
    return {'schema': 'm8s.cuda-architecture-normalization/v1',
            'nvcc_target': '120a', 'probed_compute_capability': '120',
            'original_manifest_sha256': hashlib.sha256(original).hexdigest(),
            'normalized_manifest_sha256': hashlib.sha256(updated).hexdigest(),
            'native_library_hashes_unchanged': True, 'abi_unchanged': '0.1.71',
            'release_unchanged': '0.76.1', 'authority': REFERENCE}


if __name__ == '__main__':
    try:
        receipt = normalize('/opt/mesh/native-runtimes/meshllm-native-runtime-linux-x86_64-cuda12')
        pathlib.Path('/etc/m8s/runtime-architecture-normalization.json').write_text(json.dumps(receipt, indent=2) + '\n')
    except (AssertionError, KeyError, OSError, ValueError):
        sys.exit('qualified CUDA architecture normalization refused')
