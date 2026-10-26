"""Bounded synthetic SDR normalization/replay proof; no transport or index writes."""
import hashlib
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def compact(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':')).encode()


def digest(value):
    return hashlib.sha256(compact(value)).hexdigest()


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
    print(json.dumps(dict(synthetic=True, acceptedRecords=4, quarantinedRows=0,
                          mapEligibleRecords=3, nonmappedRecords=1,
                          identicalReplay=True, hashesVerified=True, liveRecords=0,
                          payloadSha256=manifest['payload']['sha256'])))


if __name__ == '__main__':
    main()
