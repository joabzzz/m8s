"""Opt-in fixed-origin HTTPS boundary; importing this module does no I/O.

Only https_client's sanitized ReadOnlyClient surface is supported publicly.
No requests/urllib, environment proxy discovery, redirect handler or retries.
"""
import http.client
import json
import ssl

from preflight import MAX_BODY, PreflightError, ReadOnlyClient, decode, require

HOST = 'console.vast.ai'
PORT = 443
TIMEOUT_S = 10
OFFER_BODY = json.dumps({
    'limit': 10, 'type': 'on-demand', 'verified': {'eq': True},
    'rentable': {'eq': True}, 'rented': {'eq': False}}).encode()


def _tls_context():
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.verify_mode = ssl.CERT_REQUIRED
    context.check_hostname = True
    # Use compiled-in trust paths, NOT SSL_CERT_FILE / SSL_CERT_DIR overrides.
    # Never call set_default_verify_paths/create_default_context here.
    paths = ssl.get_default_verify_paths()
    context.load_verify_locations(cafile=paths.openssl_cafile,
                                  capath=paths.openssl_capath)
    require(context.cert_store_stats()['x509_ca'] > 0)
    return context


class _HttpsExchange:
    __slots__ = ('__factory',)

    def __init__(self, factory):
        self.__factory = factory

    def __repr__(self):
        return '<HTTPS read-only transport redacted>'

    def __getstate__(self):
        raise PreflightError()

    def __call__(self, method, path, headers, body):
        failed = False
        connection = None
        try:
            # Validate before constructing a connection or handling a request.
            require((method, path, body) in (
                ('GET', '/api/v0/users/current', None),
                ('POST', '/api/v0/bundles', OFFER_BODY)))
            require(type(headers) is dict and set(headers) == {
                'Authorization', 'Content-Type'})
            auth = headers['Authorization']
            require(type(auth) is str and auth.startswith('Bearer '))
            require(8 <= len(auth) <= 4103 and all(33 <= ord(c) <= 126 for c in auth[7:]))
            require(headers['Content-Type'] == 'application/json')
            context = _tls_context()
            connection = self.__factory(HOST, PORT, timeout=TIMEOUT_S, context=context)
            # http.client has no automatic redirect, proxy or HTTP retry machinery.
            connection.set_debuglevel(0)
            connection.request(method, path, body=body, headers={
                'Authorization': auth, 'Content-Type': 'application/json',
                'Accept': 'application/json', 'Accept-Encoding': 'identity',
                'Connection': 'close'})
            response = connection.getresponse()
            # Do not inspect Location or read error/redirect bodies.
            require(type(response.status) is int and response.status == 200)
            encoding = response.getheader('Content-Encoding')
            require(encoding in (None, 'identity'))
            length = response.getheader('Content-Length')
            if length is not None:
                require(type(length) is str and length.isascii() and length.isdecimal())
                require(int(length) <= MAX_BODY)
            reply = response.read(MAX_BODY + 1)
            require(type(reply) is bytes and len(reply) <= MAX_BODY)
            if length is not None:
                require(len(reply) == int(length))
            # Check JSON/envelope before returning anything across private boundary.
            decode(reply)
        except Exception:
            failed = True
        finally:
            if connection is not None:
                try:
                    connection.close()
                except Exception:
                    failed = True
        if failed:
            # Outside handler, no raw exception in __context__/__cause__.
            raise PreflightError()
        return 200, reply


def https_client(secret_supplier, *, _connection_factory=http.client.HTTPSConnection):
    """Construct only; does not obtain secret or connect until account/offers call.

    _connection_factory is a trusted TEST injection seam, never CLI/env configured.
    TLS uses compiled-in CA paths; unavailable trust fails closed. Per-I/O socket
    timeout is fixed at 10s; this is NOT an independent whole-process watchdog.
    """
    return ReadOnlyClient(_HttpsExchange(_connection_factory), secret_supplier)
