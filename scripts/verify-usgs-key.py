"""One fixed one-row credential check. No retries, redirects, or saved body."""
import importlib.util
import json
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import Request, build_opener

spec = importlib.util.spec_from_file_location('transport', Path(__file__).with_name('capture-usgs-inventory.py'))
transport = importlib.util.module_from_spec(spec)
spec.loader.exec_module(transport)


def main():
    try:
        key = transport.credential()
        if not key:
            print(json.dumps({'error': 'protected key not loaded', 'requests': 0}))
            return 1
        url = transport.FIRST.replace('limit=25', 'limit=1')
        request = Request(url, headers={'Accept': 'application/geo+json',
            'User-Agent': 'GEV-bounded-well-inventory/1', 'X-Api-Key': key})
        try:
            response = build_opener(transport.NoRedirect()).open(request, timeout=20)
        except HTTPError as error:
            response = error
        with response:
            headers = {name: transport.redact(str(response.headers.get(name)), key)[:128]
                for name in ['X-RateLimit-Limit', 'X-RateLimit-Remaining',
                             'X-RateLimit-Reset', 'Retry-After']
                if response.headers.get(name) is not None}
            status = response.code
            response.read(65536)  # Bounded discard; never log or persist the body.
        print(json.dumps({'requests': 1, 'status': status, 'rateHeaders': headers}))
        return 0 if status == 200 else 1
    except Exception as error:
        # Exception messages can contain credentials; expose type only.
        print(json.dumps({'errorType': type(error).__name__, 'error': 'verification failed; details suppressed'}))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
