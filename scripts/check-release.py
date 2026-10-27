"""Offline candidate integrity and bounded sensitive-content checks; no secret reads."""
import hashlib
import json
import csv
from pathlib import Path
import re
import subprocess
from urllib.parse import parse_qsl, urlsplit

ROOT = Path(__file__).resolve().parents[1]


def main():
    manifest_path = ROOT / 'docs/release-files.json'
    manifest = json.loads(manifest_path.read_text(encoding='utf-8'))
    actual = set(subprocess.check_output(
        ['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z'], cwd=ROOT
    ).decode('utf-8').split('\0')) - {''}
    listed = {row['path'] for row in manifest['files']}
    assert actual == listed | {'docs/release-files.json'}, 'Release file set differs'
    # The SDR source release includes only this reviewed, invented fixture.
    # Real public captures and diagnostics remain ignored local evidence.
    sdr_fixture = 'fixtures/texas-sdr/synthetic-v1.json'
    assert {name for name in listed if name.startswith('fixtures/texas-sdr/')} == {
        'fixtures/texas-sdr/README.md', 'fixtures/texas-sdr/SHA256SUMS', sdr_fixture,
    }, 'Unexpected SDR fixture file'
    fixture = json.loads((ROOT / sdr_fixture).read_text(encoding='utf-8'))
    assert fixture.get('synthetic') is True and fixture.get('kind') == \
        'rainfolk-texas-sdr-capture', 'SDR release fixture must be synthetic'
    assert {name for name in listed if name.startswith('fixtures/usgwd/')} == {
        'fixtures/usgwd/README.md', 'fixtures/usgwd/synthetic.csv',
    }, 'Unexpected USGWD fixture file'
    with (ROOT / 'fixtures/usgwd/synthetic.csv').open(encoding='utf-8', newline='') as stream:
        invented = list(csv.DictReader(stream))
    assert [r['Well ID'] for r in invented] == ['SYN_1', 'SYN_2'], 'USGWD fixture must contain only the two invented IDs'
    issues = []
    urls = 0
    for row in manifest['files']:
        relative = Path(row['path'])
        assert not relative.is_absolute() and '..' not in relative.parts
        assert not any(x in relative.parts for x in ['target', '.temp-gev', '.git', 'node_modules', 'dist'])
        assert not relative.name.startswith('.env') and relative.suffix != '.clixml'
        data = (ROOT / relative).read_bytes()
        assert len(data) == row['bytes'] and hashlib.sha256(data).hexdigest() == row['sha256'], row['path']
        text = data.decode('utf-8')
        rules = {
            'machine-path': r'[A-Za-z]:[\\/](?:Users|Windows)[\\/]',
            'private-key': r'-----BEGIN (?:RSA |OPENSSH |EC )?PRIVATE KEY-----',
            'provider-token': r'\b(?:ghp_|github_pat_|sk-proj-)[A-Za-z0-9_\-]{16,}',
            'email': r'\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b',
        }
        for name, pattern in rules.items():
            # Preserve public author/contact notices in third-party legal texts.
            if name == 'email' and relative.parts[0] == 'LICENSES':
                continue
            if re.search(pattern, text):
                issues.append({'path': row['path'], 'rule': name})
        if relative.suffix == '.json' and 'fixtures' in relative.parts:
            def walk(value):
                nonlocal urls
                if isinstance(value, dict):
                    for key, child in value.items():
                        if re.fullmatch(r'owner|owner_name|owner_address|contact|email|phone|address', key, re.I):
                            issues.append({'path': row['path'], 'rule': 'personal-field'})
                        walk(child)
                elif isinstance(value, list):
                    for child in value:
                        walk(child)
                elif isinstance(value, str) and value.startswith(('https://', 'http://')):
                    urls += 1
                    url = urlsplit(value)
                    if url.username or url.password or any(key.lower() in
                            ['api_key', 'apikey', 'token', 'x-api-key'] for key, _ in parse_qsl(url.query)):
                        issues.append({'path': row['path'], 'rule': 'credential-url'})
            walk(json.loads(text))
    for checksum_file in (ROOT / 'fixtures/us-groundwater').glob('*/sha256.json'):
        for name, digest in json.loads(checksum_file.read_text(encoding='utf-8')).items():
            assert Path(name).name == name
            assert hashlib.sha256((checksum_file.parent / name).read_bytes()).hexdigest() == digest
    sdr_checksums = (ROOT / 'fixtures/texas-sdr/SHA256SUMS').read_text(encoding='utf-8')
    assert sdr_checksums == hashlib.sha256((ROOT / sdr_fixture).read_bytes()).hexdigest() + \
        '  synthetic-v1.json\n', 'SDR fixture checksum mismatch'
    assert not issues, json.dumps(issues)  # Locations/rules only, never matched values.
    print(json.dumps({'files': len(manifest['files']) + 1, 'bytesExcludingManifest':
        sum(row['bytes'] for row in manifest['files']), 'fixtureUrlsInspected': urls,
        'sensitivePatternFindings': 0, 'fixtureHashLists': 'pass', 'fileHashes': 'pass'}))


if __name__ == '__main__':
    main()
