"""Synthetic local ZIPs only; no network or real records in fixtures."""
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
import zipfile

SCRIPT=Path(__file__).with_name('capture-texas-sdr-bulk.py')
spec=importlib.util.spec_from_file_location('bulk_transport',SCRIPT)
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
SCRATCH=m.ROOT/'.temp-gev/bulk-tests';SCRATCH.mkdir(exist_ok=True)

def compact(v):return json.dumps(v,sort_keys=True,ensure_ascii=False,separators=(',',':')).encode()

def row(identifier='001',**changes):
    data={name:'' for name in m.HEADER}
    data.update(WellReportTrackingNumber=identifier,ProposedUse='Domestic',CoordDDLat='30.25',CoordDDLong='-97.75',
        DateSubmitted='2024-03-01',DrillingStartDate='2024-02-28',DrillingEndDate='2024-02-29',
        KnownLocationError='Yes',HorizontalDatumType='WGS84',LocationVerifiedByDriller='No',
        NumberOfWellsDrilled='1',OwnerName='PRIVATE-SENTINEL')
    data.update(changes)
    return ('|'.join(data[k] for k in m.HEADER)+'\r\n').encode('cp1252')

def fixture(root,rows,extra=None):
    archive=root/'source.zip'
    with zipfile.ZipFile(archive,'w',zipfile.ZIP_DEFLATED) as z:
        z.writestr(m.MEMBER,('|'.join(m.HEADER)+'\r\n').encode()+b''.join(rows))
        if extra:z.writestr(extra,b'not executed')
    meta=root/'source.json'
    meta.write_text(json.dumps(dict(sourceUrl=m.SOURCE,status='complete',sha256=m.sha_file(archive),bytes=archive.stat().st_size,
        finishedAt='2026-10-27T00:00:00Z',responseHeaders={'Last-Modified':'synthetic fixture'})),encoding='utf-8')
    return archive,meta

def manifest(path):return json.loads((path/'bulk-manifest.json').read_text(encoding='utf-8'))
def indices(path):
    return [item for ref in manifest(path)['payload']['idIndex'] for item in json.loads((path/ref['path']).read_text(encoding='utf-8'))['ids']]

class BulkTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory(dir=SCRATCH);self.root=Path(self.tmp.name)
    def tearDown(self):self.tmp.cleanup()
    def run_import(self,rows,name='output'):
        archive,meta=fixture(self.root,rows)
        output=self.root/name
        return m.run(archive,meta,output),output
    def test_literal_quotes_cp1252_calendar_dates_and_private_projection(self):
        result,out=self.run_import([row(ProposedUse='"Domestic cafÃ©"')])
        payload=manifest(out)['payload'];ref=payload['partitions'][0]
        raw=(out/ref['path']).read_bytes();record=json.loads(raw)['records'][0]
        self.assertNotIn(b'PRIVATE-SENTINEL',raw)
        self.assertEqual(record['proposedUse'],'"Domestic cafÃ©"')
        self.assertEqual(record['sourceDates']['DrillingEndDate'],dict(date='2024-02-29',precision='day',timeZone=None))
        self.assertIsNone(record['completedAt']);self.assertIsNone(record['boreholeDepth']['value'])
        self.assertEqual(record['knownLocationError'],'Yes');self.assertTrue(record['mapEligible'])
        self.assertEqual(record['provenance']['row'],2);self.assertEqual(record['id'],'TWDB-SDR-001')
        self.assertEqual(result['networkRequests'],0)
    def test_unknown_and_unconverted_datums_and_missing_coordinates_remain_nonspatial(self):
        result,out=self.run_import([row('001'),row('002',HorizontalDatumType=''),row('003',HorizontalDatumType='NAD27'),
            row('004',HorizontalDatumType='NAD83'),row('005',CoordDDLat='0'),row('006',CoordDDLat='')])
        self.assertEqual(result['accounting'],dict(inputRows=6,acceptedRecords=6,identicalDuplicateRows=0,quarantinedRows=0,mapEligibleRecords=1,nonmappedRecords=5))
        records=[r for ref in manifest(out)['payload']['partitions'] for r in json.loads((out/ref['path']).read_text())['records']]
        self.assertEqual(records[1]['sourceCoordinates']['latitude'],'30.25')
        self.assertIsNone(records[1]['latitude']);self.assertEqual(records[1]['mapSuppressionReason'],'source-datum-missing')
    def test_cross_partition_duplicates_conflicts_and_invalid_versions_have_no_order_winner(self):
        rows=[row(f'{i:03}') for i in range(1,202)]+[row('001'),row('002',ProposedUse='Monitor'),row('003').rstrip(b'\r\n')+b'|EXTRA\r\n']
        result,forward=self.run_import(rows,'forward')
        self.assertEqual(result['accounting'],dict(inputRows=204,acceptedRecords=199,identicalDuplicateRows=1,quarantinedRows=4,mapEligibleRecords=199,nonmappedRecords=0))
        result,reverse=self.run_import(list(reversed(rows)),'reverse')
        self.assertEqual(indices(forward),indices(reverse))
        self.assertEqual(len({x['id'] for x in indices(forward) if x['status']=='accepted'}),199)
    def test_invalid_encoding_date_and_id_are_value_free_quarantines(self):
        rows=[row('001',DrillingEndDate='2025-02-29'),row('002').replace(b'Domestic',b'Bad\x81'),row('BAD')]
        result,out=self.run_import(rows)
        self.assertEqual(result['accounting']['quarantinedRows'],3)
        self.assertEqual(result['accounting']['acceptedRecords'],0)
        self.assertIn('invalid-cp1252',result['rowQuarantineReasons'])
    def test_identical_input_manifest_and_all_artifact_hashes_replay(self):
        archive,meta=fixture(self.root,[row('001'),row('002',HorizontalDatumType='')])
        first=m.run(archive,meta,self.root/'a');second=m.run(archive,meta,self.root/'b')
        self.assertEqual(first['manifestSha256'],second['manifestSha256'])
        doc=manifest(self.root/'a');self.assertEqual(doc['sha256'],hashlib.sha256(compact(doc['payload'])).hexdigest())
        for ref in doc['payload']['partitions']+doc['payload']['idIndex']:
            raw=(self.root/'a'/ref['path']).read_bytes()
            self.assertEqual(len(raw),ref['bytes']);self.assertEqual(hashlib.sha256(raw).hexdigest(),ref['sha256'])
        with self.assertRaises(ValueError):m.run(archive,meta,self.root/'a')
    def test_zip_traversal_and_changed_hash_refuse_without_output(self):
        archive,meta=fixture(self.root,[row()],extra='../outside.txt')
        with self.assertRaises(ValueError):m.prepare(archive,meta)
        archive,meta=fixture(self.root,[row()])
        archive.write_bytes(archive.read_bytes()+b'changed')
        with self.assertRaises(ValueError):m.prepare(archive,meta)
    def test_reader_row_and_line_caps_leave_no_final_manifest(self):
        archive,meta=fixture(self.root,[row('001'),row('002')])
        previous=m.MAX_ROWS
        try:
            m.MAX_ROWS=1
            with self.assertRaises(ValueError):m.run(archive,meta,self.root/'row-cap')
        finally:m.MAX_ROWS=previous
        self.assertFalse((self.root/'row-cap/bulk-manifest.json').exists())
        archive,meta=fixture(self.root,[b'001|'+b'x'*m.LINE_CAP+b'\n'])
        with self.assertRaises(ValueError):m.run(archive,meta,self.root/'line-cap')
        self.assertFalse((self.root/'line-cap/bulk-manifest.json').exists())
    def test_truncated_or_duplicate_json_stream_never_commits_manifest(self):
        archive,meta=fixture(self.root,[row()]);header=m.prepare(archive,meta);messages=list(m.batches(archive,header))
        binary=m.ROOT/'crates/us-groundwater/target/debug/well-jobs.exe'
        if not binary.exists():binary=binary.with_suffix('')
        missing_id=json.loads(json.dumps(messages[0]));missing_id['rows'][0]['id']=None
        empty_header=dict(header,memberBytes=0)
        for i,raw in enumerate((m.encoded(header)+m.encoded(messages[0]),b'{"kind":"x","kind":"y"}\n',
                m.encoded(header)+m.encoded(missing_id)+m.encoded(messages[-1]),m.encoded(empty_header))):
            out=self.root/f'bad-{i}'
            result=subprocess.run([str(binary),'import-sdr-bulk',str(out)],input=raw,capture_output=True)
            self.assertNotEqual(result.returncode,0);self.assertFalse((out/'bulk-manifest.json').exists())
            self.assertNotIn(b'PRIVATE-SENTINEL',result.stderr)

if __name__=='__main__':unittest.main()
