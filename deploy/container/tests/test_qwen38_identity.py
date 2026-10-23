"""Bind the live native HF identity to the exact pinned GGUF metadata."""
import hashlib
import json
import pathlib
import unittest


class QwenIdentity(unittest.TestCase):
    def test_native_identity_hashes_canonical_hf_source_not_raw_blob(self):
        proof = json.loads((pathlib.Path(__file__).parents[1] / 'qwen38-owner-canary-proof.json').read_text())
        source = proof['pinned_source_identity']
        # Native HuggingFaceGgufIdentity field order is part of this wire hash.
        canonical = {'schema_version': source['schema_version'], 'source': source['source'],
                     'repository': source['repository'], 'revision': source['revision'],
                     'files': [{'path': f['path'], 'bytes': f['bytes'], 'sha256': f['sha256']}
                               for f in source['files']]}
        digest = hashlib.sha256(json.dumps(canonical, separators=(',', ':')).encode()).hexdigest()
        self.assertEqual(proof['verified_peer_content_identity'], 'local-gguf/sha256-' + digest)
        self.assertEqual(proof['model'], proof['verified_peer_content_identity'])
        self.assertEqual(len(source['files']), 1)
        self.assertNotEqual(digest, source['files'][0]['sha256'])
        self.assertEqual(proof['canonical_api_model'], source['repository'] + '@' + source['revision'] + ':UD-Q4_K_M')
