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
from unittest import mock

SCRIPT = Path(__file__).with_name('capture-texas-sdr.py')
SCRATCH = SCRIPT.parents[1] / '.temp-gev' / 'sdr-transport-tests'
SCRATCH.mkdir(parents=True, exist_ok=True)
spec = importlib.util.spec_from_file_location('capture_texas_sdr', SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def schema():
    return dict(fields=[dict(name=k, type=v, domain=None) for k, v in module.TYPES.items()],
        sourceSpatialReference=dict(wkid=4326), dateFieldsTimeReference=dict(timeZone='UTC'),
        advancedQueryCapabilities=dict(supportsOrderBy=True), maxRecordCount=2000)


def page(rows=2):
    return dict(exceededTransferLimit=True, features=[dict(attributes=dict(
        WellReportTrackingNumber=str(i).zfill(15), LatitudeDD=30.25, LongitudeDD=-97.75,
        ProposedUse=None, DateOfWellCompletion=None, BoreholeDepthFt=None, LocationError=i))
        for i in range(rows)])


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

    def test_duplicate_json_members_and_nonfinite_numbers_refused(self):
        for bad in (b'{"features":[],"features":[]}', b'{"a":NaN}'):
            with self.assertRaisesRegex(ValueError, 'invalid source JSON'):
                module.decode(bad)

    def test_api_error_retains_only_numeric_code(self):
        for check in (module.sanitize, module.schema_check):
            with self.assertRaisesRegex(ValueError, '403') as error:
                check(dict(error=dict(code=403, message='PRIVATE-SENTINEL', details=['PRIVATE-SENTINEL'])))
            self.assertNotIn('PRIVATE-SENTINEL', str(error.exception))

    def test_bounded_safe_api_diagnostics_are_persisted_without_record_values(self):
        error = dict(error=dict(code=400, message='Unable to complete operation.', details=[
            'Invalid field: LatitudeDD', 'PRIVATE-SENTINEL', 'owner@example.com', 'omitted-extra-detail']),
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
            self.assertNotIn('owner@example.com', raw)
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
