"""Exercise native product integrity in a disposable, network-disabled CPU container.

The synthetic SM120 profile selects the packaged bundle; it is never a GPU
observation and must never be supplied to a live rental. No credential, key,
certificate, model weights or provider access is used. Native output stays in
memory. Reaching the expected driver-library load failure proves the preceding
native product manifest/tree checks passed, not that CUDA or inference works.
"""
import json
import pathlib
import subprocess
import tempfile


def main():
    with tempfile.TemporaryDirectory(prefix='m8s-product-integrity-') as directory:
        home = pathlib.Path(directory)
        # These fixtures deliberately do not exist, so no weights can load.
        args = ['/opt/mesh/mesh-llm', 'serve', '--gguf', str(home / 'absent-model.gguf'),
                '--node-ownership-required', '--node-ownership', str(home / 'absent-certificate.json'),
                '--headless', '--log-format', 'json', '--no-default-plugins']
        env = {'HOME': directory, 'PATH': '/usr/bin:/bin:/usr/sbin:/sbin',
               'LD_LIBRARY_PATH': '/usr/local/cuda/lib64',
               'MESH_LLM_ANALYTICS': '0', 'DO_NOT_TRACK': '1',
               'MESH_LLM_CUDA_GPU_ARCHES': '120', 'MESH_LLM_CUDA_TOOLKIT_MAJOR': '12',
               'MESH_LLM_CUDA_DRIVER_MAX_MAJOR': '13'}
        result = subprocess.run(args, env=env, stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=150)
        output = result.stdout + result.stderr
        if (len(output) > 65536 or result.returncode != 1
                or b'checksum mismatch' in output
                or b'failed to load native runtime library' not in output):
            raise ValueError('native product integrity qualification refused')
        print(json.dumps({'schema': 'm8s.native-product-integrity-check/v1',
                          'native_product_integrity_gate_passed': True,
                          'native_library_load_reached': True,
                          'driver_library_load_failed_in_cpu_container': True,
                          'synthetic_gpu_profile': True, 'gpu_or_inference_proof': False}))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError):
        raise SystemExit('native product integrity qualification refused')
