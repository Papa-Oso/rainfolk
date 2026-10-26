"""Offline transport checks using fake public responses, never a live endpoint."""
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import urllib.error

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
            self.assertFalse(target.exists())

    def test_rate_and_access_denial_have_no_retry_or_saved_body(self):
        for code in (401, 403, 429):
            opener = Opener([schema(), urllib.error.HTTPError(module.QUERY, code, '', {}, None)])
            with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
                target = Path(directory) / 'sample'
                with self.assertRaisesRegex(ValueError, f'HTTP {code}'):
                    module.capture(target, opener=opener, pause=lambda _: None)
                self.assertEqual(len(opener.urls), 2)
                self.assertFalse(target.exists())

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

    def test_default_dry_run_requires_no_network(self):
        output = subprocess.check_output([sys.executable, '-B', str(SCRIPT)])
        result = json.loads(output)
        self.assertTrue(result['dryRun'])
        self.assertEqual(result['networkRequests'], 0)


if __name__ == '__main__':
    unittest.main()
