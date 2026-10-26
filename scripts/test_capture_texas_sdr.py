"""Offline transport checks using fake public responses, never a live endpoint."""
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.error
import urllib.parse
from unittest import mock

SCRIPT = Path(__file__).with_name('capture-texas-sdr.py')
SCRATCH = SCRIPT.parents[1] / '.temp-gev' / 'sdr-transport-tests'
SCRATCH.mkdir(parents=True, exist_ok=True)
spec = importlib.util.spec_from_file_location('capture_texas_sdr', SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def schema():
    return dict(fields=[dict(name=k, type=v, domain=None) for k, v in module.TYPES.items()]
                + [dict(name='ObjectId', type='esriFieldTypeOID', nullable=False)], objectIdField='ObjectId',
        sourceSpatialReference=dict(wkid=4326), dateFieldsTimeReference=dict(timeZone='UTC'),
        advancedQueryCapabilities=dict(supportsOrderBy=True, supportsPagination=True, supportsSqlExpression=True), maxRecordCount=2000)


def page(rows=2):
    return dict(exceededTransferLimit=True, features=[dict(attributes=dict(
        WellReportTrackingNumber=str(i).zfill(15), LatitudeDD=30.25, LongitudeDD=-97.75,
        ProposedUse=None, DateOfWellCompletion=None, BoreholeDepthFt=None, LocationError=i))
        for i in range(rows)])


def batch_page(rows=200):
    result = page(rows)
    for row in result['features']:
        del row['attributes']['DateOfWellCompletion']
    return result


class Response(io.BytesIO):
    status = 200

    def __init__(self, raw, url):
        super().__init__(raw)
        self.url = url

    def geturl(self):
        return self.url


class Opener:
    def __init__(self, replies):
        self.replies = list(replies)
        self.urls = []

    def open(self, request, timeout):
        self.urls.append(request.full_url)
        assert 0 < timeout <= 15
        assert 'Authorization' not in request.headers
        assert 'Cookie' not in request.headers
        reply = self.replies.pop(0)
        if isinstance(reply, Exception):
            raise reply
        return Response(reply if isinstance(reply, bytes) else module.encoded(reply), request.full_url)


class CaptureTests(unittest.TestCase):
    def run_capture(self, replies, **kwargs):
        with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
            target = Path(directory) / 'sample'
            opener = Opener(replies)
            evidence = module.capture(target, opener=opener, pause=lambda _: None, **kwargs)
            return evidence, module.decode((target / 'capture.json').read_bytes()), opener.urls

    def test_one_bounded_page_metadata_first_and_explicit_public_marker(self):
        evidence, capture, urls = self.run_capture([schema(), page(200)])
        self.assertEqual(urls, [module.SCHEMA_URL, module.QUERY])
        self.assertEqual(evidence['networkRequests'], 2)
        self.assertEqual(evidence['inputRows'], 200)
        self.assertFalse(capture['synthetic'])
        self.assertEqual(capture['kind'], 'rainfolk-texas-sdr-public-capture')
        self.assertEqual(set(capture['response']['features'][0]['attributes']), set(module.FIELDS))
        self.assertNotIn('outFields=*', urls[1])
        self.assertIn('returnGeometry=false', urls[1])

    def test_schema_change_stops_before_feature_request(self):
        changed = schema()
        changed['fields'][0]['type'] = 'esriFieldTypeDouble'
        opener = Opener([changed, page()])
        with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
            target = Path(directory) / 'sample'
            with self.assertRaises(ValueError):
                module.capture(target, opener=opener, pause=lambda _: None)
            self.assertEqual(len(opener.urls), 1)
            self.assertEqual(module.decode((target / 'failure.json').read_bytes())['retainedRecords'], 0)
            self.assertFalse((target / 'capture.json').exists())

    def test_rate_and_access_denial_have_no_retry_or_saved_body(self):
        for code in (401, 403, 429):
            opener = Opener([schema(), urllib.error.HTTPError(module.QUERY, code, '', {}, None)])
            with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
                target = Path(directory) / 'sample'
                with self.assertRaisesRegex(ValueError, f'HTTP {code}'):
                    module.capture(target, opener=opener, pause=lambda _: None)
                self.assertEqual(len(opener.urls), 2)
                failure = module.decode((target / 'failure.json').read_bytes())
                self.assertEqual(failure['error']['code'], code)
                self.assertEqual(failure['error']['category'], 'rate-limit' if code == 429 else 'access-denial')
                self.assertFalse((target / 'capture.json').exists())
                self.assertFalse((target / 'public-response.json').exists())

    def test_byte_limit_and_time_budget_stop_without_capture(self):
        with self.assertRaisesRegex(ValueError, 'byte budget'):
            self.run_capture([b' ' * (module.PAGE_BYTES + 1)])
        ticks = iter([0, 0, 61])
        with self.assertRaisesRegex(ValueError, 'time budget'):
            self.run_capture([schema(), page()], clock=lambda: next(ticks))

    def test_row_limit_private_attributes_and_geometry_refused(self):
        too_many = page(201)
        private = page()
        private['features'][0]['attributes']['WellOwner'] = 'PRIVATE-SENTINEL'
        geometry = page()
        geometry['features'][0]['geometry'] = dict(x=1, y=2)
        for bad in (too_many, private, geometry, dict(error=dict(code=403))):
            with self.assertRaises(ValueError) as error:
                self.run_capture([schema(), bad])
            self.assertNotIn('PRIVATE-SENTINEL', str(error.exception))

    def test_automatic_oid_is_removed_from_sanitized_capture(self):
        source = page()
        source['features'][0]['attributes']['ObjectId'] = 123
        evidence, capture, _ = self.run_capture([schema(), source])
        self.assertEqual(evidence['discardedAutomaticObjectIds'], 1)
        self.assertNotIn('ObjectId', capture['response']['features'][0]['attributes'])

    def test_documented_global_id_metadata_and_null_geometry_are_allowed(self):
        source = page()
        source['globalIdFieldName'] = ''
        source['features'][0]['geometry'] = None
        _, capture, _ = self.run_capture([schema(), source])
        self.assertEqual(len(capture['response']['features']), 2)
        self.assertNotIn('geometry', capture['response']['features'][0])

    def test_diagnostic_query_variants_are_finite_minimum_field_and_bounded(self):
        for phase in module.PROBE_PHASES:
            parsed = urllib.parse.urlsplit(module.query_url(phase, 199))
            self.assertEqual(parsed.scheme, 'https')
            self.assertEqual(parsed.netloc, 'services.twdb.texas.gov')
            self.assertEqual(parsed.path, urllib.parse.urlsplit(module.SOURCE).path + '/query')
            params = urllib.parse.parse_qs(parsed.query)
            self.assertEqual(params['returnGeometry'], ['false'])
            self.assertNotIn('orderByFields', params)
            self.assertNotIn('resultOffset', params)
            if phase == 'count':
                self.assertEqual(params['returnCountOnly'], ['true'])
                self.assertNotIn('outFields', params)
            else:
                self.assertLessEqual(int(params['resultRecordCount'][0]), 199)
                self.assertTrue(set(params['outFields'][0].split(',')).issubset(module.FIELDS))
        for limit in (0, 201, True, '200'):
            with self.assertRaises(ValueError):
                module.query_url('pilot-pretty', limit)
        with self.assertRaises(ValueError):
            module.query_url('unapproved')
        with self.assertRaises(ValueError):
            module.query_url('query', 1)

    def test_duplicate_json_members_and_nonfinite_numbers_refused(self):
        for bad in (b'{"features":[],"features":[]}', b'{"a":NaN}'):
            with self.assertRaises(module.GuardFailure) as error:
                module.decode(bad)
            self.assertIn(error.exception.info['guardCode'], ('duplicate-json-member', 'json-nonfinite'))

    def test_each_local_guard_persists_a_stable_value_free_code(self):
        cases = []
        private = page()
        private['features'][0]['attributes']['WellOwner'] = 'PRIVATE-SENTINEL'
        cases.append((private, 'attribute-projection'))
        bad_metadata = page()
        bad_metadata['unreviewed'] = 'PRIVATE-SENTINEL'
        cases.append((bad_metadata, 'response-metadata'))
        bad_fields = page()
        bad_fields['fields'] = None
        cases.append((bad_fields, 'response-fields-shape'))
        for response, code in cases:
            opener = Opener([schema(), response])
            with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
                target = Path(directory) / 'sample'
                with self.assertRaises(module.GuardFailure):
                    module.capture(target, opener=opener, pause=lambda _: None)
                raw = (target / 'failure.json').read_text()
                saved = json.loads(raw)
                self.assertEqual(saved['error']['guardCode'], code)
                self.assertEqual(saved['error']['category'], 'local-validation')
                self.assertNotIn('PRIVATE-SENTINEL', raw)
        unknown = module.local_guard_info(ValueError('PRIVATE-SENTINEL'))
        self.assertEqual(unknown['guardCode'], 'unexpected-local-validation')
        self.assertNotIn('PRIVATE-SENTINEL', json.dumps(unknown))

    def test_projection_probe_rejects_unknown_fields_duplicates_and_large_limits(self):
        fields = ('WellReportTrackingNumber', 'LatitudeDD', 'LongitudeDD')
        params = urllib.parse.parse_qs(urllib.parse.urlsplit(module.query_url('projection', 1, fields)).query)
        self.assertEqual(params['outFields'], [','.join(fields)])
        self.assertEqual(params['resultRecordCount'], ['1'])
        for bad in (['WellOwner'], ['WellReportTrackingNumber', 'WellOwner'],
                    ['WellReportTrackingNumber', 'LatitudeDD', 'LatitudeDD'],
                    ['WellReportTrackingNumber', {}]):
            with self.assertRaises(module.GuardFailure):
                module.query_url('projection', 1, bad)
        with self.assertRaises(module.GuardFailure):
            module.query_url('projection', 2, fields)

    def test_api_error_retains_only_numeric_code(self):
        for check in (module.sanitize, module.schema_check):
            with self.assertRaisesRegex(ValueError, '403') as error:
                check(dict(error=dict(code=403, message='PRIVATE-SENTINEL', details=['PRIVATE-SENTINEL'])))
            self.assertNotIn('PRIVATE-SENTINEL', str(error.exception))

    def test_bounded_safe_api_diagnostics_are_persisted_without_record_values(self):
        error = dict(error=dict(code=400, message='Unable to complete operation.', details=[
            'Invalid field: LatitudeDD', 'PRIVATE-SENTINEL', 'PRIVATE-ADDRESS-SENTINEL', 'omitted-extra-detail']),
            features=[dict(attributes=dict(WellOwner='PRIVATE-SENTINEL'))])
        opener = Opener([schema(), error])
        with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
            target = Path(directory) / 'sample'
            with self.assertRaises(module.SourceFailure):
                module.capture(target, opener=opener, pause=lambda _: None)
            raw = (target / 'failure.json').read_text()
            failure = json.loads(raw)
            self.assertEqual(len(opener.urls), 2)
            self.assertEqual(failure['error']['code'], 400)
            self.assertEqual(failure['error']['category'], 'invalid-query')
            self.assertEqual(failure['error']['message'], 'Unable to complete operation.')
            self.assertEqual(failure['error']['details'], ['Invalid field: LatitudeDD',
                'Unrecognized source message omitted.', 'Unrecognized source message omitted.'])
            self.assertNotIn('PRIVATE-SENTINEL', raw)
            self.assertNotIn('PRIVATE-ADDRESS-SENTINEL', raw)
            self.assertNotIn('features', failure)
            self.assertFalse((target / 'public-response.json').exists())
            self.assertTrue((target / 'schema.json').exists())

    def test_embedded_denials_stop_without_retry(self):
        for code in (401, 403, 429, 498, 499):
            opener = Opener([schema(), dict(error=dict(code=code))])
            with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
                target = Path(directory) / 'sample'
                with self.assertRaises(module.SourceFailure):
                    module.capture(target, opener=opener, pause=lambda _: None)
                self.assertEqual(len(opener.urls), 2)
                failure = module.decode((target / 'failure.json').read_bytes())
                self.assertEqual(failure['error']['category'], 'rate-limit' if code == 429 else 'access-denial')

    def test_default_dry_run_requires_no_network(self):
        output = subprocess.check_output([sys.executable, '-B', str(SCRIPT)])
        result = json.loads(output)
        self.assertTrue(result['dryRun'])
        self.assertEqual(result['networkRequests'], 0)

    def test_no_date_batch_retains_limitations_raw_flags_and_partial_page_receipt(self):
        for count, transfer in ((200, True), (3, False), (0, False)):
            source = batch_page(count)
            source['exceededTransferLimit'] = transfer
            evidence, capture, urls = self.run_capture([schema(), source],
                date_omission_evidence_sha256='C' * 64)
            self.assertEqual(urls, [module.SCHEMA_URL, module.BATCH_QUERY])
            self.assertEqual(evidence['networkRequests'], 2)
            self.assertEqual(evidence['inputRows'], count)
            self.assertEqual(capture['fieldLimitations'], {'DateOfWellCompletion': 'source-query-api-400'})
            self.assertEqual(capture['fieldLimitationsEvidenceSha256'], 'c' * 64)
            self.assertEqual(evidence['fieldLimitations'], capture['fieldLimitations'])
            self.assertEqual(evidence['page'], dict(offset=0, requestedRows=200, returnedRows=count,
                orderByFields='ObjectId ASC', orderingVerification='requested-source-order',
                exceededTransferLimit=transfer, complete=False, automaticContinuation=False))
            self.assertEqual(capture['response']['features'], source['features'])
            self.assertTrue(all('DateOfWellCompletion' not in row['attributes']
                                for row in capture['response']['features']))

    def test_no_date_batch_query_and_cli_dry_run_are_fixed_and_zero_request(self):
        params = urllib.parse.parse_qs(urllib.parse.urlsplit(module.query_url('batch-without-date')).query)
        self.assertEqual(params['outFields'], [','.join(module.BATCH_FIELDS)])
        self.assertEqual(params['orderByFields'], ['ObjectId ASC'])
        self.assertEqual(params['resultRecordCount'], ['200'])
        self.assertEqual(params['resultOffset'], ['0'])
        self.assertEqual(params['where'], ['1=1'])
        for limit in (1, 199, 201, True):
            with self.assertRaises(module.GuardFailure):
                module.query_url('batch-without-date', limit)
        with self.assertRaises(module.GuardFailure):
            module.query_url('batch-without-date', fields=module.FIELDS)
        result = json.loads(subprocess.check_output([sys.executable, '-B', str(SCRIPT),
            '--date-omission-evidence-sha256', 'c' * 64]))
        self.assertEqual(result['queryUrl'], module.BATCH_QUERY)
        self.assertEqual(result['networkRequests'], 0)
        self.assertEqual(result['maxTotalResponseBytes'], 1048576)
        self.assertEqual(result['maxResponseBytes'], 524288)
        self.assertEqual(result['maxSeconds'], 60)
        self.assertFalse(result['automaticContinuation'])

    def test_no_date_batch_rejects_extra_rows_dates_and_invalid_ordering_schema(self):
        with self.assertRaises(module.GuardFailure):
            module.sanitize(page(), schema(), ('WellOwner',))
        for response in (batch_page(201), page(1)):
            with self.assertRaises(module.GuardFailure):
                self.run_capture([schema(), response], date_omission_evidence_sha256='c' * 64)
        for key, value in (('objectIdField', 'unreviewed'),
                           ('advancedQueryCapabilities', dict(supportsOrderBy=False, supportsPagination=True)),
                           ('advancedQueryCapabilities', dict(supportsOrderBy='true', supportsPagination=True)),
                           ('advancedQueryCapabilities', dict(supportsOrderBy=True, supportsPagination=False))):
            changed = schema()
            changed[key] = value
            opener = Opener([changed])
            with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
                with self.assertRaises(module.GuardFailure):
                    module.capture(Path(directory) / 'sample', opener=opener, pause=lambda _: None,
                                   date_omission_evidence_sha256='c' * 64)
            self.assertEqual(opener.urls, [module.SCHEMA_URL])
        for bad in ('', 'z' * 64, 'c' * 63, 'PRIVATE-SENTINEL'):
            with mock.patch.object(module, 'request_worker') as worker:
                with self.assertRaises(module.GuardFailure) as error:
                    module.capture('unused', date_omission_evidence_sha256=bad)
                self.assertEqual(error.exception.info['guardCode'], 'limitation-evidence')
                worker.assert_not_called()

    def test_no_date_batch_network_path_keeps_deadline_worker_and_exact_query_phase(self):
        with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
            target = Path(directory) / 'sample'
            with mock.patch.object(module, 'request_worker', side_effect=[
                    module.encoded(schema()), module.encoded(batch_page())]) as worker:
                evidence = module.capture(target, pause=lambda _: None,
                                          date_omission_evidence_sha256='c' * 64)
            self.assertEqual(worker.call_count, 2)
            self.assertEqual([call.args[0][-1] for call in worker.call_args_list],
                             ['schema', 'batch-without-date'])
            self.assertTrue(all(0 < call.args[1] <= 60 for call in worker.call_args_list))
            self.assertEqual(evidence['page']['returnedRows'], 200)

    def test_no_date_batch_transport_and_rust_contract_agree_end_to_end(self):
        binary = SCRIPT.parents[1] / 'crates/us-groundwater/target/debug' / (
            'gev-us-groundwater.exe' if sys.platform == 'win32' else 'gev-us-groundwater')
        if not binary.is_file():
            self.skipTest('Build the existing Rust binary before the integration test')
        # Fake source rows only: no live response or records are used.
        _, capture, _ = self.run_capture([schema(), batch_page()],
            date_omission_evidence_sha256='c' * 64)
        raw = module.encoded(capture)
        first = subprocess.check_output([str(binary), '--texas-sdr-v1'], input=raw)
        self.assertEqual(first, subprocess.check_output([str(binary), '--texas-sdr-v1'], input=raw))
        result = json.loads(first)
        self.assertEqual(result['payload']['accounting']['acceptedRecords'], 200)
        self.assertEqual(result['payload']['accounting']['quarantinedRows'], 0)
        self.assertFalse(result['payload']['coverage']['complete'])
        self.assertEqual(result['manifest']['input']['sha256'], module.digest(raw))
        for record in result['payload']['records']:
            self.assertIsNone(record['completedAt'])
            self.assertEqual(record['provenance']['unrequestedFields'], ['DateOfWellCompletion'])
            self.assertEqual(record['provenance']['fieldLimitationsEvidenceSha256'], 'c' * 64)

    def test_keyset_page_retains_verified_ordered_oid_metadata_and_bounds(self):
        source = batch_page(2)
        for row, oid in zip(source['features'], (11, 15)):
            row['attributes']['ObjectId'] = oid
        evidence, capture, urls = self.run_capture([schema(), source],
            date_omission_evidence_sha256='c'*64, after_object_id=10, record_limit=2, max_seconds=10)
        self.assertEqual(capture['pagination'], dict(afterObjectId=10, objectIds=[11,15]))
        self.assertEqual(evidence['limits']['seconds'], 10)
        self.assertEqual(evidence['page']['lastObjectId'], 15)
        self.assertEqual(evidence['page']['orderingVerification'], 'strictly-increasing-returned-objectids')
        self.assertFalse(evidence['page']['complete'])
        self.assertEqual(evidence['discardedAutomaticObjectIds'], 2)
        params=urllib.parse.parse_qs(urllib.parse.urlsplit(urls[-1]).query)
        self.assertEqual(params['where'], ['ObjectId>10'])
        self.assertEqual(params['resultRecordCount'], ['2'])
        self.assertIn('ObjectId', params['outFields'][0].split(','))
        self.assertNotIn('resultOffset', params)
        self.assertTrue(all('ObjectId' not in r['attributes'] for r in capture['response']['features']))
        binary=SCRIPT.parents[1]/'crates/us-groundwater/target/debug'/('gev-us-groundwater.exe' if sys.platform=='win32' else 'gev-us-groundwater')
        if binary.is_file():
            result=json.loads(subprocess.check_output([str(binary),'--texas-sdr-v1'],input=module.encoded(capture)))
            self.assertEqual(result['payload']['pagination'],capture['pagination'])

    def test_keyset_refuses_missing_reversed_repeated_or_noninteger_oids_without_retry(self):
        for ids in ((None,12),(12,11),(11,11),(10,12),(True,12),(11,2147483648)):
            source=batch_page(2)
            for row,oid in zip(source['features'],ids):
                row['attributes']['ObjectId']=oid
            opener=Opener([schema(),source])
            with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
                with self.assertRaises(module.GuardFailure):
                    module.capture(Path(directory)/'page',opener=opener,pause=lambda _:None,
                        date_omission_evidence_sha256='c'*64,after_object_id=10,record_limit=2)
            self.assertEqual(len(opener.urls),2)
        with self.assertRaises(module.GuardFailure):
            self.run_capture([schema(),batch_page(0)],date_omission_evidence_sha256='c'*64,after_object_id=0)
        for after in (-1,2147483648,True,'0'):
            with self.assertRaises(module.GuardFailure):
                module.query_url('job-page',2,after_object_id=after)

    def test_keyset_worker_uses_persisted_cursor_limit_and_remaining_deadline(self):
        source=batch_page(1)
        source['features'][0]['attributes']['ObjectId']=8
        with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
            with mock.patch.object(module,'request_worker',side_effect=[module.encoded(schema()),module.encoded(source)]) as worker:
                module.capture(Path(directory)/'page',pause=lambda _:None,date_omission_evidence_sha256='c'*64,
                    after_object_id=7,record_limit=1,max_seconds=9)
            command=worker.call_args_list[-1].args[0]
            self.assertEqual(command[-6:],['--_http-phase','job-page','--after-object-id','7','--_record-limit','1'])
            self.assertTrue(all(0<call.args[1]<=9 for call in worker.call_args_list))

    def test_total_deadline_cancels_connect_and_slow_trickle_worker(self):
        # No HTTP: the fake child either stalls before headers (connect-like)
        # or emits bytes frequently enough to evade an inactivity timeout.
        for source in ('import time; time.sleep(10)',
            'import sys,time\nfor _ in range(100):\n sys.stdout.buffer.write(b"x"); sys.stdout.flush(); time.sleep(0.02)'):
            started = time.monotonic()
            with self.assertRaises(module.SourceFailure) as error:
                module.request_worker([sys.executable, '-B', '-c', source], 0.2)
            self.assertEqual(error.exception.info['category'], 'total-deadline')
            self.assertLess(time.monotonic() - started, 2)

    def test_default_network_path_uses_deadline_worker_for_both_fixed_requests(self):
        with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
            target = Path(directory) / 'sample'
            with mock.patch.object(module, 'request_worker', side_effect=[
                    module.encoded(schema()), module.encoded(page())]) as worker:
                evidence = module.capture(target, pause=lambda _: None)
            self.assertEqual(worker.call_count, 2)
            self.assertEqual([call.args[0][-1] for call in worker.call_args_list], ['schema', 'query'])
            self.assertTrue(all(0 < call.args[1] <= module.TIME_LIMIT for call in worker.call_args_list))
            self.assertEqual(evidence['networkRequests'], 2)

    def test_response_crs_is_checked_and_missing_crs_has_explicit_basis(self):
        for crs in (dict(wkid=3857), dict(wkid=4326, latestWkid=3857),
                    dict(wkt='unreviewed'), 'EPSG:4326'):
            response = page()
            response['spatialReference'] = crs
            with self.assertRaisesRegex(ValueError, 'CRS'):
                self.run_capture([schema(), response])
        _, absent, _ = self.run_capture([schema(), page()])
        self.assertEqual(absent['response']['spatialReferenceBasis'],
                         'validated-layer-schema; attributes-unverified')
        present = page()
        present['spatialReference'] = dict(wkid=4326, latestWkid=4326)
        _, supplied, _ = self.run_capture([schema(), present])
        self.assertEqual(supplied['response']['spatialReferenceBasis'],
                         'source-response; attributes-unverified')
        with self.assertRaises(ValueError):
            module.sanitize(page())  # absent CRS cannot be invented without schema


if __name__ == '__main__':
    unittest.main()
