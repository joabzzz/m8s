"""Fake connections only, no sockets/provider calls or real secret values."""
import contextlib
import io
import json
import os
import pickle
import ssl
import unittest
from unittest.mock import Mock, patch

import https_readonly as transport
import readonly_runner
from preflight import MAX_BODY, PreflightError

SENTINEL = 'SYNTHETIC-NEVER-A-REAL-KEY'


class Response:
    def __init__(self, body=b'{"balance":"6"}', status=200, headers=None):
        self.status = status
        self.body = body
        self.headers = headers or {}
        self.reads = []

    def getheader(self, name):
        return self.headers.get(name)

    def read(self, limit):
        self.reads.append(limit)
        return self.body[:limit]


class Connection:
    def __init__(self, response):
        self.response = response
        self.requests = []
        self.closed = False
        self.debuglevel = None

    def set_debuglevel(self, level):
        self.debuglevel = level

    def request(self, method, path, **kwargs):
        self.requests.append((method, path, kwargs))

    def getresponse(self):
        return self.response

    def close(self):
        self.closed = True


class TransportTests(unittest.TestCase):
    def setUp(self):
        self.assertNotIn('VAST_API_KEY', os.environ)
        self.assertNotIn('CONTAINER_API_KEY', os.environ)
        self.context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        self.context.minimum_version = ssl.TLSVersion.TLSv1_2
        self.tls_patch = patch.object(transport, '_tls_context', return_value=self.context)
        self.tls_patch.start()
        self.addCleanup(self.tls_patch.stop)
        self.response = Response()
        self.connection = Connection(self.response)
        self.factory = Mock(return_value=self.connection)
        self.supplier = Mock(return_value=SENTINEL)
        self.client = transport.https_client(self.supplier, _connection_factory=self.factory)

    def assert_opaque(self, action):
        try:
            action()
            self.fail('expected opaque failure')
        except PreflightError as error:
            self.assertEqual(str(error), 'preflight unavailable')
            self.assertIsNone(error.__context__)
            self.assertIsNone(error.__cause__)
            self.assertNotIn(SENTINEL, repr(error))

    def test_no_import_or_construction_time_secret_or_network(self):
        self.supplier.assert_not_called()
        self.factory.assert_not_called()
        self.assertNotIn(SENTINEL, repr(self.client))
        self.assert_opaque(lambda: pickle.dumps(self.client))

    def test_fixed_origin_tls_timeout_headers_and_sanitized_account(self):
        self.assertEqual(self.client.account(100).balance_micro_usd, 6_000_000)
        self.factory.assert_called_once_with('console.vast.ai', 443, timeout=10,
                                             context=self.context)
        self.assertEqual(self.context.verify_mode, ssl.CERT_REQUIRED)
        self.assertTrue(self.context.check_hostname)
        self.assertEqual(self.connection.debuglevel, 0)
        method, path, args = self.connection.requests[0]
        self.assertEqual((method, path, args['body']), ('GET', '/api/v0/users/current', None))
        self.assertEqual(args['headers']['Authorization'], 'Bearer ' + SENTINEL)
        self.assertEqual(args['headers']['Accept-Encoding'], 'identity')
        self.assertEqual(self.response.reads, [MAX_BODY + 1])
        self.assertTrue(self.connection.closed)

    def test_offer_post_is_only_fixed_query(self):
        self.response.body = b'{"offers":[]}'
        self.assertEqual(self.client.offers(100), ())
        method, path, args = self.connection.requests[0]
        self.assertEqual((method, path), ('POST', '/api/v0/bundles'))
        self.assertEqual(args['body'], transport.OFFER_BODY)
        self.assertEqual(json.loads(args['body'])['type'], 'on-demand')

    def test_private_boundary_rejects_every_other_route_method_body_and_headers(self):
        exchange = transport._HttpsExchange(self.factory)
        headers = {'Authorization': 'Bearer ' + SENTINEL, 'Content-Type': 'application/json'}
        bad = [('DELETE', '/api/v0/instances/123', None),
               ('GET', 'https://evil.invalid/api/v0/users/current', None),
               ('GET', '/api/v0/users/current?api_key=synthetic', None),
               ('GET', '/api/v0/users/current/', None),
               ('GET', '/api/v0/users/current', b'{}'),
               ('POST', '/api/v0/bundles', b'{}'),
               ('PUT', '/api/v0/bundles', transport.OFFER_BODY)]
        for method, path, body in bad:
            self.assert_opaque(lambda: exchange(method, path, headers, body))
        for h in (dict(headers, Host='evil.invalid'), dict(headers, Authorization='Bearer x\ny')):
            self.assert_opaque(lambda: exchange('GET', '/api/v0/users/current', h, None))
        self.factory.assert_not_called()

    def test_redirect_auth_error_rate_limit_no_follow_or_retry(self):
        for status in (301, 302, 303, 307, 308, 401, 403, 429, 500):
            self.response.status = status
            self.response.headers = {'Location': 'https://evil.invalid/' + SENTINEL}
            self.factory.reset_mock()
            self.assert_opaque(lambda: self.client.account(100))
            self.factory.assert_called_once()
        self.assertEqual(self.response.reads, [])

    def test_body_size_limit_without_length(self):
        self.response.body = b'x' * (MAX_BODY + 1)
        self.assert_opaque(lambda: self.client.account(100))
        self.assertEqual(self.response.reads, [MAX_BODY + 1])
        self.assertTrue(self.connection.closed)

    def test_declared_size_limit_encoding_and_truncation(self):
        for headers in ({'Content-Length': str(MAX_BODY + 1)},
                        {'Content-Length': '-1'}, {'Content-Length': 'garbage'},
                        {'Content-Encoding': 'gzip'}, {'Content-Length': '999'}):
            self.response.headers = headers
            self.assert_opaque(lambda: self.client.account(100))
        self.assertTrue(self.connection.closed)

    def test_invalid_json_and_reflected_secret_never_public(self):
        self.response.body = json.dumps({'error': SENTINEL}).encode()
        output = io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            self.assert_opaque(lambda: self.client.account(100))
        self.assertEqual(output.getvalue(), '')

    def test_connection_timeout_tls_and_read_errors_are_opaque_no_retry(self):
        for error in (TimeoutError(SENTINEL), ssl.SSLCertVerificationError(SENTINEL),
                      OSError(SENTINEL)):
            self.factory.side_effect = error
            self.factory.reset_mock()
            self.assert_opaque(lambda: self.client.account(100))
            self.factory.assert_called_once()
        self.factory.side_effect = None
        self.response.read = Mock(side_effect=TimeoutError(SENTINEL))
        self.assert_opaque(lambda: self.client.account(100))
        self.assertTrue(self.connection.closed)

    def test_tls_factory_failure_is_closed_before_connection(self):
        with patch.object(transport, '_tls_context', side_effect=ssl.SSLError(SENTINEL)):
            self.assert_opaque(lambda: self.client.account(100))
        self.factory.assert_not_called()


class TrustContextTests(unittest.TestCase):
    def test_compiled_in_trust_paths_not_environment_resolved_paths(self):
        # Mock path metadata, never set/read an actual secret or certificate env var.
        paths = Mock(openssl_cafile='/compiled/ca.pem', openssl_capath='/compiled/certs')
        context = Mock()
        context.cert_store_stats.return_value = {'x509_ca': 1}
        with patch.object(transport.ssl, 'SSLContext', return_value=context) as factory, \
                patch.object(transport.ssl, 'get_default_verify_paths', return_value=paths):
            self.assertIs(transport._tls_context(), context)
        factory.assert_called_once_with(ssl.PROTOCOL_TLS_CLIENT)
        self.assertEqual(context.minimum_version, ssl.TLSVersion.TLSv1_2)
        self.assertEqual(context.verify_mode, ssl.CERT_REQUIRED)
        self.assertIs(context.check_hostname, True)
        context.load_verify_locations.assert_called_once_with(
            cafile='/compiled/ca.pem', capath='/compiled/certs')
        context.set_default_verify_paths.assert_not_called()


class RunnerTests(unittest.TestCase):
    def test_default_offline_does_not_consume_secret_or_make_client(self):
        factory, supplier = Mock(), Mock()
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(readonly_runner.main([], _factory=factory,
                                                  _secret_supplier=supplier), 0)
        supplier.assert_not_called()
        factory.assert_not_called()
        self.assertFalse(json.loads(output.getvalue())['live_authorized'])

    def test_optin_with_injected_synthetic_dependencies_only_sanitized_fields(self):
        factory = Mock(return_value=transport.https_client(
            lambda: SENTINEL, _connection_factory=Mock(side_effect=[
                Connection(Response(b'{"balance":6,"email":"DO-NOT-RETURN"}')),
                Connection(Response(b'{"offers":[]}'))])))
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        output = io.StringIO()
        with patch.object(transport, '_tls_context', return_value=context), \
                contextlib.redirect_stdout(output):
            self.assertEqual(readonly_runner.main(['--authorized-readonly'],
                             _factory=factory, _secret_supplier=lambda: SENTINEL,
                             _clock=lambda: 100), 0)
        self.assertNotIn(SENTINEL, output.getvalue())
        self.assertNotIn('DO-NOT-RETURN', output.getvalue())
        result = json.loads(output.getvalue())
        self.assertEqual(result['auto_topup'], 'unknown')
        self.assertEqual(result['pricing_qualification'], 'blocked')
        self.assertFalse(result['live_authorized'])

    def test_runner_exception_only_opaque_status(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(readonly_runner.main(['--authorized-readonly'],
                             _factory=Mock(side_effect=RuntimeError(SENTINEL))), 1)
        self.assertEqual(json.loads(output.getvalue())['status'], 'preflight unavailable')
        self.assertNotIn(SENTINEL, output.getvalue())

    def test_no_key_or_origin_cli_options(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            readonly_runner.main(['--api-key', 'SYNTHETIC'])


if __name__ == '__main__':
    unittest.main()
