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
import traceback
import urllib.error
import urllib.request
from datetime import datetime, timezone

SOURCE = 'https://services.twdb.texas.gov/arcgis/rest/services/Public/WellReports/FeatureServer/0'
FIELDS = ('WellReportTrackingNumber', 'ProposedUse', 'LatitudeDD', 'LongitudeDD',
          'DateOfWellCompletion', 'BoreholeDepthFt', 'LocationError')
QUERY = (SOURCE + '/query?f=json&where=1%3D1&outFields=' + '%2C'.join(FIELDS)
         + '&returnGeometry=false&orderByFields=WellReportTrackingNumber'
         + '&resultRecordCount=200&resultOffset=0')
BATCH_FIELDS = tuple(field for field in FIELDS if field != 'DateOfWellCompletion')
BATCH_QUERY = (SOURCE + '/query?f=pjson&where=1%3D1&outFields=' + '%2C'.join(BATCH_FIELDS)
               + '&returnGeometry=false&orderByFields=ObjectId%20ASC'
               + '&resultRecordCount=200&resultOffset=0')
SCHEMA_URL = SOURCE + '?f=pjson'
PAGE_BYTES = 512 * 1024
TOTAL_BYTES = 1024 * 1024
TIME_LIMIT = 60
PROBE_PHASES = ('count', 'id-one', 'projected-one', 'pilot-pretty')
TYPES = dict(zip(FIELDS, ('esriFieldTypeString', 'esriFieldTypeString',
    'esriFieldTypeDouble', 'esriFieldTypeDouble', 'esriFieldTypeDate',
    'esriFieldTypeDouble', 'esriFieldTypeSmallInteger')))

GUARD_MESSAGES = {
    'limitation-evidence': 'date omission requires a 64-digit hexadecimal evidence SHA-256',
    'worker-failure': 'HTTP worker failed; diagnostics omitted',
    'capture-budget': 'capture byte/time budget exhausted',
    'response-crs': 'contradictory or unsupported response CRS; no rewriting',
    'duplicate-json-member': 'duplicate JSON member',
    'output-exists': 'fresh output directory required; no automatic repeat',
    'query-row-budget': 'invalid query row budget',
    'json-invalid': 'invalid source JSON or duplicate member',
    'transfer-limit-type': 'invalid transfer-limit flag',
    'fixed-query-row-limit': 'original query has fixed 200-row limit',
    'redirect-refused': 'redirect refused; no retry',
    'request-time-budget': 'request/time budget exhausted',
    'elapsed-time-budget': 'sample time budget exhausted',
    'response-byte-budget': 'source byte budget exhausted',
    'schema-contract': 'source coordinate/date/quality/query schema changed',
    'schema-fields-shape': 'source field schema invalid',
    'schema-field-types': 'source field types changed; no feature request',
    'network-timeout': 'source network/timeout failure; stopped without retry',
    'response-row-shape-budget': 'source row limit or shape invalid',
    'query-phase': 'unapproved query phase',
    'feature-shape': 'unexpected feature shape; no retention',
    'response-url': 'unexpected response URL; no retry',
    'response-field-projection': 'unexpected response fields; no retention',
    'response-metadata': 'unexpected response metadata; no retention',
    'attribute-projection': 'unexpected source attributes; no retention',
    'source-oid-type': 'unexpected source row key type',
    'projection-fields': 'unapproved or ambiguous minimum-field projection',
    'projection-row-limit': 'projection probes require exactly one row',
    'response-fields-shape': 'invalid response field metadata shape',
    'field-casing-collision': 'ambiguous source field casing',
    'schema-root-shape': 'source schema root must be an object',
    'response-root-shape': 'source response root must be an object',
    'json-nonfinite': 'nonfinite source JSON constant',
    'json-encoding': 'capture cannot be encoded as finite JSON',
    'unexpected-local-validation': 'Unexpected local validation failure; source values omitted.',
}


class GuardFailure(ValueError):
    def __init__(self, code):
        # Codes are literals selected by local code, never source text/values.
        message = GUARD_MESSAGES.get(code, 'Local validation guard failed.')
        self.info = dict(category='local-validation', guardCode=code, message=message,
                         automaticRetry=False)
        super().__init__(f'{code}: {message}')


def local_guard_info(error):
    if isinstance(error, GuardFailure):
        return error.info
    frames = traceback.extract_tb(error.__traceback__)[-1:]
    origin = [dict(module=Path(frame.filename).name, function=frame.name, line=frame.lineno) for frame in frames]
    return dict(category='local-validation', guardCode='unexpected-local-validation', origin=origin,
                message='Unexpected local validation failure; source values omitted.',
                errorClass=type(error).__name__, automaticRetry=False)

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


def query_url(phase='query', limit=200, fields=None):
    """Fixed diagnostic variants on the same layer query; never arbitrary SQL/fields."""
    if type(limit) is not int or not 1 <= limit <= 200:
        raise GuardFailure('query-row-budget')
    if phase == 'schema':
        return SCHEMA_URL
    if phase == 'batch-without-date':
        if limit != 200 or fields is not None:
            raise GuardFailure('query-row-budget')
        return BATCH_QUERY
    if phase == 'query':
        if limit != 200:
            raise GuardFailure('fixed-query-row-limit')
        return QUERY
    base = SOURCE + '/query?f=pjson'
    if phase == 'projection':
        if limit != 1:
            raise GuardFailure('projection-row-limit')
        if (not isinstance(fields, (tuple, list)) or not fields or fields[0] != FIELDS[0]
                or len(fields) > len(FIELDS) or any(not isinstance(field, str) for field in fields)
                or len(set(fields)) != len(fields)
                or any(field not in FIELDS for field in fields)):
            raise GuardFailure('projection-fields')
        return (base + '&where=1%3D1&outFields=' + '%2C'.join(fields)
                + '&returnGeometry=false&resultRecordCount=1')
    if phase == 'count':
        return base + '&where=1%3D1&returnCountOnly=true&returnGeometry=false'
    if phase == 'id-one':
        return base + '&where=1%3D1&outFields=WellReportTrackingNumber&returnGeometry=false&resultRecordCount=1'
    if phase in ('projected-one', 'pilot-pretty'):
        return (SOURCE + '/query?f=' + ('pjson' if phase == 'pilot-pretty' else 'json') + '&where=1%3D1&outFields=' + '%2C'.join(FIELDS)
                + '&returnGeometry=false&resultRecordCount=' + str(1 if phase == 'projected-one' else limit))
    raise GuardFailure('query-phase')


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
            raise GuardFailure('worker-failure') from None
        if isinstance(info, dict) and isinstance(info.get('guardCode'), str):
            raise GuardFailure(info['guardCode'] if info['guardCode'] in GUARD_MESSAGES else 'worker-internal-guard')
        if isinstance(info, dict) and info.get('httpError') is True:
            raise SourceFailure(info.get('phase', 'transport'), info.get('code'),
                                info.get('message'), http=True)
        raise GuardFailure('network-timeout')
    if len(result.stdout) > PAGE_BYTES:
        raise GuardFailure('response-byte-budget')
    return result.stdout


def http_worker(phase, limit=200, fields=None):
    # Private CLI mode accepts fixed phase names, never arbitrary URLs/hosts.
    url = query_url(phase, limit, fields)
    request = urllib.request.Request(url, headers={
        'Accept': 'application/json', 'User-Agent': 'Rainfolk-bounded-SDR-sample/1',
        'Accept-Encoding': 'identity'})
    try:
        with urllib.request.build_opener(NoRedirect()).open(request, timeout=15) as response:
            if response.geturl() != url:
                raise GuardFailure('response-url')
            if response.status != 200:
                raise SourceFailure(phase, response.status, http=True)
            raw = response.read(PAGE_BYTES + 1)
            if len(raw) > PAGE_BYTES:
                raise GuardFailure('response-byte-budget')
            return raw
    except urllib.error.HTTPError as error:
        raise SourceFailure(phase, error.code, http=True) from None


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        raise GuardFailure('redirect-refused')


def unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise GuardFailure('duplicate-json-member')
        result[key] = value
    return result


def decode(raw):
    try:
        return json.loads(raw, object_pairs_hook=unique,
                          parse_constant=lambda _: (_ for _ in ()).throw(GuardFailure('json-nonfinite')))
    except GuardFailure:
        raise
    except (ValueError, UnicodeError):
        raise GuardFailure('json-invalid') from None


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def encoded(value):
    try:
        return (json.dumps(value, sort_keys=True, ensure_ascii=False, allow_nan=False) + '\n').encode()
    except (ValueError, TypeError):
        raise GuardFailure('json-encoding') from None


def audit_encoded(value):
    return (json.dumps(value, sort_keys=True, indent=2, ensure_ascii=False, allow_nan=False) + '\n').encode()


def schema_check(schema, ordered_batch=False):
    if not isinstance(schema, dict):
        raise GuardFailure('schema-root-shape')
    if 'error' in schema:
        raise source_failure(schema, 'schema')
    fields = schema.get('fields')
    if (not isinstance(fields, list) or any(not isinstance(f, dict) or not isinstance(f.get('name'), str) for f in fields)
            or len({f['name'] for f in fields}) != len(fields)):
        raise GuardFailure('schema-fields-shape')
    indexed = {f['name']: f for f in fields}
    if any(indexed.get(k, {}).get('type') != v for k, v in TYPES.items()):
        raise GuardFailure('schema-field-types')
    if (any(not isinstance(schema.get(k), dict) for k in
            ('sourceSpatialReference', 'dateFieldsTimeReference', 'advancedQueryCapabilities'))
            or type(schema.get('maxRecordCount')) is not int):
        raise GuardFailure('schema-contract')
    if (schema['sourceSpatialReference'].get('wkid') != 4326
            or schema['dateFieldsTimeReference'].get('timeZone') != 'UTC'
            or indexed['LocationError'].get('domain') is not None
            or not schema.get('advancedQueryCapabilities', {}).get('supportsOrderBy')
            or schema.get('maxRecordCount', 0) < 200):
        raise GuardFailure('schema-contract')
    if ordered_batch and (schema.get('objectIdField') != 'ObjectId'
            or indexed.get('ObjectId', {}).get('type') != 'esriFieldTypeOID'
            or indexed.get('ObjectId', {}).get('nullable') is not False
            or schema['advancedQueryCapabilities'].get('supportsOrderBy') is not True
            or schema['advancedQueryCapabilities'].get('supportsPagination') is not True):
        raise GuardFailure('schema-contract')


def sanitize(response, schema=None, requested_fields=FIELDS):
    if requested_fields not in (FIELDS, BATCH_FIELDS):
        raise GuardFailure('projection-fields')
    if not isinstance(response, dict):
        raise GuardFailure('response-root-shape')
    if 'error' in response:
        raise source_failure(response, 'query')
    if set(response) - {'features', 'fields', 'displayFieldName', 'fieldAliases',
            'exceededTransferLimit', 'spatialReference', 'objectIdFieldName',
            'globalIdFieldName', 'uniqueIdField', 'hasZ', 'hasM', 'geometryType'}:
        raise GuardFailure('response-metadata')
    response_fields = response.get('fields', [])
    if not isinstance(response_fields, list):
        raise GuardFailure('response-fields-shape')
    if any(not isinstance(f, dict) or not isinstance(f.get('name'), str) or f.get('name') not in set(requested_fields) | {'ObjectId'}
           for f in response_fields):
        raise GuardFailure('response-field-projection')
    supplied_crs = response.get('spatialReference')
    if supplied_crs is not None:
        if (not isinstance(supplied_crs, dict) or set(supplied_crs) - {'wkid', 'latestWkid'}
                or not supplied_crs or any(type(v) is not int or v != 4326 for v in supplied_crs.values())):
            raise GuardFailure('response-crs')
        crs_basis = 'source-response; attributes-unverified'
    else:
        schema_check(schema)
        crs_basis = 'validated-layer-schema; attributes-unverified'
    rows = response.get('features')
    if not isinstance(rows, list) or len(rows) > 200:
        raise GuardFailure('response-row-shape-budget')
    cleaned, dropped_oids = [], 0
    for row in rows:
        if not isinstance(row, dict) or set(row) - {'attributes', 'geometry'} or row.get('geometry') is not None:
            raise GuardFailure('feature-shape')
        attributes = row.get('attributes')
        # Some ArcGIS servers include an OID automatically. Keep only the selected
        # requested fields; no other unrequested attribute may be saved.
        if not isinstance(attributes, dict) or set(attributes) - set(requested_fields) - {'ObjectId'}:
            raise GuardFailure('attribute-projection')
        if 'ObjectId' in attributes:
            if type(attributes['ObjectId']) is not int:
                raise GuardFailure('source-oid-type')
            dropped_oids += 1
        cleaned.append({'attributes': {k: attributes[k] for k in requested_fields if k in attributes}})
    flag = response.get('exceededTransferLimit', False)
    if type(flag) is not bool:
        raise GuardFailure('transfer-limit-type')
    return dict(spatialReference=dict(wkid=4326), spatialReferenceBasis=crs_basis, exceededTransferLimit=flag,
                features=cleaned), dropped_oids


def _capture(directory, opener=None, pause=time.sleep, clock=time.monotonic, context=None,
             date_omission_evidence_sha256=None):
    destination = Path(directory)
    if destination.exists():
        raise GuardFailure('output-exists')
    started = clock()
    total, requests = 0, 0
    query = BATCH_QUERY if date_omission_evidence_sha256 is not None else QUERY

    def read(url):
        nonlocal total, requests
        remaining = TIME_LIMIT - (clock() - started)
        if requests >= 2 or remaining <= 0:
            raise GuardFailure('request-time-budget')
        requests += 1
        context['networkRequests'] = requests
        phase = ('schema' if url == SCHEMA_URL else
                 'batch-without-date' if url == BATCH_QUERY else 'query')
        event = dict(url=url, phase=phase,
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
                raise GuardFailure('response-byte-budget')
            event.update(httpStatus=200, bytes=len(raw), sha256=digest(raw))
            return raw
        request = urllib.request.Request(url, headers={
            'Accept': 'application/json', 'User-Agent': 'Rainfolk-bounded-SDR-sample/1',
            'Accept-Encoding': 'identity'})
        try:
            with opener.open(request, timeout=min(15, remaining)) as response:
                if response.geturl() != url or response.status != 200:
                    if response.geturl() != url:
                        raise GuardFailure('response-url')
                    raise SourceFailure(event['phase'], response.status, http=True)
                event['httpStatus'] = response.status
                chunks, size = [], 0
                while True:
                    if clock() - started >= TIME_LIMIT:
                        raise GuardFailure('elapsed-time-budget')
                    chunk = response.read(min(65536, PAGE_BYTES - size + 1,
                                              TOTAL_BYTES - total + 1))
                    if not chunk:
                        break
                    size += len(chunk)
                    total += len(chunk)
                    context['responseBytes'] = total
                    if size > PAGE_BYTES or total > TOTAL_BYTES:
                        raise GuardFailure('response-byte-budget')
                    chunks.append(chunk)
        except urllib.error.HTTPError as error:
            event['httpStatus'] = error.code
            raise SourceFailure(event['phase'], error.code, http=True) from None
        except (urllib.error.URLError, TimeoutError, OSError):
            raise GuardFailure('network-timeout') from None
        raw = b''.join(chunks)
        event.update(bytes=len(raw), sha256=digest(raw))
        return raw

    schema_raw = read(SCHEMA_URL)
    schema = decode(schema_raw)
    schema_check(schema, ordered_batch=date_omission_evidence_sha256 is not None)
    context['schemaRaw'] = schema_raw
    pause(1)
    raw = read(query)
    response, dropped_oids = sanitize(decode(raw), schema,
        BATCH_FIELDS if date_omission_evidence_sha256 is not None else FIELDS)
    retrieved = datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z')
    capture = dict(schemaVersion=1, kind='rainfolk-texas-sdr-public-capture',
        synthetic=False, sourceUrl=SOURCE, queryUrl=query, retrievedAt=retrieved,
        sourceResponseSha256=digest(raw), sourceSchemaSha256=digest(schema_raw), response=response)
    if date_omission_evidence_sha256 is not None:
        capture.update(fieldLimitations={'DateOfWellCompletion': 'source-query-api-400'},
                       fieldLimitationsEvidenceSha256=date_omission_evidence_sha256)
    capture_raw = encoded(capture)
    if len(capture_raw) > TOTAL_BYTES or clock() - started > TIME_LIMIT:
        raise GuardFailure('capture-budget')
    evidence = dict(retrievedAt=retrieved, source=SOURCE, queryUrl=query,
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
    if date_omission_evidence_sha256 is not None:
        evidence.update(fieldLimitations=capture['fieldLimitations'],
            fieldLimitationsEvidenceSha256=date_omission_evidence_sha256,
            page=dict(offset=0, requestedRows=200, returnedRows=len(response['features']),
                orderByFields='ObjectId ASC', orderingVerification='requested-source-order',
                exceededTransferLimit=response['exceededTransferLimit'], complete=False,
                automaticContinuation=False))
    destination.mkdir(parents=True, exist_ok=False)
    (destination / 'schema.json').write_bytes(schema_raw)
    # Source response already passed the minimum-attribute check. Metadata and
    # possible numeric automatic OIDs are the only discarded response content.
    (destination / 'public-response.json').write_bytes(raw)
    (destination / 'capture.json').write_bytes(capture_raw)
    (destination / 'evidence.json').write_bytes(audit_encoded(evidence))
    return evidence


def limitation_evidence(value):
    if value is not None and (not isinstance(value, str) or len(value) != 64
            or any(c not in '0123456789abcdefABCDEF' for c in value)):
        raise GuardFailure('limitation-evidence')
    return value.lower() if value is not None else None


def capture(directory, opener=None, pause=time.sleep, clock=time.monotonic,
            date_omission_evidence_sha256=None):
    date_omission_evidence_sha256 = limitation_evidence(date_omission_evidence_sha256)
    destination = Path(directory)
    if destination.exists():
        raise GuardFailure('output-exists')
    context = dict(source=SOURCE, queryUrl=BATCH_QUERY if date_omission_evidence_sha256 is not None else QUERY,
                   networkRequests=0, responseBytes=0, requests=[])
    try:
        return _capture(directory, opener, pause, clock, context, date_omission_evidence_sha256)
    except (ValueError, TypeError, KeyError, AttributeError, RecursionError, OverflowError) as error:
        schema_raw = context.pop('schemaRaw', None)
        info = error.info if isinstance(error, SourceFailure) else local_guard_info(error)
        context.update(status='stopped', error=info, retainedRecords=0,
                       recordedAt=datetime.now(timezone.utc).isoformat())
        destination.mkdir(parents=True, exist_ok=False)
        (destination / 'failure.json').write_bytes(audit_encoded(context))
        if schema_raw is not None:
            (destination / 'schema.json').write_bytes(schema_raw)
        # No error response body, source record or unrecognized message is persisted.
        if isinstance(error, (GuardFailure, SourceFailure)):
            raise
        raise GuardFailure('unexpected-local-validation') from None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--public-sample', action='store_true')
    parser.add_argument('--date-omission-evidence-sha256',
                        help='select the fixed six-field 200-row page, citing retained date-query failure evidence')
    parser.add_argument('--_http-phase', choices=('schema', 'query', 'projection', 'batch-without-date', *PROBE_PHASES), help=argparse.SUPPRESS)
    parser.add_argument('--_record-limit', type=int, default=200, help=argparse.SUPPRESS)
    parser.add_argument('--_fields', help=argparse.SUPPRESS)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if args._http_phase:
        try:
            sys.stdout.buffer.write(http_worker(args._http_phase, args._record_limit,
                args._fields.split(',') if args._fields is not None else None))
        except Exception as error:
            info = error.info if isinstance(error, SourceFailure) else local_guard_info(error)
            sys.stderr.buffer.write(encoded(info))
            raise SystemExit(1) from None
        return
    if not args.public_sample:
        try:
            evidence_hash = limitation_evidence(args.date_omission_evidence_sha256)
        except GuardFailure as error:
            print(json.dumps(error.info), file=sys.stderr)
            raise SystemExit(1) from None
        print(json.dumps(dict(dryRun=True, schemaUrl=SCHEMA_URL,
            queryUrl=BATCH_QUERY if evidence_hash is not None else QUERY,
            maxRecords=200, maxRequests=2, maxResponseBytes=PAGE_BYTES,
            maxTotalResponseBytes=TOTAL_BYTES, maxSeconds=TIME_LIMIT, retries=0,
            automaticContinuation=False, networkRequests=0,
            fieldLimitations={'DateOfWellCompletion': 'source-query-api-400'} if evidence_hash else {},
            fieldLimitationsEvidenceSha256=evidence_hash)))
        return
    if args.output is None:
        parser.error('--output required for explicit public sample')
    try:
        print(json.dumps(capture(args.output,
            date_omission_evidence_sha256=args.date_omission_evidence_sha256)))
    except ValueError as error:
        info = error.info if isinstance(error, SourceFailure) else local_guard_info(error)
        print(json.dumps(info, sort_keys=True), file=sys.stderr)
        raise SystemExit(1) from None


if __name__ == '__main__':
    main()
