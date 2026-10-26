"""One explicitly requested public SDR sample. Metadata then one <=200-row page.

Source-specific bounded transport in the existing Python/Rust architecture;
no retries, cursor loop, full reports, credentials, index writes or publishing.
Default is an offline dry run. Real acquisition requires --public-sample.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
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

SAFE_MESSAGES = {
    'Unable to complete operation.', 'Failed to execute query.',
    'Unable to perform query. Please check your parameters.',
    'Invalid or missing input parameters.', 'Invalid query parameters.',
    'Pagination is not supported.', 'Query operation is not supported.',
    'Token Required', 'Invalid Token', 'Access denied.', 'Forbidden',
    'Too Many Requests', 'Unable to perform query operation.',
    'Total request deadline exceeded.',
}


def safe_message(value):
    """Only recognized generic diagnostics or exact approved field names survive."""
    if not isinstance(value, str):
        return 'Source message not provided.'
    text = ' '.join(value.split())
    if text in SAFE_MESSAGES:
        return text
    for field in (*FIELDS, 'ObjectId'):
        for pattern in ('Invalid field: {}', 'Invalid field: {}.', 'Field {} does not exist.'):
            if text == pattern.format(field):
                return text
    return 'Unrecognized source message omitted.'


class SourceFailure(ValueError):
    def __init__(self, phase, code=None, message=None, details=None, http=False):
        code = code if type(code) is int and 0 <= code <= 9999 else None
        category = ('rate-limit' if code == 429 else
                    'access-denial' if code in (401, 403, 498, 499) else
                    'invalid-query' if code == 400 else
                    'server-error' if code is not None and code >= 500 else 'source-error')
        clean = safe_message(message)
        if clean in ('Token Required', 'Invalid Token', 'Access denied.', 'Forbidden'):
            category = 'access-denial'
        if clean == 'Too Many Requests':
            category = 'rate-limit'
        if clean == 'Total request deadline exceeded.':
            category = 'total-deadline'
        self.info = dict(phase=phase, code=code, category=category, message=clean,
            details=[safe_message(s) for s in details[:3]] if isinstance(details, list) else [],
            httpError=http, automaticRetry=False)
        super().__init__(f'{"HTTP" if http else "API"} {code if code is not None else "unknown"} '
                         f'{category}: {clean}; stopped without retry')


def source_failure(value, phase):
    error = value.get('error') if isinstance(value, dict) else None
    error = error if isinstance(error, dict) else {}
    return SourceFailure(phase, error.get('code'), error.get('message'), error.get('details'))


def request_worker(command, remaining):
    """Kill/wait the HTTP child on total deadline, including connect/slow body."""
    try:
        result = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, timeout=remaining, check=False)
    except subprocess.TimeoutExpired:
        raise SourceFailure('transport', message='Total request deadline exceeded.') from None
    if result.returncode:
        try:
            info = decode(result.stderr)
        except ValueError:
            raise ValueError('HTTP worker failed; diagnostics omitted') from None
        if isinstance(info, dict) and info.get('httpError') is True:
            raise SourceFailure(info.get('phase', 'transport'), info.get('code'),
                                info.get('message'), http=True)
        raise ValueError('source network/timeout failure; stopped without retry')
    if len(result.stdout) > PAGE_BYTES:
        raise ValueError('source byte budget exhausted')
    return result.stdout


def http_worker(phase):
    # Private CLI mode accepts fixed phase names, never arbitrary URLs/hosts.
    url = SCHEMA_URL if phase == 'schema' else QUERY
    request = urllib.request.Request(url, headers={
        'Accept': 'application/json', 'User-Agent': 'Rainfolk-bounded-SDR-sample/1',
        'Accept-Encoding': 'identity'})
    try:
        with urllib.request.build_opener(NoRedirect()).open(request, timeout=15) as response:
            if response.geturl() != url:
                raise ValueError('unexpected response URL; no retry')
            if response.status != 200:
                raise SourceFailure(phase, response.status, http=True)
            raw = response.read(PAGE_BYTES + 1)
            if len(raw) > PAGE_BYTES:
                raise ValueError('source byte budget exhausted')
            return raw
    except urllib.error.HTTPError as error:
        raise SourceFailure(phase, error.code, http=True) from None


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


def audit_encoded(value):
    return (json.dumps(value, sort_keys=True, indent=2, ensure_ascii=False, allow_nan=False) + '\n').encode()


def schema_check(schema):
    if not isinstance(schema, dict) or 'error' in schema:
        raise source_failure(schema, 'schema')
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


def sanitize(response, schema=None):
    if not isinstance(response, dict) or 'error' in response:
        raise source_failure(response, 'query')
    if set(response) - {'features', 'fields', 'displayFieldName', 'fieldAliases',
            'exceededTransferLimit', 'spatialReference', 'objectIdFieldName',
            'uniqueIdField', 'hasZ', 'hasM', 'geometryType'}:
        raise ValueError('unexpected response metadata; no retention')
    if any(not isinstance(f, dict) or f.get('name') not in set(FIELDS) | {'ObjectId'}
           for f in response.get('fields', [])):
        raise ValueError('unexpected response fields; no retention')
    supplied_crs = response.get('spatialReference')
    if supplied_crs is not None:
        if (not isinstance(supplied_crs, dict) or set(supplied_crs) - {'wkid', 'latestWkid'}
                or not supplied_crs or any(type(v) is not int or v != 4326 for v in supplied_crs.values())):
            raise ValueError('contradictory or unsupported response CRS; no rewriting')
        crs_basis = 'source-response; attributes-unverified'
    else:
        schema_check(schema)
        crs_basis = 'validated-layer-schema; attributes-unverified'
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
    return dict(spatialReference=dict(wkid=4326), spatialReferenceBasis=crs_basis, exceededTransferLimit=flag,
                features=cleaned), dropped_oids


def _capture(directory, opener=None, pause=time.sleep, clock=time.monotonic, context=None):
    destination = Path(directory)
    if destination.exists():
        raise ValueError('fresh output directory required; no automatic repeat')
    started = clock()
    total, requests = 0, 0

    def read(url):
        nonlocal total, requests
        remaining = TIME_LIMIT - (clock() - started)
        if requests >= 2 or remaining <= 0:
            raise ValueError('request/time budget exhausted')
        requests += 1
        context['networkRequests'] = requests
        event = dict(url=url, phase='schema' if url == SCHEMA_URL else 'query',
                     requestedAt=datetime.now(timezone.utc).isoformat())
        context['requests'].append(event)
        if opener is None:
            # The parent enforces the remaining whole-capture deadline around
            # a disposable HTTP process, so slow trickle cannot reset the budget.
            try:
                raw = request_worker([sys.executable, '-B', str(Path(__file__).resolve()),
                    '--_http-phase', event['phase']], remaining)
            except SourceFailure as error:
                if error.info['httpError']:
                    event['httpStatus'] = error.info['code']
                raise
            total += len(raw)
            context['responseBytes'] = total
            if total > TOTAL_BYTES:
                raise ValueError('source byte budget exhausted')
            event.update(httpStatus=200, bytes=len(raw), sha256=digest(raw))
            return raw
        request = urllib.request.Request(url, headers={
            'Accept': 'application/json', 'User-Agent': 'Rainfolk-bounded-SDR-sample/1',
            'Accept-Encoding': 'identity'})
        try:
            with opener.open(request, timeout=min(15, remaining)) as response:
                if response.geturl() != url or response.status != 200:
                    if response.geturl() != url:
                        raise ValueError('unexpected response URL; no retry')
                    raise SourceFailure(event['phase'], response.status, http=True)
                event['httpStatus'] = response.status
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
                    context['responseBytes'] = total
                    if size > PAGE_BYTES or total > TOTAL_BYTES:
                        raise ValueError('source byte budget exhausted')
                    chunks.append(chunk)
        except urllib.error.HTTPError as error:
            event['httpStatus'] = error.code
            raise SourceFailure(event['phase'], error.code, http=True) from None
        except (urllib.error.URLError, TimeoutError, OSError):
            raise ValueError('source network/timeout failure; stopped without retry') from None
        raw = b''.join(chunks)
        event.update(bytes=len(raw), sha256=digest(raw))
        return raw

    schema_raw = read(SCHEMA_URL)
    schema = decode(schema_raw)
    schema_check(schema)
    context['schemaRaw'] = schema_raw
    pause(1)
    raw = read(QUERY)
    response, dropped_oids = sanitize(decode(raw), schema)
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
        scope='Local sample only; no full reports, owner/contact values, bulk, index or publication.',
        requests=context['requests'])
    destination.mkdir(parents=True, exist_ok=False)
    (destination / 'schema.json').write_bytes(schema_raw)
    # Source response already passed the minimum-attribute check. Metadata and
    # possible numeric automatic OIDs are the only discarded response content.
    (destination / 'public-response.json').write_bytes(raw)
    (destination / 'capture.json').write_bytes(capture_raw)
    (destination / 'evidence.json').write_bytes(audit_encoded(evidence))
    return evidence


def capture(directory, opener=None, pause=time.sleep, clock=time.monotonic):
    destination = Path(directory)
    if destination.exists():
        raise ValueError('fresh output directory required; no automatic repeat')
    context = dict(source=SOURCE, queryUrl=QUERY, networkRequests=0, responseBytes=0, requests=[])
    try:
        return _capture(directory, opener, pause, clock, context)
    except ValueError as error:
        schema_raw = context.pop('schemaRaw', None)
        info = error.info if isinstance(error, SourceFailure) else dict(
            phase='transport-or-validation', code=None, category='local-validation',
            message='Local transport/schema/validation guard stopped; see bounded exception.',
            automaticRetry=False)
        context.update(status='stopped', error=info, retainedRecords=0,
                       recordedAt=datetime.now(timezone.utc).isoformat())
        destination.mkdir(parents=True, exist_ok=False)
        (destination / 'failure.json').write_bytes(audit_encoded(context))
        if schema_raw is not None:
            (destination / 'schema.json').write_bytes(schema_raw)
        # No error response body, source record or unrecognized message is persisted.
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--public-sample', action='store_true')
    parser.add_argument('--_http-phase', choices=('schema', 'query'), help=argparse.SUPPRESS)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if args._http_phase:
        try:
            sys.stdout.buffer.write(http_worker(args._http_phase))
        except Exception as error:
            info = error.info if isinstance(error, SourceFailure) else dict(
                httpError=False, category='transport-failure', message='HTTP worker stopped.')
            sys.stderr.buffer.write(encoded(info))
            raise SystemExit(1) from None
        return
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
