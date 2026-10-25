"""Offline transport regressions. No network calls, packages or child processes."""
import importlib.util
import json
from contextlib import contextmanager
from uuid import uuid4
import unittest
from unittest.mock import patch
from pathlib import Path
from urllib.error import URLError

spec = importlib.util.spec_from_file_location('capture', Path(__file__).with_name('capture-usgs-inventory.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


@contextmanager
def scratch():
    directory = Path(__file__).resolve().parents[1] / '.temp-gev' / ('usgs-transport-' + uuid4().hex)
    directory.mkdir(parents=True)
    yield directory
    # Preserve task-owned test evidence; do not delete or change permissions.


class Response:
    def __init__(self, url, body):
        self.url, self.raw = url, json.dumps(body).encode()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False

    def geturl(self):
        return self.url

    def read(self, size):
        return self.raw[:size]


class Opener:
    def __init__(self, bodies):
        self.bodies, self.calls = list(bodies), 0

    def open(self, request, timeout):
        assert timeout == 20
        self.calls += 1
        body = self.bodies.pop(0)
        if isinstance(body, Exception):
            raise body
        return Response(request.full_url, body)


def page(rows=1, next_url=None):
    return {'type': 'FeatureCollection', 'features': [{'id': i} for i in range(rows)],
            'links': [] if next_url is None else [{'rel': 'next', 'href': next_url}]}


class CaptureTests(unittest.TestCase):
    def test_projected_cached_body_and_header_loading_without_refetch(self):
        from urllib.parse import urlencode
        filters = dict(module.FILTERS, limit='10000', properties=module.PROJECTED_FIELDS)
        url = module.BASE + 'items?' + urlencode(filters)
        class Projected:
            calls = 0
            def open(inner, request, timeout):
                self.assertEqual(timeout, 120)
                self.assertEqual(request.get_header('X-api-key'), 'dummyProjectedKey')
                inner.calls += 1
                return Response(request.full_url, page(1001, url + '&cursor=next'))
        with patch.dict('os.environ', {'USGS_API_KEY': 'dummyProjectedKey'}), scratch() as directory:
            opener = Projected()
            kwargs = dict(limit=10000, properties=module.PROJECTED_FIELDS, max_bytes=67108864, page_bytes=33554432)
            result = module.capture(directory, opener=opener, **kwargs)
            metrics = {}
            self.assertEqual(result, module.capture(directory, opener=Opener([]), metrics=metrics, **kwargs))
            self.assertEqual(metrics['cacheHits'], 1)
            self.assertEqual(metrics['networkRequests'], 0)
            self.assertEqual(opener.calls, 1)
            for file in directory.iterdir(): self.assertNotIn(b'dummyProjectedKey', file.read_bytes())
            with self.assertRaises(ValueError):
                module.page_url(url.replace('limit=10000', 'limit=50000') + '&cursor=next', True, filters)
        with patch.dict('os.environ', {'USGS_API_KEY': 'dummyProjectedKey'}), scratch() as directory:
            with self.assertRaisesRegex(ValueError, 'byte cap'):
                module.capture(directory, opener=Projected(), limit=10000, properties=module.PROJECTED_FIELDS,
                               max_bytes=67108864, page_bytes=1024)

    def test_key_is_header_only_and_never_in_capture_cache_or_metrics(self):
        dummy = 'dummyUSGSkey123456'
        class HeaderCheck(Opener):
            def open(self, request, timeout):
                self_outer.assertEqual(request.get_header('X-api-key'), dummy)
                self_outer.assertNotIn(dummy, request.full_url)
                return super().open(request, timeout)
        self_outer = self
        with patch.dict('os.environ', {'USGS_API_KEY': dummy}), scratch() as directory:
            metrics = {}
            result = module.capture(directory, opener=HeaderCheck([page()]), metrics=metrics)
            self.assertNotIn(dummy, json.dumps(result))
            self.assertNotIn(dummy, json.dumps(metrics))
            for path in directory.iterdir():
                self.assertNotIn(dummy.encode(), path.read_bytes())

    def test_transport_errors_redact_key_and_echoed_key_is_not_cached(self):
        dummy = 'dummyUSGSkey123456'
        with patch.dict('os.environ', {'USGS_API_KEY': dummy}), scratch() as directory:
            with self.assertRaisesRegex(RuntimeError, 'REDACTED') as caught:
                module.capture(directory, opener=Opener([URLError(dummy)]))
            self.assertNotIn(dummy, str(caught.exception))
            self.assertEqual(list(directory.iterdir()), [])
        with patch.dict('os.environ', {'USGS_API_KEY': dummy}), scratch() as directory:
            body = page()
            body['echo'] = dummy
            with self.assertRaisesRegex(ValueError, 'credential; capture refused'):
                module.capture(directory, opener=Opener([body]))
            self.assertEqual(list(directory.iterdir()), [])

    def test_invalid_key_is_suppressed_before_any_request(self):
        with patch.dict('os.environ', {'USGS_API_KEY': 'invalid\r\nkey'}), scratch() as directory:
            opener = Opener([])
            with self.assertRaisesRegex(ValueError, 'value suppressed'):
                module.capture(directory, opener=opener)
            self.assertEqual(opener.calls, 0)

    def test_thousand_rows_partial_final_page_cache_and_quota_headers(self):
        class WithHeaders(Opener):
            def open(self, request, timeout):
                response = super().open(request, timeout)
                response.headers = {'X-RateLimit-Limit': '50', 'X-RateLimit-Remaining': '49'}
                return response
        first = module.FIRST.replace('limit=25', 'limit=1000')
        with scratch() as directory:
            opener = WithHeaders([page(1000, first + '&cursor=two'), page(7)])
            metrics = {}
            result = module.capture(directory, pages=2, limit=1000, opener=opener, pause=lambda _: None, metrics=metrics)
            self.assertEqual(sum(len(p['response']['features']) for p in result['acquisitions']), 1007)
            self.assertEqual(metrics['networkRequests'], 2)
            self.assertEqual(metrics['quotaHeaders'][0]['X-RateLimit-Limit'], '50')
            replay = {}
            self.assertEqual(result, module.capture(directory, pages=2, limit=1000, opener=Opener([]), metrics=replay))
            self.assertEqual(replay['networkRequests'], 0)
            self.assertEqual(replay['cacheHits'], 2)

    def test_large_row_and_response_byte_bounds_remain_finite(self):
        with scratch() as directory:
            with self.assertRaises(ValueError):
                module.capture(directory, limit=1001, opener=Opener([]))
        with scratch() as directory:
            with self.assertRaisesRegex(ValueError, 'page row cap'):
                module.capture(directory, limit=1000, opener=Opener([page(1001)]))
        with scratch() as directory:
            opener = Opener([page(1000)])
            with self.assertRaisesRegex(ValueError, 'byte cap'):
                module.capture(directory, limit=1000, max_bytes=1024, opener=opener)
            self.assertEqual(opener.calls, 1)

    def test_start_cursor_replays_exact_url_and_reports_cache(self):
        url = module.FIRST + '&cursor=opaque%2Btoken%3D'
        with scratch() as directory:
            metrics = {}
            first = module.capture(directory, start_url=url, opener=Opener([page()]), metrics=metrics)
            self.assertEqual(first['acquisitions'][0]['url'], url)
            self.assertEqual(metrics['networkRequests'], 1)
            repeat = {}
            self.assertEqual(first, module.capture(directory, start_url=url, opener=Opener([]), metrics=repeat))
            self.assertEqual(repeat['networkRequests'], 0)
            self.assertEqual(repeat['cacheHits'], 1)

    def test_start_cursor_scope_and_immediate_loop_refused(self):
        for url in ['https://example.org/items?cursor=a', module.FIRST.replace('state_code=22', 'state_code=36') + '&cursor=a']:
            with scratch() as directory:
                opener = Opener([])
                with self.assertRaises(ValueError):
                    module.capture(directory, start_url=url, opener=opener)
                self.assertEqual(opener.calls, 0)
        with scratch() as directory:
            url = module.FIRST + '&cursor=A'
            with self.assertRaisesRegex(ValueError, 'loop|cursor'):
                module.capture(directory, start_url=url, opener=Opener([page(1, module.FIRST + '&cursor=%41')]))

    def test_explicit_state_and_smaller_row_budget_keep_filter_scope(self):
        with scratch() as directory:
            result = module.capture(directory, state='36', limit=5, opener=Opener([page(5)]))
            self.assertIn('state_code=36', result['acquisitions'][0]['url'])
            self.assertIn('limit=5', result['acquisitions'][0]['url'])
        with scratch() as directory:
            with self.assertRaises(ValueError):
                module.capture(directory, state='36', limit=5,
                               opener=Opener([page(5, module.FIRST + '&cursor=two')]))
        with self.assertRaises(ValueError):
            module.capture('unused', state='LA', opener=Opener([]))

    def test_opaque_cursor_url_is_not_rewritten(self):
        url = module.FIRST + '&cursor=opaque%2Bvalue%2Fpart%3D'
        self.assertEqual(module.page_url(url, next_page=True), url)
        with scratch() as directory:
            opener = Opener([page(1, url), page()])
            result = module.capture(directory, pages=2, opener=opener, pause=lambda _: None)
            self.assertEqual(result['acquisitions'][1]['url'], url)

    def test_malformed_missing_and_duplicate_cursor_parameters(self):
        for suffix in ['', '&cursor=', '&cursor=one&cursor=two', '&cursor=%00',
                       '&cursor=%0A', '&cursor=%ZZ', '&cursor=+', '&cursor=' + 'x' * 2049]:
            with self.subTest(suffix=suffix[:50]), self.assertRaises(ValueError):
                module.page_url(module.FIRST + suffix, next_page=True)

    def test_next_link_keeps_exact_origin_path_and_filters(self):
        good = module.FIRST + '&cursor=one'
        for bad in [good.replace('https:', 'http:'), good.replace('api.waterdata.usgs.gov', 'example.org'),
                    good.replace('api.waterdata.usgs.gov', 'api.waterdata.usgs.gov:443'),
                    good.replace('/items?', '/schema?'), good.replace('state_code=22', 'state_code=36'),
                    good.replace('site_type_code=GW', 'site_type_code=ST'),
                    good.replace('limit=25', 'limit=500'), good + '&agency_code=USGS',
                    good + '#fragment', good + '&offset=25', good.replace('https://', 'https://\t')]:
            with self.subTest(url=bad), self.assertRaises(ValueError):
                module.page_url(bad, next_page=True)

    def test_repeated_cursor_refuses_second_page_even_when_query_order_changes(self):
        first_next = module.FIRST + '&cursor=one'
        repeated = module.BASE + 'items?cursor=%6Fne&' + module.FIRST.split('?', 1)[1]
        with scratch() as directory:
            opener = Opener([page(1, first_next), page(1, repeated)])
            with self.assertRaisesRegex(ValueError, 'loop|repeated'):
                module.capture(directory, pages=2, opener=opener, pause=lambda _: None)
            self.assertEqual(opener.calls, 2)

    def test_bad_links_do_not_request_another_page(self):
        for next_url in [module.FIRST, None, 'https://example.org/items?cursor=one']:
            with scratch() as directory:
                opener = Opener([page(1, module.FIRST) | {'links': [{'rel': 'next', 'href': next_url}]}])
                with self.assertRaises(ValueError):
                    module.capture(directory, pages=2, opener=opener)
                self.assertEqual(opener.calls, 1)

    def test_total_byte_budget_includes_capture_encoding(self):
        with scratch() as directory:
            opener = Opener([page(1, module.FIRST + '&cursor=two') | {'padding': 'a' * 400},
                             page(1) | {'padding': 'b' * 400}])
            with self.assertRaisesRegex(ValueError, 'total .*byte cap'):
                module.capture(directory, pages=2, opener=opener, pause=lambda _: None, max_bytes=1600)
            self.assertEqual(opener.calls, 2)

    def test_page_budget_leaves_third_page_unrequested(self):
        with scratch() as directory:
            opener = Opener([page(25, module.FIRST + '&cursor=two'),
                             page(25, module.FIRST + '&cursor=three')])
            result = module.capture(directory, pages=2, opener=opener, pause=lambda _: None)
            self.assertEqual(opener.calls, 2)
            self.assertEqual(len(result['acquisitions']), 2)

    def test_total_raw_response_budget_is_enforced_before_output(self):
        class WhitespaceOpener(Opener):
            def open(self, request, timeout):
                response = super().open(request, timeout)
                response.raw += b' ' * 400000
                return response
        with scratch() as directory:
            opener = WhitespaceOpener([page(1, module.FIRST + '&cursor=two'), page(1)])
            with self.assertRaisesRegex(ValueError, 'total response byte cap'):
                module.capture(directory, pages=2, opener=opener, pause=lambda _: None, max_bytes=700000)
            self.assertEqual(opener.calls, 2)

    def test_bad_cursor_keeps_existing_valid_candidate_bytes(self):
        previous = Path(__file__).resolve().parents[1] / 'fixtures/us-groundwater/usgs-inventory-pilot/candidate-bundle.json'
        before = previous.read_bytes()
        with scratch() as directory:
            opener = Opener([page(1, module.FIRST + '&cursor=one'),
                             page(1, module.FIRST + '&cursor=one')])
            with self.assertRaises(ValueError):
                module.capture(directory, pages=2, opener=opener, pause=lambda _: None)
        self.assertEqual(previous.read_bytes(), before)

    def test_cached_repeat_is_identical_and_zero_network(self):
        with scratch() as directory:
            opener = Opener([page()])
            first = module.capture(directory, opener=opener)
            second = module.capture(directory, opener=Opener([]))
            self.assertEqual(first, second)
            self.assertEqual(opener.calls, 1)

    def test_two_small_pages_and_one_second_rate_limit(self):
        with scratch() as directory:
            opener = Opener([page(25, module.FIRST + '&cursor=page-2'), page(25)])
            pauses = []
            result = module.capture(directory, pages=2, opener=opener, pause=pauses.append)
            self.assertEqual(sum(len(p['response']['features']) for p in result['acquisitions']), 50)
            self.assertEqual(pauses, [1])

    def test_scope_redirect_and_page_budget_refusals(self):
        for url in ['https://example.org/items', module.FIRST.replace('state_code=22', 'state_code=36'),
                    module.FIRST + '&offset=50', module.FIRST + '&token=bad']:
            with self.assertRaises(ValueError):
                module.page_url(url)
        with self.assertRaises(ValueError):
            module.NoRedirect().redirect_request(None, None, 302, None, None, 'https://example.org')
        with scratch() as directory:
            with self.assertRaises(ValueError):
                module.capture(directory, pages=3, opener=Opener([]))

    def test_malformed_oversized_pages_are_not_cached(self):
        for body in [page(26), {'type': 'FeatureCollection'}, page() | {'padding': 'x' * module.PAGE_BYTES}]:
            with scratch() as directory:
                with self.assertRaises((ValueError, json.JSONDecodeError)):
                    module.capture(directory, opener=Opener([body]))
                self.assertEqual(list(Path(directory).iterdir()), [])

    def test_corrupt_cache_is_refused_without_refetch(self):
        with scratch() as directory:
            module.capture(directory, opener=Opener([page()]))
            path = next(Path(directory).iterdir())
            entry = json.loads(path.read_text())
            entry['body'] += ' '
            path.write_text(json.dumps(entry))
            opener = Opener([])
            with self.assertRaisesRegex(ValueError, 'integrity'):
                module.capture(directory, opener=opener)
            self.assertEqual(opener.calls, 0)

    def test_failure_has_no_retry_and_preserves_existing_cache(self):
        with scratch() as directory:
            first = module.capture(directory, opener=Opener([page(1, module.FIRST + '&cursor=page-2')]))
            path = next(Path(directory).iterdir())
            before = path.read_bytes()
            opener = Opener([URLError('DNS unavailable')])
            with self.assertRaises(URLError):
                module.capture(directory, pages=2, opener=opener, pause=lambda _: None)
            self.assertEqual(opener.calls, 1)
            self.assertEqual(path.read_bytes(), before)
            self.assertEqual(module.capture(directory, opener=Opener([])), first)


if __name__ == '__main__':
    unittest.main()
