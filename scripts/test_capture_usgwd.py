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
        r=subprocess.run([str(BINARY),'import-usgwd',*map(str,paths),str(self.root/out),str(allowance)],capture_output=True)
        if okay:self.assertEqual(r.returncode,0,r.stderr.decode());return json.loads(r.stdout)
        self.assertNotEqual(r.returncode,0);return r
    def payload(self,out='output'):
        return json.loads((self.root/out/'usgwd-manifest.json').read_bytes())['payload']
    def index(self,out='output'):
        return [r for ref in self.payload(out)['idIndex'] for r in json.loads((self.root/out/ref['path']).read_bytes())['ids']]
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
        rows += [rows[0],dict(rows[1],County='Invented conflict'),dict(rows[2],**{'Well Depth (Feet)':'-1'})]
        paths=self.source(rows);first=self.run_import(paths,allowance=1)
        self.assertFalse(first['snapshotFullyRead']);self.assertFalse((self.root/'output/usgwd-manifest.json').exists())
        final=self.run_import(paths,allowance=1)
        self.assertEqual(final['accounting'],dict(inputRows=204,acceptedRecords=199,identicalDuplicateRows=1,quarantinedRows=4,mapEligibleRecords=199,nonmappedRecords=0))
        self.assertTrue(final['snapshotFullyRead']);self.assertEqual(len(self.index()),201)
        self.run_import(paths,'fresh');self.assertEqual(self.index(),self.index('fresh'))
        paths=self.source(list(reversed(rows)));self.run_import(paths,'reverse');self.assertEqual(self.index(),self.index('reverse'))
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

if __name__=='__main__':unittest.main()
