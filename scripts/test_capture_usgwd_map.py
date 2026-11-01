"""Invented records through normalization, existing index writers and browser admission."""
import hashlib
import json
import os
import random
from pathlib import Path
import subprocess
import unittest
import test_capture_usgwd as fixtures
ROOT = fixtures.ROOT

INDEX = ROOT/'crates/us-groundwater/target/debug'/('well_index.exe' if os.name == 'nt' else 'well_index')


class UsgwdMapTests(unittest.TestCase):
    setUp = fixtures.UsgwdTests.setUp
    tearDown = fixtures.UsgwdTests.tearDown
    source = fixtures.UsgwdTests.source
    run_import = fixtures.UsgwdTests.run_import
    segment = fixtures.UsgwdTests.segment

    def index_run(self, manifest, out, okay=True):
        result = subprocess.run([str(INDEX), '--usgwd', str(manifest), str(out)], capture_output=True)
        if okay:
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            return json.loads(result.stdout)
        self.assertNotEqual(result.returncode, 0)
        return result

    def packs(self, out):
        index = json.loads((out/'index.json').read_bytes())
        result = []
        for ref in index['regions']:
            raw = (out/ref['url']).read_bytes()
            self.assertEqual(hashlib.sha256(raw).hexdigest(), ref['sha256'])
            result.append(json.loads(raw))
        return index, result

    def test_dbf_export_preserves_published_archive_and_derived_identity(self):
        paths=self.source();s=json.loads(paths[1].read_bytes());s.update(schemaVersion=2,sourceUrl=f'https://www.hydroshare.org/resource/{fixtures.RESOURCE}/data/contents/USGWD-Geospatial/USGWD_New_Hampshire.zip',derivation=dict(policy='usgwd-dbf-attributes-v1',archiveSha256='c'*64,archiveMd5='d'*32,member='USGWD_New Hampshire.dbf',memberSha256='e'*64,sourceCrs='EPSG:4269',recordCount=len(self.rows)))
        paths[1].write_text(json.dumps(s));self.run_import(paths,allowance=None);out=self.root/'dbf-map';self.index_run(self.root/'output/usgwd-manifest.json',out)
        _,packs=self.packs(out)
        for pack in packs:
            a=pack['acquisitions'][0];self.assertEqual(a['derivation'],s['derivation']);self.assertEqual(a['inputSha256'],s['inputSha256']);self.assertEqual(a['url'],s['sourceUrl'])

    def test_map_refuses_resigned_derived_cardinality_mismatch(self):
        paths=self.source();s=json.loads(paths[1].read_bytes());s.update(schemaVersion=2,sourceUrl=f'https://www.hydroshare.org/resource/{fixtures.RESOURCE}/data/contents/USGWD-Geospatial/USGWD_New_Hampshire.zip',derivation=dict(policy='usgwd-dbf-attributes-v1',archiveSha256='c'*64,archiveMd5='d'*32,member='USGWD_New Hampshire.dbf',memberSha256='e'*64,sourceCrs='EPSG:4269',recordCount=len(self.rows)));paths[1].write_text(json.dumps(s));self.run_import(paths,allowance=None)
        manifest=self.root/'output/usgwd-manifest.json';e=json.loads(manifest.read_bytes());e['payload']['source']['derivation']['recordCount']=1;e['sha256']=hashlib.sha256(json.dumps(e['payload'],sort_keys=True,separators=(',',':'),ensure_ascii=False).encode()).hexdigest();manifest.write_text(json.dumps(e));r=self.index_run(manifest,self.root/'bad-count',okay=False);self.assertIn(b'row count mismatch',r.stderr)

    def test_explicit_deadline_changes_no_artifact_bytes_and_invalid_limits_write_nothing(self):
        paths = self.source(); self.run_import(paths, allowance=None)
        manifest = self.root/'output/usgwd-manifest.json'
        default = self.index_run(manifest, self.root/'default-budget')
        explicit = subprocess.run([str(INDEX), '--usgwd', str(manifest), str(self.root/'explicit-budget'), '900'], capture_output=True)
        self.assertEqual(explicit.returncode, 0, explicit.stderr.decode())
        self.assertEqual(json.loads(explicit.stdout)['indexSha256'], default['indexSha256'])
        for value in ['0', '901', '-1', 'invalid', '18446744073709551616']:
            out = self.root/('invalid-'+value)
            failed = subprocess.run([str(INDEX), '--usgwd', str(manifest), str(out), value], capture_output=True)
            self.assertNotEqual(failed.returncode, 0)
            self.assertFalse(out.exists())

    def test_invalid_quantity_keeps_original_and_maps_without_fake_usgs_identity(self):
        paths = self.source([dict(self.rows[0], **{'Length of Screen (Feet)': '-5'})])
        self.run_import(paths, allowance=None)
        out = self.root/'map'
        self.index_run(self.root/'output/usgwd-manifest.json', out)
        index, packs = self.packs(out)
        self.assertEqual(index['indexedRecords'], 1)
        self.assertEqual(index['catalogRecords'], 1)
        self.assertIsNone(index['distinctPhysicalWells'])
        r = packs[0]['records'][0]
        self.assertTrue(r['id'].startswith('USGWD-'))
        self.assertEqual(r['source'], 'USGWD')
        self.assertEqual(r['readings'], [])
        self.assertEqual(r['metadata']['quantityErrors'], [{'field': 'Length of Screen (Feet)', 'reason': 'negative', 'rawValue': '-5'}])
        self.assertIsNone(r['metadata']['quantities']['Length of Screen (Feet)']['value'])
        self.assertEqual(r['metadata']['sourceCoordinates']['crs'], 'EPSG:4269')
        self.assertEqual(r['coordinateReference'], 'EPSG:4326')
        self.assertEqual(r['metadata']['coordinateOperation']['operation'], 'EPSG:1188')

    def test_finite_measurement_precision_survives_json_hash_round_trip(self):
        rng=random.Random(20261028)
        rows=[dict(self.rows[0], **{'Well ID':f'PRECISION_{i}', 'Well Depth (Feet)':repr(32+rng.random()*32)}) for i in range(100)]
        paths=self.source(rows);self.run_import(paths,allowance=None)
        out=self.root/'precision-map';result=self.index_run(self.root/'output/usgwd-manifest.json',out)
        self.assertEqual(result['indexedRecords'],100)
        _,packs=self.packs(out)
        depths={r['metadata']['sourceRecordId']:r['metadata']['quantities']['Well Depth (Feet)']['value'] for p in packs for r in p['records']}
        for row in rows:self.assertEqual(depths[row['Well ID']],float(row['Well Depth (Feet)']))

    def test_buffered_coincident_records_remain_distinct_across_source_partitions(self):
        rows=[dict(self.rows[0], **{'Well ID':f'BUFFERED_{i:05}'}) for i in range(1201)]
        paths=self.source(rows);self.run_import(paths,allowance=None);out=self.root/'buffered-map'
        self.index_run(self.root/'output/usgwd-manifest.json',out);index,packs=self.packs(out)
        ids=[r['id'] for p in packs for r in p['records']]
        self.assertEqual(index['indexedRecords'],1201);self.assertEqual(len(set(ids)),1201)
        self.assertEqual(len(ids),1201)

    def test_dense_compaction_streams_coincident_and_split_cells_without_identity_loss(self):
        rows=[dict(self.rows[0], **{'Well ID':f'DENSE_{i:05}'}) for i in range(4001)]
        for i,row in enumerate(rows):
            if i%3==0: row['Longitude']='-71.40'
        paths=self.source(rows);self.run_import(paths,allowance=None);out=self.root/'dense-map'
        first=self.index_run(self.root/'output/usgwd-manifest.json',out);index,packs=self.packs(out)
        ids=[r['id'] for p in packs for r in p['records']]
        self.assertEqual(index['indexedRecords'],4001);self.assertEqual(len(ids),4001);self.assertEqual(len(set(ids)),4001)
        self.assertTrue(all(len(p['records'])<=256 for p in packs))
        before=(out/'index.json').read_bytes();again=self.index_run(self.root/'output/usgwd-manifest.json',out)
        self.assertEqual(first['indexSha256'],again['indexSha256']);self.assertEqual((out/'index.json').read_bytes(),before)

    def test_global_conflicting_versions_do_not_leak_from_partitions_to_map(self):
        rows = [dict(self.rows[0], **{'Well ID': f'MAP_{i}'}) for i in range(200)]
        rows += [rows[0], dict(rows[1], **{'Length of Screen (Feet)': '-5'})]
        paths = self.source(rows)
        self.segment(paths); final = self.segment(paths)
        self.assertEqual(final['accounting']['acceptedRecords'], 199)
        out = self.root/'map'
        self.index_run(self.root/'segmented/usgwd-segmented-manifest.json', out)
        index, packs = self.packs(out)
        ids = [r['id'] for p in packs for r in p['records']]
        self.assertEqual(index['indexedRecords'], 199)
        self.assertEqual(len(ids), len(set(ids)))
        self.assertFalse(any(i.endswith('-MAP_1') for i in ids))

    def test_unmapped_records_stay_in_catalog_and_old_policy_refuses_activation(self):
        paths = self.source()
        self.run_import(paths, allowance=None)
        out = self.root/'map'; manifest = self.root/'output/usgwd-manifest.json'
        self.index_run(manifest, out)
        index, _ = self.packs(out)
        self.assertEqual((index['catalogRecords'], index['indexedRecords'], index['unmappedRecords']), (2, 1, 1))
        before = (out/'index.json').read_bytes()
        self.index_run(manifest, out)
        self.assertEqual((out/'index.json').read_bytes(), before)
        e = json.loads(manifest.read_bytes()); e['payload']['normalizationPolicy'] = 'previous'
        compact = lambda v: json.dumps(v, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()
        e['sha256'] = hashlib.sha256(compact(e['payload'])).hexdigest(); manifest.write_bytes(compact(e))
        self.index_run(manifest, out, okay=False)
        self.assertEqual((out/'index.json').read_bytes(), before)

    def test_tampered_partition_preserves_last_good_index(self):
        paths = self.source(); self.run_import(paths, allowance=None)
        out = self.root/'map'; manifest = self.root/'output/usgwd-manifest.json'
        self.index_run(manifest, out); before = (out/'index.json').read_bytes()
        ref = json.loads(manifest.read_bytes())['payload']['partitions'][0]
        p = manifest.parent/ref['path']; p.write_bytes(p.read_bytes() + b' ')
        self.index_run(manifest, out, okay=False)
        self.assertEqual((out/'index.json').read_bytes(), before)

    def test_self_consistent_manifest_cannot_duplicate_one_partition_and_omit_another(self):
        paths=self.source([dict(self.rows[0], **{'Well ID':f'PART_{i}'}) for i in range(400)])
        self.run_import(paths, allowance=None)
        out=self.root/'map';manifest=self.root/'output/usgwd-manifest.json'
        self.index_run(manifest,out);before=(out/'index.json').read_bytes()
        e=json.loads(manifest.read_bytes());e['payload']['partitions'][1]=e['payload']['partitions'][0]
        compact=lambda v:json.dumps(v,sort_keys=True,separators=(',',':'),ensure_ascii=False).encode()
        e['sha256']=hashlib.sha256(compact(e['payload'])).hexdigest();manifest.write_bytes(compact(e))
        self.index_run(manifest,out,okay=False)
        self.assertEqual((out/'index.json').read_bytes(),before)

    def make_state_index(self, state, native, target):
        row = dict(self.rows[0], **{'Well ID': native, 'State': state})
        paths = self.source([row]); source = json.loads(paths[1].read_bytes())
        source['state'] = state; source['sourceUrl'] = source['sourceUrl'].replace('New_Hampshire', state.replace(' ', '_'))
        paths[1].write_text(json.dumps(source))
        self.run_import(paths, state+'-'+native, allowance=None)
        out = target/'states'/state.replace(' ', '_')
        self.index_run(self.root/(state+'-'+native)/'usgwd-manifest.json', out)
        return dict(state=state, path=f'states/{state.replace(" ", "_")}/index.json', sha256=hashlib.sha256((out/'index.json').read_bytes()).hexdigest())

    def test_federation_uses_its_existing_child_index_cap_and_rejects_duplicate_members(self):
        out=self.root/'sized-national';ref=self.make_state_index('New Hampshire','SIZED_SOURCE',out)
        child=out/ref['path'];original=child.read_bytes();descriptor=self.root/'sized-states.json'
        def merge(raw):
            child.write_bytes(raw);ref['sha256']=hashlib.sha256(raw).hexdigest()
            descriptor.write_text(json.dumps(dict(states=[ref])))
            return subprocess.run([str(INDEX),'--merge-usgwd',str(descriptor),str(out)],capture_output=True)
        result=merge(original+b' '*(1024*1024+1));self.assertEqual(result.returncode,0,result.stderr.decode())
        self.assertEqual(json.loads((out/'index.json').read_bytes())['indexedRecords'],1)
        before=(out/'index.json').read_bytes()
        duplicate=original[:-1]+b',"sourceState":"New Hampshire"}'+b' '*(1024*1024+1)
        result=merge(duplicate);self.assertNotEqual(result.returncode,0)
        self.assertEqual((out/'index.json').read_bytes(),before)
        result=merge(original+b' '*(8*1024*1024));self.assertNotEqual(result.returncode,0)
        self.assertEqual((out/'index.json').read_bytes(),before)

    def test_state_federation_combines_overlapping_cells_without_location_dedup(self):
        out = self.root/'national'
        states = [self.make_state_index('New Hampshire', 'NH_SOURCE_A', out), self.make_state_index('Rhode Island', 'RI_SOURCE_A', out)]
        descriptor = self.root/'states.json'; descriptor.write_text(json.dumps(dict(states=states, blockedStates=['Nebraska', 'South Carolina'])))
        command = [str(INDEX), '--merge-usgwd', str(descriptor), str(out)]
        result = subprocess.run(command, capture_output=True); self.assertEqual(result.returncode, 0, result.stderr.decode())
        index = json.loads((out/'index.json').read_bytes())
        self.assertEqual(index['indexedRecords'], 2); self.assertEqual(index['catalogRecords'], 2)
        self.assertEqual(len(index['children']), 2); self.assertEqual(index['regions'], [])
        overview = json.loads((out/index['aggregates']['overview']['url']).read_bytes())
        for level in overview['levels']:
            self.assertEqual(sum(c['count'] for c in level['cells']), 2)
            self.assertEqual(len(level['cells']), 1)
        before = (out/'index.json').read_bytes()
        result = subprocess.run(command, capture_output=True); self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertEqual((out/'index.json').read_bytes(), before)
        # Same release ID in different files is a collision, regardless of location.
        states[1] = self.make_state_index('Rhode Island', 'NH_SOURCE_A', out)
        descriptor.write_text(json.dumps(dict(states=states)))
        result = subprocess.run(command, capture_output=True); self.assertNotEqual(result.returncode, 0)
        self.assertEqual((out/'index.json').read_bytes(), before)


if __name__ == '__main__':
    unittest.main()
