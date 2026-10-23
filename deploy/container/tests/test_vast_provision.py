import base64
import importlib.util
import io
import json
from pathlib import Path
import struct
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('provision', Path(__file__).parents[1] / 'vast_provision.py')
P = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(P)


class Provision(unittest.TestCase):
    def config(self):
        c = dict.fromkeys(P.CONFIG_FIELDS, 'synthetic')
        c.update(schema='m8s.vast-node/v1', program='/opt/mesh/mesh-llm',
                 gguf='/models/' + P.MODEL + '.gguf', model=P.MODEL,
                 material_root=str(P.ROOT / 'material'))
        return c

    def frame(self, config=None):
        raw = json.dumps(config or self.config()).encode()
        return P.MAGIC + struct.pack('>I', len(raw)) + raw + bytes([7])*32 + b'a'*64

    def test_public_key_is_exact_ed25519_wire_key_without_options(self):
        key = 'ssh-ed25519 ' + base64.b64encode(
            b'\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20' + bytes([9])*32).decode()
        self.assertEqual(P.public_key(key), key)
        for bad in [key + '\n', key + ' comment', 'restrict ' + key,
                    key.replace('ssh-ed25519', 'ssh-rsa', 1), 'ssh-ed25519 AAAA']:
            with self.assertRaises((ValueError, TypeError)):
                P.public_key(bad)

    def test_raw_nonce_and_node_key_are_separate_from_public_config(self):
        config, raw, nonce, node_key = P.packet(io.BytesIO(self.frame()))
        self.assertEqual(config, self.config())
        self.assertEqual(json.loads(raw), config)
        self.assertEqual(nonce, bytes([7])*32)
        self.assertEqual(node_key, b'a'*64)

    def test_truncated_oversized_or_trailing_frames_refuse(self):
        frame = self.frame()
        for bad in [b'', frame[:5], frame[:-1], frame + b'x',
                    P.MAGIC + struct.pack('>I', 8193),
                    frame[:-64] + b'Z'*64]:
            with self.assertRaises(ValueError):
                P.packet(io.BytesIO(bad))

    def test_fixed_paths_unknown_fields_and_duplicate_keys_refuse(self):
        for field, value in [('program', '/bin/sh'), ('gguf', '/tmp/other.gguf'),
                             ('material_root', '/tmp/other'), ('model', 'other'),
                             ('owner_private_key', 'synthetic')]:
            config = self.config()
            config[field] = value
            with self.assertRaises(ValueError):
                P.packet(io.BytesIO(self.frame(config)))
        with self.assertRaises(ValueError):
            P.unique([('schema', 'one'), ('schema', 'two')])

    def test_inputs_are_created_once_and_never_overwritten(self):
        with tempfile.TemporaryDirectory() as root:
            target = Path(root) / 'nonce'
            P.write_new(target, b'synthetic-first')
            self.assertEqual(target.stat().st_mode & 0o777, 0o600)
            with self.assertRaises(FileExistsError):
                P.write_new(target, b'synthetic-second')
            self.assertEqual(target.read_bytes(), b'synthetic-first')


if __name__ == '__main__':
    unittest.main()
