"""Default offline; explicit future-authorized read-only invocation only.

No create/destroy/payment or automatic launch qualification. Never run the opt-in
mode until root confirms authorization and secure credential injection scope.
"""
import argparse
import json
import os
import time

from https_readonly import https_client


def _runtime_secret():
    # Only called by ReadOnlyClient after opt-in invocation; never import-time.
    return os.environ['VAST_API_KEY']


def main(argv=None, *, _factory=https_client, _secret_supplier=_runtime_secret,
         _clock=time.time):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--authorized-readonly', action='store_true',
                        help='root-authorized read-only use only; never permits launch')
    args = parser.parse_args(argv)
    if not args.authorized_readonly:
        print('{"mode":"offline","live_authorized":false}')
        return 0
    failed = False
    try:
        client = _factory(_secret_supplier)
        account = client.account(int(_clock()))
        offers = client.offers(int(_clock()))
        # Fixed whitelist: no raw provider text, raw replies, credentials or headers.
        result = {
            'mode': 'readonly-unqualified', 'live_authorized': False,
            'balance_micro_usd': account.balance_micro_usd,
            'balance_observed_s': account.observed_s,
            'auto_topup': 'unknown', 'pricing_qualification': 'blocked',
            'offers': [{'offer_id': row.offer_id,
                        'diagnostic_hourly_micro_usd': row.hourly_micro_usd,
                        'inet_down_mbps': row.inet_down_mbps,
                        'country': row.country, 'datacenter': row.datacenter,
                        'observed_s': row.observed_s,
                        'unit_resolution': 'conflicting-unqualified'} for row in offers],
        }
    except Exception:
        failed = True
    if failed:
        print('{"status":"preflight unavailable","live_authorized":false}')
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
