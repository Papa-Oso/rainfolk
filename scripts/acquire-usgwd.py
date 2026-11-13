"""USGWD acquisition against a pinned HydroShare release manifest.

Commands:
  pin    LISTING_PAGE... --out release.json [--expect SHA]
         Build a pinned release from retained HydroShare file-listing pages.
  check  release.json --status status.json
         Fetch the live listing (two small requests), compare to the pin and
         classify differences. Exit 0: pinned data unchanged. Exit 3: review.
  fetch  release.json STATE --out DIR [--geospatial-fallback]
         Download exactly one pinned state file, verify size and publisher MD5,
         and write the existing import receipt next to it.

The release fingerprint is SHA-256 of compact JSON of sorted
(relative path, size, checksum) triples, as in docs/USGWD.md. Only files named
in the pin are ever downloaded, by exact path; look-alike upstream entries are
reported, never used. One attempt per file; no automatic retries.
"""
import argparse, datetime, hashlib, importlib.util, json, re, sys, time, urllib.parse, urllib.request, zipfile
from pathlib import Path, PurePosixPath

RESOURCE = '8b02895f02c14dd1a749bcc5584a5c55'
HOST = 'www.hydroshare.org'
LISTING = f'https://{HOST}/hsapi/resource/{RESOURCE}/files/'
CONTENTS = f'/resource/{RESOURCE}/data/contents/'
SCHEMA_PATH = 'Metadata/Table_S3-Definitions_of_attributes_in_our_dataset.xlsx'
ATTRIBUTION = ('U.S. Groundwater Well Database (USGWD), HydroShare resource '
               f'{RESOURCE}; original creators and citation at the resource landing page')
AGENT = 'rainfolk-usgwd-acquire/1'
PAGE_CAP, MAX_PAGES = 1024**2, 4
CSV_CAP, ZIP_CAP, SCHEMA_CAP = 192 * 1024**2, 64 * 1024**2, 4 * 1024**2
SOCKET_SECONDS, ARCHIVE_WAIT_SECONDS, POLL_SECONDS = 60, 900, 10
STRAY_BYTES = 1024
STATE_RE = re.compile(r'[A-Za-z][A-Za-z ]{1,40}')
DATA_RE = re.compile(r'(USGWD-Tabular/USGWD_[A-Z][A-Za-z_]+\.csv|USGWD-Geospatial/USGWD_[A-Z][A-Za-z_]+\.zip|Metadata/[^/]+\.xlsx)')


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat(timespec='seconds')


def sha256(b):
    return hashlib.sha256(b).hexdigest()


def relative(url):
    path = urllib.parse.unquote(urllib.parse.urlparse(url).path)
    if CONTENTS not in path:
        raise ValueError(f'listing URL outside resource contents: {url}')
    return path.split(CONTENTS, 1)[1]


def entries(pages):
    files = {}
    for page in pages:
        for f in page['results']:
            path = relative(f['url'])
            if path in files:
                raise ValueError(f'duplicate listing path {path}')
            files[path] = dict(size=int(f['size']), checksum=str(f['checksum']), modified=f.get('modified_time'))
    return files


def fingerprint(files):
    triples = sorted([p, f['size'], f['checksum']] for p, f in files.items())
    return sha256(json.dumps(triples, separators=(',', ':')).encode())


def checked_url(url):
    """HydroShare itself, or its published-content bucket for this resource only."""
    u = urllib.parse.urlparse(url)
    bucket = (u.hostname == 'storage.googleapis.com' and (
        re.fullmatch(rf'/hydroshare-[a-z0-9-]+-published/{RESOURCE}/data/contents/.+', u.path)
        # prepared archive returned by HydroShare's own task-status reply
        or re.fullmatch(r'/hydroshare-[a-z0-9-]+-resource/tmp/[0-9a-f-]{36}\.zip', u.path)))
    if u.scheme != 'https' or not (u.hostname == HOST or bucket):
        raise ValueError(f'refusing URL outside HydroShare: {u.scheme}://{u.hostname}{u.path}')
    return url


class SameHostRedirects(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        checked_url(urllib.parse.urljoin(req.full_url, newurl))
        return super().redirect_request(req, fp, code, msg, headers, newurl)


OPENER = urllib.request.build_opener(SameHostRedirects)


def get(url, cap, sink=None):
    """GET with a hard byte cap. Returns (bytes or None, content-type, headers)."""
    req = urllib.request.Request(checked_url(url), headers={'User-Agent': AGENT})
    with OPENER.open(req, timeout=SOCKET_SECONDS) as r:
        kind = r.headers.get('Content-Type', '')
        declared = r.headers.get('Content-Length')
        if declared and int(declared) > cap:
            raise ValueError(f'{url}: declared {declared} bytes exceeds cap {cap}')
        got, chunks = 0, []
        while chunk := r.read(1024 * 1024):
            got += len(chunk)
            if got > cap:
                raise ValueError(f'{url}: response exceeds cap {cap}')
            (sink.write(chunk) if sink else chunks.append(chunk))
        headers = {k: r.headers.get(k) for k in ('ETag', 'Last-Modified', 'Content-Length', 'Content-Type')}
        return (None if sink else b''.join(chunks)), kind, headers


def live_pages():
    pages, url = [], LISTING
    while url:
        if len(pages) == MAX_PAGES:
            raise ValueError('listing page cap reached')
        body, _, _ = get(url, PAGE_CAP)
        page = json.loads(body)
        pages.append(page)
        url = page.get('next')
    return pages


def load_pin(path):
    pin = json.loads(Path(path).read_text(encoding='utf-8'))
    if pin.get('schemaVersion') != 1 or pin.get('resourceId') != RESOURCE:
        raise ValueError('unsupported release pin')
    if fingerprint(pin['files']) != pin['releaseSha256']:
        raise ValueError('release pin does not match its own fingerprint')
    return pin


def cmd_pin(a):
    pages = [json.loads(Path(p).read_bytes()) for p in a.pages]
    files = entries(pages)
    release = fingerprint(files)
    if a.expect and release != a.expect:
        raise SystemExit(f'fingerprint {release} != expected {a.expect}')
    pin = dict(schemaVersion=1, resourceId=RESOURCE, releaseSha256=release, pinnedAt=now(),
               pages=[dict(file=str(p), sha256=sha256(Path(p).read_bytes())) for p in a.pages], files=files)
    out = Path(a.out)
    if out.exists():
        raise SystemExit(f'{out} already exists')
    out.write_text(json.dumps(pin, indent=1) + '\n', encoding='utf-8')
    print(json.dumps(dict(releaseSha256=release, files=len(files))))


def classify(pinned, live):
    report = dict(unchangedData=0, changedData=[], removedData=[], newData=[], strayEntries=[], otherChanges=[])
    for path, f in pinned.items():
        cur = live.get(path)
        data = bool(DATA_RE.fullmatch(path))
        if cur is None:
            (report['removedData'] if data else report['otherChanges']).append(path)
        elif (cur['size'], cur['checksum']) != (f['size'], f['checksum']):
            (report['changedData'] if data else report['otherChanges']).append(path)
        elif data:
            report['unchangedData'] += 1
    for path, f in live.items():
        if path in pinned:
            continue
        if DATA_RE.fullmatch(path) and f['size'] >= STRAY_BYTES:
            report['newData'].append(path)
        else:
            report['strayEntries'].append(dict(path=path, size=f['size'], modified=f['modified']))
    return report


def cmd_check(a):
    pin = load_pin(a.release)
    status = dict(schemaVersion=1, job='usgwd-source-check', checkedAt=now(), pinnedRelease=pin['releaseSha256'])
    try:
        live = entries(live_pages())
        report = classify(pin['files'], live)
        review = bool(report['changedData'] or report['removedData'] or report['newData'])
        status.update(liveRelease=fingerprint(live), liveFiles=len(live), **report,
                      outcome='review-needed' if review else 'pinned-data-unchanged')
        code = 3 if review else 0
    except Exception as e:  # surfaced in the status file for the scheduler
        status.update(outcome='check-failed', error=f'{type(e).__name__}: {e}')
        code = 2
    text = json.dumps(status, indent=1) + '\n'
    if a.status:
        Path(a.status).parent.mkdir(parents=True, exist_ok=True)
        Path(a.status).write_text(text, encoding='utf-8')
    print(text, end='')
    return code


def download(path, pinned, dest, cap):
    """Download one pinned path into dest; handle HydroShare's async archive reply."""
    if path not in pinned:
        raise ValueError(f'{path} is not in the pinned release')
    want = pinned[path]
    url = f'https://{HOST}{CONTENTS}{urllib.parse.quote(path)}'
    part = dest.with_name(dest.name + '.partial')
    if dest.exists() or part.exists():
        raise ValueError(f'{dest} already exists; use a fresh output directory')
    evidence = dict(path=path, url=url, startedAt=now())
    try:
        with part.open('xb') as f:
            _, kind, headers = get(url, cap, f)
    except Exception:
        part.unlink(missing_ok=True)
        raise
    evidence['headers'] = headers
    head = part.read_bytes()[:65536] if part.stat().st_size < 65536 else b''
    if 'json' in kind.lower() and head:
        reply = json.loads(head)
        if not reply.get('task_id'):
            raise ValueError(f'unexpected JSON reply for {path}: {reply}')
        evidence['archiveTask'] = reply
        part.unlink()
        payload = wait_for_archive(reply['task_id'], evidence)
        archive = dest.with_name(dest.name + '.archive.zip')
        with archive.open('xb') as f:
            get(payload, cap * 2, f)
        member = PurePosixPath(path).name
        with zipfile.ZipFile(archive) as z:
            names = [i for i in z.infolist() if PurePosixPath(i.filename).name == member]
            if len(names) != 1 or names[0].file_size > cap:
                raise ValueError(f'archive does not hold exactly one bounded {member}')
            with z.open(names[0]) as src, part.open('xb') as out:
                while b := src.read(1024 * 1024):
                    out.write(b)
        evidence['archiveSha256'] = sha256(archive.read_bytes())
        archive.unlink()
    raw = part.read_bytes()
    evidence.update(bytes=len(raw), md5=hashlib.md5(raw).hexdigest(), sha256=sha256(raw), finishedAt=now(),
                    publisherSize=want['size'], publisherMd5=want['checksum'])
    evidence['verified'] = len(raw) == want['size'] and evidence['md5'] == want['checksum']
    if evidence['verified']:
        part.rename(dest)
    else:
        rejected = dest.with_name(dest.name + '.rejected')
        part.rename(rejected)
        evidence['rejectedFile'] = rejected.name
    return evidence


def wait_for_archive(task_id, evidence):
    if not re.fullmatch(r'[A-Za-z0-9-]{8,64}', str(task_id)):
        raise ValueError('invalid archive task id')
    deadline = time.monotonic() + ARCHIVE_WAIT_SECONDS
    polls = 0
    while time.monotonic() < deadline:
        time.sleep(POLL_SECONDS)
        polls += 1
        body, _, _ = get(f'https://{HOST}/hsapi/taskstatus/{task_id}/', PAGE_CAP)
        state = json.loads(body)
        if str(state.get('status')).lower() in ('true', 'completed', 'success'):
            evidence['archivePolls'] = polls
            return urllib.parse.urljoin(f'https://{HOST}/', state['payload'])
        if str(state.get('status')).lower() in ('false', 'failed', 'aborted'):
            raise ValueError(f'archive task failed: {state}')
    raise TimeoutError('archive task did not finish in time')


def receipt(pin, state, csv_path, url, modified, schema_sha):
    raw = csv_path.read_bytes()
    return dict(schemaVersion=1, resourceId=RESOURCE, releaseSha256=pin['releaseSha256'], inputSha256=sha256(raw),
                inputBytes=len(raw), state=state, sourceUrl=url, checkedAt=now(), sourcePublishedAt=None,
                sourceFileModifiedAt=modified, sourcePublishedTimeZone=None, license='CC-BY-4.0',
                attribution=ATTRIBUTION, schemaSha256=schema_sha)


def cmd_fetch(a):
    pin = load_pin(a.release)
    if not STATE_RE.fullmatch(a.state):
        raise SystemExit('invalid state name')
    stem = 'USGWD_' + a.state.replace(' ', '_')
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    log = dict(schemaVersion=1, job='usgwd-fetch', state=a.state, releaseSha256=pin['releaseSha256'], steps=[])

    def save_log():
        (out / 'acquisition.json').write_text(json.dumps(log, indent=1) + '\n', encoding='utf-8')

    try:
        schema = download(SCHEMA_PATH, pin['files'], out / 'schema.xlsx', SCHEMA_CAP)
        log['steps'].append(schema)
        if not schema['verified']:
            raise ValueError('schema workbook failed publisher checksum')
        csv_path = f'USGWD-Tabular/{stem}.csv'
        step = download(csv_path, pin['files'], out / f'{stem}.csv', CSV_CAP)
        log['steps'].append(step)
        if step['verified']:
            body = receipt(pin, a.state, out / f'{stem}.csv', f'https://{HOST}{CONTENTS}{csv_path}',
                           pin['files'][csv_path]['modified'], schema['sha256'])
            (out / 'source.json').write_text(json.dumps(body, indent=2) + '\n', encoding='utf-8')
            log['outcome'] = 'verified-csv'
        elif a.geospatial_fallback:
            zip_path = f'USGWD-Geospatial/{stem}.zip'
            zstep = download(zip_path, pin['files'], out / f'{stem}.zip', ZIP_CAP)
            log['steps'].append(zstep)
            if not zstep['verified']:
                raise ValueError('CSV and geospatial ZIP both failed publisher checksums')
            template = out / 'template.json'
            template.write_text(json.dumps(receipt(pin, a.state, out / f'{stem}.zip', f'https://{HOST}{CONTENTS}{zip_path}',
                                                   pin['files'][zip_path]['modified'], schema['sha256'])), encoding='utf-8')
            spec = importlib.util.spec_from_file_location('usgwd_dbf', Path(__file__).with_name('usgwd-dbf.py'))
            dbf = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(dbf)
            log['derivation'] = dbf.convert(out / f'{stem}.zip', template, out / f'{stem}.derived.csv', out / 'source.json', zstep['md5'])
            template.unlink()
            log['outcome'] = 'verified-geospatial-derived-csv'
        else:
            log['outcome'] = 'csv-checksum-mismatch'
    except Exception as e:
        log.update(outcome='failed', error=f'{type(e).__name__}: {e}')
    log['finishedAt'] = now()
    save_log()
    print(json.dumps({k: log[k] for k in ('state', 'outcome') if k in log} | ({'error': log['error']} if 'error' in log else {})))
    return 0 if log['outcome'].startswith('verified') else 3


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='cmd', required=True)
    s = sub.add_parser('pin'); s.add_argument('pages', nargs='+'); s.add_argument('--out', required=True); s.add_argument('--expect')
    s = sub.add_parser('check'); s.add_argument('release'); s.add_argument('--status')
    s = sub.add_parser('fetch'); s.add_argument('release'); s.add_argument('state'); s.add_argument('--out', required=True)
    s.add_argument('--geospatial-fallback', action='store_true')
    a = p.parse_args(argv)
    return {'pin': cmd_pin, 'check': cmd_check, 'fetch': cmd_fetch}[a.cmd](a) or 0


if __name__ == '__main__':
    sys.exit(main())
