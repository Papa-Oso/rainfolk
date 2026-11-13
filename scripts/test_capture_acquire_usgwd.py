"""Offline checks for scripts/acquire-usgwd.py: no network access."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('acquire_usgwd', Path(__file__).with_name('acquire-usgwd.py'))
acq = importlib.util.module_from_spec(spec)
spec.loader.exec_module(acq)
BASE = f'http://www.hydroshare.org/resource/{acq.RESOURCE}/data/contents/'


def page(*files):
    return dict(results=[dict(url=BASE + p, size=s, checksum=c, modified_time='2024-03-08T14:22:08') for p, s, c in files])


PINNED = [('USGWD-Tabular/USGWD_Nebraska.csv', 59806912, '985bb61e2f293968fe5a997ded6b7c33'),
          ('USGWD-Geospatial/USGWD_Nebraska.zip', 25624956, 'd2ae605b4bc02616519c80f98888aa41'),
          ('Metadata/Table_S3-Definitions_of_attributes_in_our_dataset.xlsx', 11210, 'f39091b7e6020df7ea61dee2afb5270c')]


class AcquireTests(unittest.TestCase):
    def test_fingerprint_is_order_independent_and_compact(self):
        a = acq.entries([page(*PINNED)])
        b = acq.entries([page(*reversed(PINNED))])
        self.assertEqual(acq.fingerprint(a), acq.fingerprint(b))
        self.assertRegex(acq.fingerprint(a), '^[0-9a-f]{64}$')

    def test_lookalike_uploads_are_strays_not_data(self):
        pinned = acq.entries([page(*PINNED)])
        live = acq.entries([page(*PINNED, ('USGWD-Tabular/USGWD-Neb.csv', 22, 'x-1'),
                                  ('USGWD-Tabular/USGWD_nebraska.csv', 22, 'x-1'),
                                  ('USGWD-Tabular/USGWD_Nebraska.csv/zip', 22, 'x-1'))])
        r = acq.classify(pinned, live)
        self.assertEqual((r['unchangedData'], r['changedData'], r['newData']), (3, [], []))
        self.assertEqual(len(r['strayEntries']), 3)
        self.assertNotEqual(acq.fingerprint(pinned), acq.fingerprint(live))

    def test_changed_or_new_state_file_needs_review(self):
        pinned = acq.entries([page(*PINNED)])
        live = acq.entries([page(('USGWD-Tabular/USGWD_Nebraska.csv', 59806913, 'ffff'), *PINNED[1:],
                                  ('USGWD-Tabular/USGWD_Puerto_Rico.csv', 5000, 'abcd'))])
        r = acq.classify(pinned, live)
        self.assertEqual(r['changedData'], ['USGWD-Tabular/USGWD_Nebraska.csv'])
        self.assertEqual(r['newData'], ['USGWD-Tabular/USGWD_Puerto_Rico.csv'])

    def test_duplicate_listing_paths_are_refused(self):
        with self.assertRaises(ValueError):
            acq.entries([page(PINNED[0]), page(PINNED[0])])

    def test_only_hydroshare_and_its_buckets_are_allowed(self):
        ok = [f'https://www.hydroshare.org/resource/{acq.RESOURCE}/data/contents/x.csv',
              f'https://storage.googleapis.com/hydroshare-1-prod-cloud-native-published/{acq.RESOURCE}/data/contents/x.csv?sig=1',
              'https://storage.googleapis.com/hydroshare-1-prod-cloud-native-resource/tmp/7ac26be0-54a1-438a-98a3-d9b1be750690.zip']
        bad = ['http://www.hydroshare.org/x', 'https://example.com/x.csv',
               'https://storage.googleapis.com/other-bucket/x.csv',
               'https://storage.googleapis.com/hydroshare-1-prod-cloud-native-published/0000/data/contents/x.csv',
               'https://storage.googleapis.com/hydroshare-1-prod-cloud-native-resource/tmp/../secret.zip']
        for url in ok:
            self.assertEqual(acq.checked_url(url), url)
        for url in bad:
            with self.assertRaises(ValueError, msg=url):
                acq.checked_url(url)


if __name__ == '__main__':
    unittest.main()
