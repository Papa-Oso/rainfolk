"""Compose a national USGWD map root from published and newly built state indexes.

  python -B scripts/usgwd-national.py ROOT_INDEX_URL OUT_DIR --add STATE_MAP_DIR... [--blocked STATE...]

Reuses every state already in a published immutable root by hash: only each
state's small index.json and its *.counts.json summaries are mirrored (no detail
packs). Newly built states (well_index --usgwd output directories) are added,
then the existing `well_index --merge-usgwd` writes OUT_DIR/index.json.
States must share one release; the merge tool refuses mixed releases.
This writes local files only. Uploading the new root is a separate, approved step.
"""
import argparse, hashlib, json, os, re, shutil, subprocess, sys, urllib.parse, urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
INDEX = ROOT / 'crates/us-groundwater/target/debug' / ('well_index.exe' if os.name == 'nt' else 'well_index')
FEED_HOST = 'data.rainfolk.org'
REF_CAP, TOTAL_CAP = 8 * 1024**2, 512 * 1024**2
AGENT = 'rainfolk-usgwd-national/1'
fetched = 0


def sha(b):
    return hashlib.sha256(b).hexdigest()


def get(url, cap=REF_CAP):
    global fetched
    u = urllib.parse.urlparse(url)
    if u.scheme != 'https' or u.hostname != FEED_HOST:
        raise ValueError(f'refusing URL outside {FEED_HOST}: {url}')
    with urllib.request.urlopen(urllib.request.Request(url, headers={'User-Agent': AGENT}), timeout=60) as r:
        body = r.read(cap + 1)
    fetched += len(body)
    if len(body) > cap or fetched > TOTAL_CAP:
        raise ValueError(f'byte cap exceeded at {url}')
    return body


def pinned(base, ref, dest):
    """Fetch a hash-named reference, verify bytes and SHA-256, save under dest."""
    name = ref['url']
    if not re.fullmatch(r'[0-9a-f]{64}\.[a-z]+\.json', name) or not name.startswith(ref['sha256']):
        raise ValueError(f'unexpected reference {name}')
    body = get(urllib.parse.urljoin(base, name))
    if len(body) != ref['bytes'] or sha(body) != ref['sha256']:
        raise ValueError(f'integrity failure for {name}')
    (dest / name).write_bytes(body)


def mirror(root_url, out):
    body = get(root_url)
    m = re.search(r'/([0-9a-f]{64})/index\.json$', root_url)
    if not m or sha(body) != m.group(1):
        raise ValueError('published root does not match the hash in its URL')
    root = json.loads(body)
    refs = []
    for child in root['children']:
        state, base = child['state'], urllib.parse.urljoin(root_url, child['directory'])
        dest = out / 'states' / state.replace(' ', '_')
        dest.mkdir(parents=True, exist_ok=True)
        data = get(urllib.parse.urljoin(root_url, child['url']))
        if sha(data) != child['sha256'] or len(data) != child['bytes']:
            raise ValueError(f'{state}: child index integrity failure')
        (dest / 'index.json').write_bytes(data)
        agg = json.loads(data)['aggregates']
        pinned(base, agg['overview'], dest)
        for level in agg['levels']:
            for chunk in level['chunks']:
                pinned(base, chunk, dest)
        refs.append(dict(state=state, path=f'states/{dest.name}/index.json', sha256=child['sha256']))
    return root, refs


def add_local(map_dir, out):
    index = map_dir / 'index.json'
    data = index.read_bytes()
    state = json.loads(data)['sourceState']
    dest = out / 'states' / state.replace(' ', '_')
    if dest.exists():
        raise ValueError(f'{state} already present in the composed root')
    dest.mkdir(parents=True)
    shutil.copyfile(index, dest / 'index.json')
    for counts in map_dir.glob('*.counts.json'):
        shutil.copyfile(counts, dest / counts.name)
    return dict(state=state, path=f'states/{dest.name}/index.json', sha256=sha(data))


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('root_url'); p.add_argument('out', type=Path)
    p.add_argument('--add', type=Path, nargs='+', default=[]); p.add_argument('--blocked', nargs='*', default=[])
    a = p.parse_args(argv)
    if a.out.exists():
        raise SystemExit(f'{a.out} already exists; use a fresh directory')
    a.out.mkdir(parents=True)
    published, refs = mirror(a.root_url, a.out)
    refs += [add_local(d, a.out) for d in a.add]
    refs.sort(key=lambda r: r['state'])
    descriptor = a.out.parent / (a.out.name + '.states.json')
    descriptor.write_text(json.dumps(dict(states=refs, blockedStates=sorted(a.blocked)), indent=1), encoding='utf-8')
    result = subprocess.run([str(INDEX), '--merge-usgwd', str(descriptor), str(a.out)], capture_output=True, text=True)
    if result.returncode:
        raise SystemExit(f'merge failed: {result.stderr.strip()}')
    root = json.loads((a.out / 'index.json').read_bytes())
    summary = dict(states=len(root['children']), catalogRecords=root['catalogRecords'], indexedRecords=root['indexedRecords'],
                   unmappedRecords=root['unmappedRecords'], releaseSha256=root['snapshot']['releaseSha256'],
                   blockedStates=root['snapshot']['blockedStates'], rootSha256=sha((a.out / 'index.json').read_bytes()),
                   previous=dict(url=a.root_url, states=len(published['children']), catalogRecords=published['catalogRecords'],
                                 indexedRecords=published['indexedRecords']), mirroredBytes=fetched)
    (a.out.parent / (a.out.name + '.summary.json')).write_text(json.dumps(summary, indent=1) + '\n', encoding='utf-8')
    print(json.dumps(summary, indent=1))


if __name__ == '__main__':
    sys.exit(main())
