"""Bounded HTTP transport only; Rust owns well admission and artifact production.

Explicit state monitoring-location pilot, maximum two 1000-row pages. No retries,
observation requests, activation, or LLM calls. Optional USGS_API_KEY is sent
only as X-Api-Key. Existing cache is
reused exactly; remove/replace nothing automatically. stdout is capture JSON.
"""
import argparse
import hashlib
import json
import os
import re
import sys
import time
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

BASE = 'https://api.waterdata.usgs.gov/ogcapi/v1/collections/monitoring-locations/'
FILTERS = {'f': 'json', 'limit': '25', 'agency_code': 'USGS',
           'country_code': 'US', 'site_type_code': 'GW', 'state_code': '22'}
FIRST = BASE + 'items?' + urllib.parse.urlencode(FILTERS)
PAGE_BYTES = 4 * 1024 * 1024
MAX_CAPTURE_BYTES = 8 * 1024 * 1024
PROJECTED_FIELDS = 'id,agency_code,country_code,state_code,county_code,site_type_code,monitoring_location_number,monitoring_location_name,construction_date,well_constructed_depth,hole_constructed_depth,depth_source_code,altitude,altitude_accuracy,altitude_method_code,vertical_datum,vertical_datum_name,horizontal_positional_accuracy_code,horizontal_positional_accuracy,horizontal_position_method_code,horizontal_position_method_name,original_horizontal_datum,original_horizontal_datum_name,aquifer_code,national_aquifer_code,aquifer_type_code,revision_note,revision_created,revision_modified'


def credential():
    value = os.environ.get('USGS_API_KEY', '')
    if value and not re.fullmatch(r'[A-Za-z0-9]{8,256}', value):
        raise ValueError('invalid USGS_API_KEY format; value suppressed')
    return value


def redact(value, key):
    return value.replace(key, '[REDACTED]') if key else value


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        raise ValueError('redirect refused; inspect source metadata before retry')


def page_url(url, next_page=False, filters=None):
    filters = FILTERS if filters is None else filters
    # urlsplit strips some controls: reject them before parsing an authority.
    if (not isinstance(url, str) or len(url) > 8192
            or any(ord(char) < 33 or ord(char) > 126 for char in url)
            or re.search(r'%(?![0-9a-fA-F]{2})', url)):
        raise ValueError('malformed page URL')
    parsed = urllib.parse.urlsplit(url)
    if parsed.scheme != 'https' or parsed.netloc != 'api.waterdata.usgs.gov' or parsed.fragment:
        raise ValueError('next page outside fixed public USGS origin')
    if parsed.path != urllib.parse.urlsplit(BASE + 'items').path:
        raise ValueError('next page outside inventory collection')
    params = urllib.parse.parse_qs(parsed.query, strict_parsing=True, errors='strict')
    if any(params.get(key) != [value] for key, value in filters.items()):
        raise ValueError('next page changed pilot scope')
    if set(params) != set(filters) | ({'cursor'} if next_page else set()):
        raise ValueError('unknown next-page parameters require review')
    if next_page:
        cursors = params['cursor']
        if (len(cursors) != 1 or not 1 <= len(cursors[0]) <= 2048
                or any(char.isspace() or ord(char) < 32 or ord(char) == 127 for char in cursors[0])):
            raise ValueError('malformed cursor')
    # Cursor contents are opaque. Do not decode a token or reconstruct its URL.
    return url


def capture_bytes(value):
    return (json.dumps(value, sort_keys=True, ensure_ascii=False) + '\n').encode('utf-8')


def capture(cache_dir, pages=1, opener=None, pause=time.sleep,
            state='22', limit=25, max_bytes=MAX_CAPTURE_BYTES, rate_millis=1000,
            start_url=None, metrics=None, page_bytes=PAGE_BYTES, properties=None):
    projected = properties is not None
    if projected and properties != PROJECTED_FIELDS:
        raise ValueError('unapproved production projection')
    capture_cap = 64 * 1024 * 1024 if projected else MAX_CAPTURE_BYTES
    if not 1024 <= page_bytes <= (32 * 1024 * 1024 if projected else PAGE_BYTES):
        raise ValueError('invalid page byte cap')
    if pages not in (1, 2):
        raise ValueError('page budget must be 1 or 2')
    if (not isinstance(state, str) or not re.fullmatch(r'[0-9]{2}', state)
            or not 1 <= limit <= (10000 if projected else 1000) or not 1024 <= max_bytes <= capture_cap
            or not 1000 <= rate_millis <= 3600000):
        raise ValueError('invalid state or capture budgets')
    filters = dict(FILTERS, state_code=state, limit=str(limit))
    if projected:
        if pages != 1 or limit != 10000:
            raise ValueError('projected acquisition requires one 10000-row page')
        filters['properties'] = properties
    api_key = credential()
    if metrics is not None:
        metrics.update(networkRequests=0, cacheHits=0, rawResponseBytes=0, quotaHeaders=[])
    opener = opener or urllib.request.build_opener(NoRedirect())
    cache_dir = Path(cache_dir)
    cache_dir.mkdir(parents=True, exist_ok=True)
    url, acquisitions, seen, cursors = start_url or BASE + 'items?' + urllib.parse.urlencode(filters), [], set(), set()
    page_url(url, next_page=start_url is not None, filters=filters)
    if start_url is not None:
        cursors.add(urllib.parse.parse_qs(urllib.parse.urlsplit(url).query)['cursor'][0])
    total_bytes = 0
    for index in range(pages):
        page_url(url, next_page=index > 0 or start_url is not None, filters=filters)
        if url in seen:
            raise ValueError('pagination loop')
        seen.add(url)
        key = hashlib.sha256(url.encode()).hexdigest()
        path = cache_dir / (key + '.json')
        if path.exists():
            if metrics is not None: metrics['cacheHits'] += 1
            if path.stat().st_size > page_bytes * 2:
                raise ValueError('cache byte cap')
            entry = json.loads(path.read_bytes())
            if projected:
                body_path = Path(entry['bodyFile'])
                if not body_path.resolve().is_relative_to(Path(__file__).resolve().parents[1]):
                    raise ValueError('cached body outside repository')
                if body_path.stat().st_size > page_bytes:
                    raise ValueError('cached body byte cap')
                raw = body_path.read_bytes()
            else:
                raw = entry['body'].encode('utf-8')
            if entry['url'] != url or hashlib.sha256(raw).hexdigest() != entry['sha256']:
                raise ValueError('cache integrity failure')
        else:
            if projected and (cache_dir / (key + '.response.json')).exists():
                raise ValueError('captured orphan body needs metadata reconciliation; no refetch')
            if index:
                pause(rate_millis / 1000)
            request = urllib.request.Request(url, headers={
                'Accept': 'application/geo+json', 'User-Agent': 'GEV-bounded-well-inventory/1'})
            if api_key:
                request.add_header('X-Api-Key', api_key)
            if metrics is not None: metrics['networkRequests'] += 1
            try:
                with opener.open(request, timeout=120 if projected else 20) as response:
                    if response.geturl() != url:
                        raise ValueError('unexpected response URL')
                    raw = response.read(min(page_bytes + (0 if projected else 1), max_bytes - total_bytes))
                    headers = getattr(response, 'headers', {})
                    quota = {name: redact(str(headers.get(name)), api_key)[:128] for name in
                             ['X-RateLimit-Limit', 'X-RateLimit-Remaining', 'X-RateLimit-Reset', 'Retry-After']
                             if headers.get(name) is not None}
            except Exception as error:
                if api_key and api_key in str(error):
                    raise RuntimeError(redact(str(error), api_key)) from None
                raise
            if api_key and api_key.encode() in raw:
                raise ValueError('response contains credential; capture refused')
            if projected and len(raw) >= page_bytes:
                raise ValueError('projected response reached byte cap')
            if len(raw) > page_bytes:
                raise ValueError('page byte cap')
            entry = {'url': url, 'retrievedAt': datetime.now(timezone.utc).isoformat(),
                     'sha256': hashlib.sha256(raw).hexdigest(), 'body': raw.decode('utf-8'),
                     'quotaHeaders': quota}
        if api_key and api_key.encode() in raw:
            raise ValueError('cached response contains credential; capture refused')
        if metrics is not None:
            metrics['quotaHeaders'].append(entry.get('quotaHeaders', {}))
        if len(raw) >= max_bytes - total_bytes:
            raise ValueError('total response byte cap')
        if len(raw) > page_bytes:
            raise ValueError('cached page byte cap')
        total_bytes += len(raw)
        if metrics is not None: metrics['rawResponseBytes'] = total_bytes
        if total_bytes > min(max_bytes, capture_cap):
            raise ValueError('total response byte cap')
        body = json.loads(raw)
        if body.get('type') != 'FeatureCollection' or not isinstance(body.get('features'), list):
            raise ValueError('expected inventory FeatureCollection')
        if len(body['features']) > limit:
            raise ValueError('page row cap')
        acquisition = {'collection': 'monitoring-locations', 'url': url,
                       'retrievedAt': entry['retrievedAt'], 'response': body}
        if projected:
            body_path = Path(entry.get('bodyFile', cache_dir / (key + '.response.json')))
            acquisition['rawFile'] = str(body_path.resolve())
            acquisition['rawSha256'] = entry['sha256']
        acquisitions.append(acquisition)
        if len(capture_bytes({'schemaVersion': 1, 'acquisitions': acquisitions})) > min(max_bytes, capture_cap):
            raise ValueError('total capture byte cap')
        links = body.get('links', [])
        if not isinstance(links, list) or any(not isinstance(link, dict) for link in links):
            raise ValueError('malformed pagination links')
        next_links = [link.get('href') for link in links if link.get('rel') == 'next']
        if len(next_links) > 1:
            raise ValueError('ambiguous pagination')
        if next_links:
            next_url = page_url(next_links[0], next_page=True, filters=filters)
            cursor = urllib.parse.parse_qs(urllib.parse.urlsplit(next_url).query)['cursor'][0]
            if next_url in seen or cursor in cursors:
                raise ValueError('pagination loop or repeated cursor')
            cursors.add(cursor)
        if not path.exists():
            if projected:
                if body_path.exists():
                    if hashlib.sha256(body_path.read_bytes()).hexdigest() != entry['sha256']:
                        raise ValueError('orphan body integrity failure')
                else:
                    with body_path.open('xb') as stream:
                        stream.write(raw)
                        stream.flush()
                        os.fsync(stream.fileno())
                entry.pop('body', None)
                entry['bodyFile'] = str(body_path.resolve())
            with path.open('x', encoding='utf-8') as stream:
                json.dump(entry, stream, ensure_ascii=False)
                stream.flush()
                os.fsync(stream.fileno())
        if not next_links or index + 1 == pages:
            break
        url = next_url
    return {'schemaVersion': 1, 'acquisitions': acquisitions}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cache-dir', required=True)
    parser.add_argument('--pages', type=int, choices=(1, 2), default=1)
    parser.add_argument('--state', default='22')
    parser.add_argument('--limit', type=int, default=25)
    parser.add_argument('--max-bytes', type=int, default=MAX_CAPTURE_BYTES)
    parser.add_argument('--rate-millis', type=int, default=1000)
    parser.add_argument('--start-url')
    parser.add_argument('--page-bytes', type=int, default=PAGE_BYTES)
    parser.add_argument('--properties')
    args = parser.parse_args()
    metrics = {}
    try:
        result = capture(args.cache_dir, args.pages, state=args.state,
                         limit=args.limit, max_bytes=args.max_bytes,
                         rate_millis=args.rate_millis, start_url=args.start_url,
                         metrics=metrics, page_bytes=args.page_bytes, properties=args.properties)
        print(capture_bytes(result).decode('utf-8'), end='')
    except Exception as error:
        print('USGS_METRICS ' + json.dumps(metrics), file=sys.stderr)
        print('USGS capture refused: ' + redact(str(error), os.environ.get('USGS_API_KEY', '')), file=sys.stderr)
        raise SystemExit(1) from None
    print('USGS_METRICS ' + json.dumps(metrics), file=sys.stderr)
