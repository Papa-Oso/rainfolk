"""Synthetic, offline USGWD CSV admission and checkpoint regressions."""
import csv
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]
FIXTURE=ROOT/'fixtures/usgwd/synthetic.csv'
BINARY=ROOT/'crates/us-groundwater/target/debug'/('well-jobs.exe' if os.name=='nt' else 'well-jobs')
SCRATCH=ROOT/'.temp-gev/usgwd-tests'
SCRATCH.mkdir(exist_ok=True)
RESOURCE='8b02895f02c14dd1a749bcc5584a5c55'

class UsgwdTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory(dir=SCRATCH);self.root=Path(self.tmp.name)
        with FIXTURE.open(encoding='utf-8',newline='') as f:
            self.header=next(csv.reader(f));f.seek(0);self.rows=list(csv.DictReader(f))
    def tearDown(self):self.tmp.cleanup()
    def source(self,rows=None,raw=None,release='a'*64):
        if raw is None:
            s=io.StringIO(newline='');w=csv.DictWriter(s,self.header,lineterminator='\r\n');w.writeheader();w.writerows(self.rows if rows is None else rows);raw=s.getvalue().encode()
        csv_path=self.root/'source.csv';csv_path.write_bytes(raw)
        source=dict(schemaVersion=1,resourceId=RESOURCE,releaseSha256=release,inputSha256=hashlib.sha256(raw).hexdigest(),inputBytes=len(raw),state='New Hampshire',
            sourceUrl=f'https://www.hydroshare.org/resource/{RESOURCE}/data/contents/USGWD-Tabular/USGWD_New_Hampshire.csv',
            checkedAt='2026-10-27T00:00:00Z',sourcePublishedAt=None,sourceFileModifiedAt='2024-03-08T14:22:12',sourcePublishedTimeZone=None,
            license='CC-BY-4.0',attribution='Invented regression records; no downloaded data',schemaSha256='b'*64)
        receipt=self.root/'source.json';receipt.write_text(json.dumps(source));return csv_path,receipt
    def run_import(self,paths,out='output',allowance=10,okay=True):
        args=[str(BINARY),'import-usgwd',*map(str,paths),str(self.root/out)]
        if allowance is not None:args.append(str(allowance))
        r=subprocess.run(args,capture_output=True)
        if okay:self.assertEqual(r.returncode,0,r.stderr.decode());return json.loads(r.stdout)
        self.assertNotEqual(r.returncode,0);return r
    def payload(self,out='output'):
        return json.loads((self.root/out/'usgwd-manifest.json').read_bytes())['payload']
    def index(self,out='output'):
        return [r for ref in self.payload(out)['idIndex'] for r in json.loads((self.root/out/ref['path']).read_bytes())['ids']]
    def segment(self,paths,out='segmented',rows=200,okay=True,allowance=None):
        args=[str(BINARY),'import-usgwd-segmented',*map(str,paths),str(self.root/out),str(rows)]
        if allowance is not None:args.append(str(allowance))
        r=subprocess.run(args,capture_output=True)
        if okay:self.assertEqual(r.returncode,0,r.stderr.decode());return json.loads(r.stdout)
        self.assertNotEqual(r.returncode,0);return r
    def segmented_index(self,out='segmented'):
        p=json.loads((self.root/out/'usgwd-segmented-manifest.json').read_bytes())['payload']
        return [row for group in p['idIndexGroups'] for ref in json.loads((self.root/out/group['path']).read_bytes())['idIndex'] for row in json.loads((self.root/out/ref['path']).read_bytes())['ids']]
    def test_derived_dbf_receipt_is_explicit_and_strict(self):
        paths=self.source();source=json.loads(paths[1].read_bytes());source.update(schemaVersion=2,sourceUrl=f'https://www.hydroshare.org/resource/{RESOURCE}/data/contents/USGWD-Geospatial/USGWD_New_Hampshire.zip',derivation=dict(policy='usgwd-dbf-attributes-v1',archiveSha256='c'*64,archiveMd5='d'*32,member='USGWD_New Hampshire.dbf',memberSha256='e'*64,sourceCrs='EPSG:4269',recordCount=len(self.rows)))
        paths[1].write_text(json.dumps(source));self.run_import(paths);self.assertEqual(self.payload()['source'],source)
        source['derivation']['sourceCrs']='EPSG:4326';paths[1].write_text(json.dumps(source));self.run_import(paths,'bad',okay=False)
    def test_segment_boundaries_reconcile_duplicates_conflicts_and_resume_exactly(self):
        rows=[dict(self.rows[0],**{'Well ID':f'SEG_{i}'}) for i in range(201)]
        rows += [rows[0],dict(rows[1],County='Invented conflict'),dict(rows[2],**{'Year Reported':'invalid'})]
        paths=self.source(rows);first=self.segment(paths)
        self.assertFalse(first['snapshotFullyRead']);self.assertEqual(first['segments'],1)
        self.assertFalse((self.root/'segmented/usgwd-segmented-manifest.json').exists())
        final=self.segment(paths);single=self.run_import(paths,'single',allowance=None)
        self.assertEqual(final['accounting'],single['accounting'])
        self.assertEqual(self.segmented_index(),self.index('single'))
        manifest=self.root/'segmented/usgwd-segmented-manifest.json';checkpoint=self.root/'segmented/checkpoint-usgwd-segments.json'
        before=(manifest.read_bytes(),checkpoint.read_bytes())
        replay=self.segment(paths);self.assertTrue(replay['snapshotFullyRead'])
        self.assertEqual(before,(manifest.read_bytes(),checkpoint.read_bytes()))
        self.assertEqual(replay['newSegments'],0)
        p=json.loads(manifest.read_bytes())['payload'];self.assertFalse(p['complete']);self.assertFalse(p['existingIndexEligible'])
        source=json.loads(paths[1].read_bytes());self.assertEqual(p['source']['inputSha256'],source['inputSha256'])
        records=[];parts=[];indices=[]
        for ref in p['segments']:
            d=self.root/'segmented'/ref['directory'];m=json.loads((d/ref['path']).read_bytes())['payload']
            parts.extend(m['partitions'])
            for part in m['partitions']:records.extend(json.loads((d/part['path']).read_bytes())['records'])
        for group in p['idIndexGroups']:indices.extend(json.loads((self.root/'segmented'/group['path']).read_bytes())['idIndex'])
        self.assertEqual(parts,self.payload('single')['partitions']);self.assertEqual(indices,self.payload('single')['idIndex'])
        self.assertEqual([r['provenance']['sourceRow'] for r in records],list(range(2,205)))
        self.assertTrue(all(r['provenance']['inputSha256']==source['inputSha256'] for r in records))
    def test_segment_interruption_within_work_unit_replays_only_that_prefix(self):
        rows=[dict(self.rows[0],**{'Well ID':f'PAUSE_{i}'},County='Invented\nquoted,"county') for i in range(401)]
        rows[-1]=dict(rows[0],County='Conflicting last chunk')
        paths=self.source(rows);pause=self.segment(paths,rows=400,allowance=1)
        self.assertEqual(pause['segments'],0);self.assertEqual(pause['activeSegmentAccounting']['inputRows'],200)
        self.assertFalse((self.root/'segmented/usgwd-segmented-manifest.json').exists())
        middle=self.segment(paths,rows=400);self.assertEqual(middle['segments'],1)
        final=self.segment(paths,rows=400);single=self.run_import(paths,'single',allowance=None)
        self.assertEqual(final['accounting'],single['accounting']);self.assertEqual(self.segmented_index(),self.index('single'))
        self.assertEqual(final['accounting']['quarantinedRows'],2)
    def test_segment_invalid_utf8_version_poisoning_and_reverse_order(self):
        good=self.rows[0];bad=dict(good,County='INVALID-UTF8-MARKER')
        filler=[dict(good,**{'Well ID':f'FILLER_{i}'}) for i in range(199)]
        for n,(first,last) in enumerate(((good,bad),(bad,good))):
            paths=self.source([first,*filler,last]);paths=self.source(raw=paths[0].read_bytes().replace(b'INVALID-UTF8-MARKER',b'\x81'))
            out=f'segment-utf8-{n}';self.segment(paths,out);r=self.segment(paths,out)
            self.assertEqual(r['accounting']['acceptedRecords'],199);self.assertEqual(r['accounting']['quarantinedRows'],2)
            self.run_import(paths,f'single-utf8-{n}',allowance=None)
            self.assertEqual(self.segmented_index(out),self.index(f'single-utf8-{n}'))
        self.assertEqual(self.segmented_index('segment-utf8-0'),self.segmented_index('segment-utf8-1'))
    def test_segment_tamper_source_change_and_reduce_only_limits_fail_closed(self):
        rows=[dict(self.rows[0],**{'Well ID':f'BOUND_{i}'}) for i in range(201)]
        paths=self.source(rows);self.segment(paths)
        self.segment(paths,rows=0,okay=False);self.segment(paths,rows=125200,okay=False)
        self.segment(paths,rows=400,okay=False)
        cp=json.loads((self.root/'segmented/checkpoint-usgwd-segments.json').read_bytes())['payload']
        ref=cp['segments'][0];m=self.root/'segmented'/ref['directory']/ref['path'];m.write_bytes(m.read_bytes()+b' ')
        self.segment(paths,okay=False)
        self.assertFalse((self.root/'segmented/usgwd-segmented-manifest.json').exists())
        paths[0].write_bytes(paths[0].read_bytes()+b'changed');self.segment(paths,'source-changed',okay=False)
    def test_segment_global_count_cap_and_anonymous_quarantine(self):
        rows=[dict(self.rows[0],**{'Well ID':f'LIMIT_{i}'}) for i in range(3201)]
        paths=self.source(rows)
        for n in range(16):self.assertEqual(self.segment(paths)['segments'],n+1)
        result=self.segment(paths,okay=False);self.assertIn(b'sixteen-segment cap',result.stderr)
        self.assertFalse((self.root/'segmented/usgwd-segmented-manifest.json').exists())
        rows=[dict(self.rows[0],**{'Well ID':f'ANON_{i}'}) for i in range(201)]
        rows[-1]=dict(rows[-1],**{'Well ID':'invalid id'})
        paths=self.source(rows);self.segment(paths,'anonymous');final=self.segment(paths,'anonymous')
        single=self.run_import(paths,'anonymous-single',allowance=None)
        self.assertEqual(final['accounting'],single['accounting']);self.assertEqual(final['accounting']['quarantinedRows'],1)
    def test_smoke_retains_nonspatial_state_id_collisions_strings_crs_and_replay(self):
        paths=self.source();result=self.run_import(paths)
        self.assertEqual(result['accounting'],dict(inputRows=2,acceptedRecords=2,identicalDuplicateRows=0,quarantinedRows=0,mapEligibleRecords=1,nonmappedRecords=1))
        p=self.payload();r=json.loads((self.root/'output'/p['partitions'][0]['path']).read_bytes())['records'][0]
        self.assertEqual(r['coordinateReference'],'EPSG:4269');self.assertEqual(r['fips'],'00001');self.assertEqual(r['huc12'],'000000000001')
        self.assertFalse(r['existingIndexEligible']);self.assertEqual(r['sourceValues']['Flag Duplicate'],'3');self.assertIsNone(r['sourceSystem'])
        self.assertFalse(p['complete']);self.assertTrue(p['snapshotFullyRead']);before=(self.root/'output/checkpoint-usgwd.json').read_bytes()
        replay=self.run_import(paths);self.assertEqual(replay['manifestSha256'],result['manifestSha256']);self.assertEqual(replay['newPartitions'],0)
        self.assertEqual(before,(self.root/'output/checkpoint-usgwd.json').read_bytes())
    def test_interruption_resume_cross_partition_duplicate_conflict_and_invalid_version(self):
        rows=[dict(self.rows[0],**{'Well ID':f'SYN_{i}'}) for i in range(201)]
        rows += [rows[0],dict(rows[1],County='Invented conflict'),dict(rows[2],**{'Year Reported':'invalid'})]
        paths=self.source(rows);first=self.run_import(paths,allowance=1)
        self.assertFalse(first['snapshotFullyRead']);self.assertFalse((self.root/'output/usgwd-manifest.json').exists())
        final=self.run_import(paths,allowance=None)
        self.assertEqual(final['prefixReplayRows'],200)
        self.assertEqual(final['accounting'],dict(inputRows=204,acceptedRecords=199,identicalDuplicateRows=1,quarantinedRows=4,mapEligibleRecords=199,nonmappedRecords=0))
        self.assertTrue(final['snapshotFullyRead']);self.assertEqual(len(self.index()),201)
        self.run_import(paths,'fresh');self.assertEqual(self.index(),self.index('fresh'))
        paths=self.source(list(reversed(rows)));self.run_import(paths,'reverse');self.assertEqual(self.index(),self.index('reverse'))
    def test_invalid_measurements_retain_records_raw_values_and_coordinates(self):
        rows=[]
        for field in ['Well Depth (Feet)','Screen Depth (Feet)','Length of Screen (Feet)','Well Capacity (GPM)','Surface Elevation (Feet)']:
            for value in ['-5','NaN','inf','not-a-number','Unknown','']:
                rows.append(dict(self.rows[0],**{'Well ID':f'QUALITY_{len(rows)}',field:value}))
        paths=self.source(rows);result=self.run_import(paths,allowance=None)
        self.assertEqual(result['accounting']['acceptedRecords'],len(rows))
        self.assertEqual(result['accounting']['quarantinedRows'],0)
        p=self.payload();records=[]
        for ref in p['partitions']:records.extend(json.loads((self.root/'output'/ref['path']).read_bytes())['records'])
        for original,record in zip(rows,records):
            self.assertEqual(record['sourceValues'],original)
            self.assertTrue(record['mapEligible'])
            for field in ['Well Depth (Feet)','Screen Depth (Feet)','Length of Screen (Feet)','Well Capacity (GPM)','Surface Elevation (Feet)']:
                raw=original[field];q=record['quantities'][field]
                invalid=raw in ['NaN','inf','not-a-number'] or raw=='-5' and field!='Surface Elevation (Feet)'
                if invalid:
                    self.assertEqual(q['quality'],'invalid');self.assertIsNone(q['value'])
                    self.assertIn({'field':field,'reason':q['issue']},record['quantityErrors'])
                elif raw in ['Unknown','']:
                    self.assertEqual(q['quality'],'missing');self.assertIsNone(q['value'])
                else:self.assertEqual(q['quality'],'valid')
        self.assertEqual(p['normalizationPolicy'],'usgwd-field-quality-v2')
        self.assertEqual(self.run_import(paths)['manifestSha256'],result['manifestSha256'])

    def test_previous_normalization_policy_refuses_resume_without_writes(self):
        paths=self.source();self.run_import(paths)
        cp=self.root/'output/checkpoint-usgwd.json';e=json.loads(cp.read_bytes())
        compact=lambda v:json.dumps(v,sort_keys=True,ensure_ascii=False,separators=(',',':')).encode()
        e['payload']['sourceFingerprint']=hashlib.sha256(compact(json.loads(paths[1].read_bytes()))).hexdigest()
        e['sha256']=hashlib.sha256(compact(e['payload'])).hexdigest();cp.write_bytes(compact(e));before=cp.read_bytes()
        manifest=(self.root/'output/usgwd-manifest.json').read_bytes()
        self.run_import(paths,okay=False)
        self.assertEqual(cp.read_bytes(),before);self.assertEqual((self.root/'output/usgwd-manifest.json').read_bytes(),manifest)

    def test_field_quality_conflicts_across_segments_still_quarantine_all_versions(self):
        rows=[dict(self.rows[0],**{'Well ID':f'QUALITY_BOUND_{i}'}) for i in range(200)]
        rows.append(dict(rows[0],**{'Length of Screen (Feet)':'-5'}))
        paths=self.source(rows);self.segment(paths);segmented=self.segment(paths)
        single=self.run_import(paths,'single',allowance=None)
        self.assertEqual(segmented['accounting'],single['accounting'])
        self.assertEqual(single['accounting']['quarantinedRows'],2)
        self.assertEqual(single['accounting']['acceptedRecords'],199)

    def test_tampered_partition_or_changed_receipt_refuses_resume(self):
        rows=[dict(self.rows[0],**{'Well ID':f'SYN_{i}'}) for i in range(201)];paths=self.source(rows);self.run_import(paths,allowance=1)
        cp=json.loads((self.root/'output/checkpoint-usgwd.json').read_bytes())['payload'];part=self.root/'output'/cp['partitions'][0]['path'];part.write_bytes(part.read_bytes()+b' ')
        self.run_import(paths,okay=False);self.assertFalse((self.root/'output/usgwd-manifest.json').exists())
        self.run_import(self.source(rows,release='c'*64),okay=False)
    def test_quoted_multiline_fields_and_invalid_coordinate_retention(self):
        rows=[dict(self.rows[0],County='Invented, "quoted"\ncounty',Longitude='not-a-number')];paths=self.source(rows);r=self.run_import(paths)
        self.assertEqual(r['accounting']['acceptedRecords'],1);self.assertEqual(r['accounting']['nonmappedRecords'],1)
    def test_invalid_utf8_and_extra_columns_quarantine_without_echoing_values(self):
        raw=FIXTURE.read_bytes();lines=raw.splitlines(keepends=True);paths=self.source(raw=lines[0]+lines[1].replace(b'SYN_1',b'Bad\x81')+lines[2].rstrip(b'\r\n')+b',SENTINEL\r\n')
        r=self.run_import(paths);self.assertEqual(r['accounting']['quarantinedRows'],2)
        self.assertNotIn(b'SENTINEL',(self.root/'output/usgwd-manifest.json').read_bytes())
    def test_unterminated_csv_and_schema_drift_never_commit_manifest(self):
        for i,raw in enumerate([FIXTURE.read_bytes()+b'"unterminated',FIXTURE.read_bytes().replace(b'Flag US',b'Unknown Header')]):
            self.run_import(self.source(raw=raw),f'bad{i}',okay=False);self.assertFalse((self.root/f'bad{i}/usgwd-manifest.json').exists())
    def test_source_hash_allowance_and_duplicate_receipt_keys_fail_closed(self):
        paths=self.source();paths[0].write_bytes(paths[0].read_bytes()+b'changed');self.run_import(paths,okay=False)
        paths=self.source();self.run_import(paths,allowance=0,okay=False)
        paths[1].write_text(paths[1].read_text()[:-1]+',"schemaVersion":1}');self.run_import(paths,okay=False)
    def test_input_budget_refuses_oversized_receipt_before_output(self):
        paths=self.source();source=json.loads(paths[1].read_text());source['inputBytes']=192*1024*1024+1
        paths[1].write_text(json.dumps(source));self.run_import(paths,okay=False)
        self.assertFalse((self.root/'output').exists())
    def test_normal_full_file_path_keeps_state_beyond_one_hundred_partitions(self):
        rows=[dict(self.rows[0],**{'Well ID':f'FULL_{i}'}) for i in range(20001)]
        r=self.run_import(self.source(rows),allowance=None)
        self.assertTrue(r['snapshotFullyRead']);self.assertFalse(r['complete'])
        self.assertEqual(r['partitions'],101);self.assertEqual(r['newPartitions'],101)
        self.assertEqual(r['prefixReplayRows'],0);self.assertEqual(r['prefixReplaySeconds'],0)
        self.assertEqual(r['accounting']['acceptedRecords'],20001)
        self.assertEqual(r['limits']['rows'],250000);self.assertEqual(r['limits']['invocationSeconds'],300)
    def test_explicit_full_mode_and_numeric_allowance_guards_remain(self):
        paths=self.source();r=self.run_import(paths,allowance='full')
        self.assertTrue(r['snapshotFullyRead']);self.assertEqual(r['prefixReplayRows'],0)
        self.run_import(paths,'too-many',allowance=101,okay=False)
    def test_invalid_utf8_in_another_field_quarantines_all_id_versions_in_both_orders(self):
        good=self.rows[0];bad=dict(good,County='INVALID-UTF8-MARKER')
        for n,rows in enumerate(([good,bad],[bad,good])):
            with self.subTest(order=n):
                paths=self.source(rows);raw=paths[0].read_bytes().replace(b'INVALID-UTF8-MARKER',b'\x81');paths=self.source(raw=raw)
                r=self.run_import(paths,f'utf8-{n}')
                self.assertEqual(r['accounting']['acceptedRecords'],0);self.assertEqual(r['accounting']['quarantinedRows'],2)
                index=self.index(f'utf8-{n}');self.assertEqual(len(index),1);self.assertEqual(index[0]['status'],'quarantined')
                self.assertEqual(index[0]['sourceRows'],2);self.assertIsNone(index[0]['contentSha256'])
    def test_invalid_utf8_id_collision_across_checkpoint_partition_in_both_orders(self):
        good=self.rows[0];bad=dict(good,County='INVALID-UTF8-MARKER')
        filler=[dict(good,**{'Well ID':f'FILLER_{i}'}) for i in range(199)]
        for n,(first,last) in enumerate(((good,bad),(bad,good))):
            with self.subTest(order=n):
                paths=self.source([first,*filler,last]);raw=paths[0].read_bytes().replace(b'INVALID-UTF8-MARKER',b'\x81');paths=self.source(raw=raw)
                out=f'cross-utf8-{n}';self.assertFalse(self.run_import(paths,out,allowance=1)['snapshotFullyRead'])
                r=self.run_import(paths,out,allowance=1)
                self.assertEqual(r['accounting'],dict(inputRows=201,acceptedRecords=199,identicalDuplicateRows=0,quarantinedRows=2,mapEligibleRecords=199,nonmappedRecords=0))
                index=self.index(out);self.assertEqual(sum(x['status']=='quarantined' for x in index),1)
        self.assertEqual(self.index('cross-utf8-0'),self.index('cross-utf8-1'))
    def test_documented_quality_domains_preserve_raw_and_suppress_invalid_spellings(self):
        cases=[('Flag State',' 1 ','source-state-or-US-inconsistency'),('Flag US',' 1 ','source-state-or-US-inconsistency')]
        for key in ['Flag County','Flag State','Flag US','Flag Duplicate']:
            cases += [(key,v,'invalid-source-quality-flag') for v in ['1.0','unexpected','']]
        cases += [('Flag County','3','invalid-source-quality-flag'),('Flag State','3','invalid-source-quality-flag'),
            ('Flag US','-1','invalid-source-quality-flag'),('Flag Duplicate','4','invalid-source-quality-flag'),
            ('Location Verified','unexpected','invalid-source-quality-flag')]
        for n,(key,raw,reason) in enumerate(cases):
            with self.subTest(field=key,raw=raw):
                out=f'flag-{n}';r=self.run_import(self.source([dict(self.rows[0],**{key:raw})]),out)
                self.assertEqual(r['accounting']['acceptedRecords'],1);self.assertEqual(r['accounting']['mapEligibleRecords'],0)
                p=self.payload(out);record=json.loads((self.root/out/p['partitions'][0]['path']).read_bytes())['records'][0]
                self.assertEqual(record['sourceValues'][key],raw);self.assertEqual(record['mapSuppressionReason'],reason)
        out='valid-flags';r=self.run_import(self.source([dict(self.rows[0],**{'Flag State':' 0 ','Flag US':'0','Flag Duplicate':'3'})]),out)
        self.assertEqual(r['accounting']['mapEligibleRecords'],1)
        p=self.payload(out);record=json.loads((self.root/out/p['partitions'][0]['path']).read_bytes())['records'][0]
        self.assertEqual(record['sourceValues']['Flag State'],' 0 ')

if __name__=='__main__':unittest.main()
