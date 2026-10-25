"""Six-site, byte-safe export/index/replay proof. No network or credential use."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import uuid

ROOT = Path(__file__).resolve().parents[1]


def main():
    work = ROOT / '.temp-gev' / ('offline-example-' + uuid.uuid4().hex)
    work.mkdir(parents=True)
    suffix = '.exe' if os.name == 'nt' else ''
    binaries = ROOT / 'crates/us-groundwater/target/debug'
    env = dict(os.environ)
    env.pop('USGS_API_KEY', None)
    env.pop('OPENAI_API_KEY', None)
    capture = (ROOT / 'fixtures/us-groundwater/v1/capture.json').read_bytes()
    result = subprocess.run([str(binaries / ('gev-us-groundwater' + suffix)), '--bundle-v1'],
                            input=capture, capture_output=True, check=True, env=env)
    candidate = work / 'candidate.json'
    candidate.write_bytes(result.stdout)
    descriptor = work / 'batches.json'
    descriptor.write_text(json.dumps({'snapshot': 'offline-six-site-example', 'candidates': [
        {'path': 'candidate.json', 'sha256': hashlib.sha256(result.stdout).hexdigest()}]}), encoding='utf-8')
    command = [str(binaries / ('well_index' + suffix)), str(descriptor), str(work / 'index')]
    first = subprocess.run(command, capture_output=True, check=True, env=env)
    summary = json.loads(first.stdout)
    assert summary['indexedRecords'] == 6, summary
    previous = (work / 'index/index.json').read_bytes()
    subprocess.run(command, capture_output=True, check=True, env=env)
    assert (work / 'index/index.json').read_bytes() == previous
    candidate.write_bytes(b'{}')
    rejected = subprocess.run(command, capture_output=True, env=env)
    assert rejected.returncode != 0
    assert (work / 'index/index.json').read_bytes() == previous
    print(json.dumps({'acceptedIds': 6, 'identicalReplay': True, 'tamperingRejected': True,
                      'lastGoodRetained': True, 'indexSha256': hashlib.sha256(previous).hexdigest()}))


if __name__ == '__main__':
    main()
