"""Bounded synthetic SDR normalization/replay proof; no transport or index writes."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import copy

ROOT = Path(__file__).resolve().parents[1]


def compact(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':')).encode()


def digest(value):
    return hashlib.sha256(compact(value)).hexdigest()


def resumable_fixture(source, work):
    """Two invented pages through the existing well-jobs CLI, no HTTP."""
    jobs=ROOT/'crates/us-groundwater/target/debug'/('well-jobs.exe' if os.name=='nt' else 'well-jobs')
    with tempfile.TemporaryDirectory(prefix='resume-',dir=work) as directory:
        stage=Path(directory)
        paths=[]
        selected=[source['response']['features'][:2],source['response']['features'][1:3]]
        fields=('WellReportTrackingNumber','ProposedUse','LatitudeDD','LongitudeDD','BoreholeDepthFt','LocationError','ObjectId')
        for i,rows in enumerate(selected):
            page=copy.deepcopy(source)
            page.update(kind='rainfolk-texas-sdr-public-capture',synthetic=False,
                queryUrl=('https://services.twdb.texas.gov/arcgis/rest/services/Public/WellReports/FeatureServer/0/query?f=pjson&where=ObjectId%3E'
                    +str(i*2)+'&outFields='+'%2C'.join(fields)+'&returnGeometry=false&orderByFields=ObjectId%20ASC&resultRecordCount=2'),
                sourceResponseSha256=hashlib.sha256(b'{}').hexdigest(),sourceSchemaSha256=hashlib.sha256(b'{}').hexdigest(),
                fieldLimitations={'DateOfWellCompletion':'source-query-api-400'},fieldLimitationsEvidenceSha256='c'*64,
                pagination=dict(afterObjectId=i*2,objectIds=[i*2+1,i*2+2]))
            page['response']['features']=copy.deepcopy(rows)
            page['response']['spatialReferenceBasis']='source-response; attributes-unverified'
            page['response']['exceededTransferLimit']=i==0
            for row in page['response']['features']:
                row['attributes'].pop('DateOfWellCompletion',None)
            path=stage/f'fixture-{i}.json'
            path.write_bytes(compact(page))
            paths.append(path.relative_to(ROOT).as_posix())
        config=dict(schemaVersion=1,adapter='texas-sdr',mode='fixture',dateOmissionEvidenceSha256='c'*64,fixturePages=paths,
            budgets=dict(rowsPerPage=2,runPages=1,jobPages=2,jobRequests=4,jobBytes=2097152,jobRows=4,jobSeconds=300,runSeconds=60,rateMillis=1000))
        config_path=stage/'queue.json';config_path.write_bytes(compact(config))
        command=[str(jobs),'run',str(config_path),str(stage/'job')]
        first=json.loads(subprocess.check_output(command))
        command[1]='resume';second=json.loads(subprocess.check_output(command))
        assert first['progress']['accounting']['acceptedRecords']==2
        assert second['progress']['accounting']['acceptedRecords']==3
        assert second['progress']['accounting']['identicalDuplicateRows']==1
        assert second['progress']['accounting']['nonmappedRecords']==1
        assert not second['complete'] and second['sourceExhausted']
        assert second['reservedRequests']==4 and second['knownCommittedTransportRequests']==0
        assert json.loads(subprocess.check_output(command))==second
        return dict(synthetic=True,acceptedRecords=3,duplicateRows=1,nonmappedRecords=1,complete=False,
                    cursor=second['cursor'],reservedRequestAllowance=4,actualSourceRequests=0,resumeReplay=True)


def main():
    binary = ROOT / 'crates/us-groundwater/target/debug' / (
        'gev-us-groundwater.exe' if os.name == 'nt' else 'gev-us-groundwater')
    capture = (ROOT / 'fixtures/texas-sdr/synthetic-v1.json').read_bytes()
    env = dict(os.environ)
    env.pop('USGS_API_KEY', None)
    env.pop('OPENAI_API_KEY', None)
    command = [str(binary), '--texas-sdr-v1']
    first = subprocess.run(command, input=capture, capture_output=True, check=True, env=env).stdout
    second = subprocess.run(command, input=capture, capture_output=True, check=True, env=env).stdout
    assert first == second
    candidate = json.loads(first)
    manifest, payload = candidate['manifest'], candidate['payload']
    assert manifest['input']['sha256'] == hashlib.sha256(capture).hexdigest()
    assert candidate['manifestSha256'] == digest(manifest)
    assert manifest['payload']['sha256'] == digest(payload)
    assert manifest['payload']['bytes'] == len(compact(payload))
    assert payload['accounting'] == dict(inputRows=4, acceptedRecords=4,
                                      identicalDuplicateRows=0, quarantinedRows=0,
                                      mapEligibleRecords=3, nonmappedRecords=1)
    for record in payload['records']:
        content = {k: v for k, v in record.items() if k not in ('contentSha256', 'provenance')}
        assert record['contentSha256'] == digest(content)
    work = ROOT / '.temp-gev/texas-sdr-offline'
    work.mkdir(parents=True, exist_ok=True)
    (work / 'candidate.json').write_bytes(first)
    resume=resumable_fixture(json.loads(capture),work)
    print(json.dumps(dict(synthetic=True, acceptedRecords=4, quarantinedRows=0,
                          mapEligibleRecords=3, nonmappedRecords=1,
                          identicalReplay=True, hashesVerified=True, liveRecords=0,
                          payloadSha256=manifest['payload']['sha256'],resumableJob=resume)))


if __name__ == '__main__':
    main()
