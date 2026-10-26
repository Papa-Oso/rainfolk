"""One explicitly requested public SDR sample. Metadata then one <=200-row page.

Source-specific bounded transport in the existing Python/Rust architecture;
no retries, cursor loop, full reports, credentials, index writes or publishing.
Default is an offline dry run. Real acquisition requires --public-sample.
"""
import argparse
import hashlib
import json
from pathlib import Path
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone

SOURCE = 'https://services.twdb.texas.gov/arcgis/rest/services/Public/WellReports/FeatureServer/0'
FIELDS = ('WellReportTrackingNumber', 'ProposedUse', 'LatitudeDD', 'LongitudeDD',
          'DateOfWellCompletion', 'BoreholeDepthFt', 'LocationError')
QUERY = (SOURCE + '/query?f=json&where=1%3D1&outFields=' + '%2C'.join(FIELDS)
         + '&returnGeometry=false&orderByFields=WellReportTrackingNumber'
         + '&resultRecordCount=200&resultOffset=0')
SCHEMA_URL = SOURCE + '?f=pjson'
PAGE_BYTES = 512 * 1024
TOTAL_BYTES = 1024 * 1024
TIME_LIMIT = 60
TYPES = dict(zip(FIELDS, ('esriFieldTypeString', 'esriFieldTypeString',
    'esriFieldTypeDouble', 'esriFieldTypeDouble', 'esriFieldTypeDate',
    'esriFieldTypeDouble', 'esriFieldTypeSmallInteger')))


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        raise ValueError('redirect refused; no retry')


def unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('duplicate JSON member')
        result[key] = value
    return result


def decode(raw):
    try:
        return json.loads(raw, object_pairs_hook=unique,
                          parse_constant=lambda _: (_ for _ in ()).throw(ValueError()))
    except (ValueError, UnicodeError):
        raise ValueError('invalid source JSON or duplicate member') from None


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def encoded(value):
    return (json.dumps(value, sort_keys=True, ensure_ascii=False, allow_nan=False) + '\n').encode()


def schema_check(schema):
    if not isinstance(schema, dict) or 'error' in schema:
        code = schema.get('error', {}).get('code') if isinstance(schema, dict) and isinstance(schema.get('error'), dict) else None
        raise ValueError(f'source schema error {code if type(code) is int else "unknown"}; no feature request')
    fields = schema.get('fields')
    if not isinstance(fields, list) or len({f.get('name') for f in fields}) != len(fields):
        raise ValueError('source field schema invalid')
    indexed = {f['name']: f for f in fields}
    if any(indexed.get(k, {}).get('type') != v for k, v in TYPES.items()):
        raise ValueError('source field types changed; no feature request')
    if (schema.get('sourceSpatialReference', {}).get('wkid') != 4326
            or schema.get('dateFieldsTimeReference', {}).get('timeZone') != 'UTC'
            or indexed['LocationError'].get('domain') is not None
            or not schema.get('advancedQueryCapabilities', {}).get('supportsOrderBy')
            or schema.get('maxRecordCount', 0) < 200):
        raise ValueError('source coordinate/date/quality/query schema changed')


def sanitize(response):
    if not isinstance(response, dict) or 'error' in response:
        code = response.get('error', {}).get('code') if isinstance(response, dict) and isinstance(response.get('error'), dict) else None
        raise ValueError(f'source API error {code if type(code) is int else "unknown"}; stopped without retry')
    if set(response) - {'features', 'fields', 'displayFieldName', 'fieldAliases',
            'exceededTransferLimit', 'spatialReference', 'objectIdFieldName',
            'uniqueIdField', 'hasZ', 'hasM', 'geometryType'}:
        raise ValueError('unexpected response metadata; no retention')
    if any(not isinstance(f, dict) or f.get('name') not in set(FIELDS) | {'ObjectId'}
           for f in response.get('fields', [])):
        raise ValueError('unexpected response fields; no retention')
    rows = response.get('features')
    if not isinstance(rows, list) or len(rows) > 200:
        raise ValueError('source row limit or shape invalid')
    cleaned, dropped_oids = [], 0
    for row in rows:
        if not isinstance(row, dict) or set(row) - {'attributes'}:
            raise ValueError('unexpected feature shape; no retention')
        attributes = row.get('attributes')
        # Some ArcGIS servers include an OID automatically. Keep only the seven
        # requested fields; no other unrequested attribute may be saved.
        if not isinstance(attributes, dict) or set(attributes) - set(FIELDS) - {'ObjectId'}:
            raise ValueError('unexpected source attributes; no retention')
        if 'ObjectId' in attributes:
            if type(attributes['ObjectId']) is not int:
                raise ValueError('unexpected source row key type')
            dropped_oids += 1
        cleaned.append({'attributes': {k: attributes[k] for k in FIELDS if k in attributes}})
    flag = response.get('exceededTransferLimit', False)
    if type(flag) is not bool:
        raise ValueError('invalid transfer-limit flag')
    return dict(spatialReference=dict(wkid=4326), exceededTransferLimit=flag,
                features=cleaned), dropped_oids


def capture(directory, opener=None, pause=time.sleep, clock=time.monotonic):
    destination = Path(directory)
    if destination.exists():
        raise ValueError('fresh output directory required; no automatic repeat')
    started = clock()
    opener = opener or urllib.request.build_opener(NoRedirect())
    total, requests = 0, 0

    def read(url):
        nonlocal total, requests
        remaining = TIME_LIMIT - (clock() - started)
        if requests >= 2 or remaining <= 0:
            raise ValueError('request/time budget exhausted')
        requests += 1
        request = urllib.request.Request(url, headers={
            'Accept': 'application/json', 'User-Agent': 'Rainfolk-bounded-SDR-sample/1',
            'Accept-Encoding': 'identity'})
        try:
            with opener.open(request, timeout=min(15, remaining)) as response:
                if response.geturl() != url or response.status != 200:
                    raise ValueError('unexpected response status or URL; no retry')
                chunks, size = [], 0
                while True:
                    if clock() - started >= TIME_LIMIT:
                        raise ValueError('sample time budget exhausted')
                    chunk = response.read(min(65536, PAGE_BYTES - size + 1,
                                              TOTAL_BYTES - total + 1))
                    if not chunk:
                        break
                    size += len(chunk)
                    total += len(chunk)
                    if size > PAGE_BYTES or total > TOTAL_BYTES:
                        raise ValueError('source byte budget exhausted')
                    chunks.append(chunk)
        except urllib.error.HTTPError as error:
            raise ValueError(f'HTTP {error.code}; stopped without retry') from None
        except (urllib.error.URLError, TimeoutError, OSError):
            raise ValueError('source network/timeout failure; stopped without retry') from None
        return b''.join(chunks)

    schema_raw = read(SCHEMA_URL)
    schema_check(decode(schema_raw))
    pause(1)
    raw = read(QUERY)
    response, dropped_oids = sanitize(decode(raw))
    retrieved = datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z')
    capture = dict(schemaVersion=1, kind='rainfolk-texas-sdr-public-capture',
        synthetic=False, sourceUrl=SOURCE, queryUrl=QUERY, retrievedAt=retrieved,
        sourceResponseSha256=digest(raw), sourceSchemaSha256=digest(schema_raw), response=response)
    capture_raw = encoded(capture)
    if len(capture_raw) > TOTAL_BYTES or clock() - started > TIME_LIMIT:
        raise ValueError('capture byte/time budget exhausted')
    evidence = dict(retrievedAt=retrieved, source=SOURCE, queryUrl=QUERY,
        networkRequests=requests, responseBytes=total, inputRows=len(response['features']),
        discardedAutomaticObjectIds=dropped_oids, responseSha256=digest(raw),
        schemaSha256=digest(schema_raw), captureSha256=digest(capture_raw),
        limits=dict(requests=2, rows=200, responseBytes=TOTAL_BYTES, seconds=TIME_LIMIT,
                    timeoutSeconds=15, pacingSeconds=1, retries=0),
        attribution='Texas Water Development Board; Submitted Drillers Reports / TDLR',
        terms=dict(manual='https://www.twdb.texas.gov/groundwater/data/doc/TWRSRS_UserManual.pdf',
            manualRevision='November 2015', publicWorkflowPages=[4, 24],
            policy='https://www.twdb.texas.gov/policies/site/index.asp',
            dataPage='https://www.twdb.texas.gov/groundwater/data/data-well-reports.asp',
            uncertainty='Public workflow evidence; export confidential-filter guarantee and later removal handling unverified; no blanket data license or publication approval.'),
        scope='Local sample only; no full reports, owner/contact values, bulk, index or publication.')
    destination.mkdir(parents=True, exist_ok=False)
    (destination / 'schema.json').write_bytes(schema_raw)
    # Source response already passed the minimum-attribute check. Metadata and
    # possible numeric automatic OIDs are the only discarded response content.
    (destination / 'public-response.json').write_bytes(raw)
    (destination / 'capture.json').write_bytes(capture_raw)
    (destination / 'evidence.json').write_bytes(encoded(evidence))
    return evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--public-sample', action='store_true')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if not args.public_sample:
        print(json.dumps(dict(dryRun=True, schemaUrl=SCHEMA_URL, queryUrl=QUERY,
                              maxRecords=200, maxRequests=2, networkRequests=0)))
        return
    if args.output is None:
        parser.error('--output required for explicit public sample')
    try:
        print(json.dumps(capture(args.output)))
    except ValueError as error:
        print(str(error), file=sys.stderr)
        raise SystemExit(1) from None


if __name__ == '__main__':
    main()
