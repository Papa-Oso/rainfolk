"""Reproduce the existing thin consumer from local Git objects; no fetch/install.

Only a NEW ignored directory inside this Rainfolk checkout may be written.
The supplied existing GEV checkout is read-only; no worktree, config or reset.
"""
from pathlib import Path
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
base, target = map(lambda s: Path(s).resolve(), sys.argv[1:])
scratch = ROOT/'.temp-gev'
if not target.is_relative_to(scratch) or target.exists():
    raise SystemExit('Require a new output directory inside Rainfolk .temp-gev')
target.mkdir(parents=True)
pins = json.loads((HERE/'consumer-pins.json').read_bytes())
revision = pins['gevBase']
def original(name):
    if Path(name).is_absolute() or '..' in Path(name).parts or name.startswith(('.git/', '.env')):
        raise ValueError('Unsafe source path')
    return subprocess.check_output(['git', 'show', revision+':'+name], cwd=base)
for entry in pins['basePatches']:
    patch = HERE/entry['path']; data = patch.read_bytes()
    if hashlib.sha256(data).hexdigest() != entry['sha256']:
        raise ValueError('Base patch hash mismatch')
    for name in re.findall(r'^--- a/(.+)$', data.decode(), re.M):
        p = target/name; p.parent.mkdir(parents=True, exist_ok=True); p.write_bytes(original(name))
    for check in [True, False]:
        subprocess.run(['git', 'apply', *(['--check'] if check else []), '--directory', target.relative_to(ROOT).as_posix(), str(patch)], cwd=ROOT, check=True)
patch = HERE/'usgwd-consumer.patch'
for check in [True, False]:
    subprocess.run(['git', 'apply', *(['--check'] if check else []), '--directory', target.relative_to(ROOT).as_posix(), str(patch)], cwd=ROOT, check=True)
shutil.copyfile(HERE/'historicalPack.mjs', target/'src/layers/usGroundwater/historicalPack.mjs')
shutil.copyfile(HERE/'dataBase.mjs', target/'src/layers/usGroundwater/dataBase.mjs')
for p in (ROOT/'fixtures/us-groundwater/v1').iterdir():
    if p.is_file():
        dest=target/'fixtures/us-groundwater/v1'/p.name;dest.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(p,dest)
for p in (HERE/'rainfolk-water').iterdir():
    if p.is_file():
        dest=target/'examples/rainfolk-water'/p.name;dest.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(p,dest)
pending = [target/'examples/rainfolk-water/main.js']; seen=set()
while pending:
    p=pending.pop(); rel=p.relative_to(target)
    if rel in seen: continue
    seen.add(rel)
    if len(seen)>300: raise ValueError('Relative-import closure cap')
    if not p.is_file():
        p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(original(rel.as_posix()))
    if p.stat().st_size>2*1024**2: raise ValueError('Source file byte cap')
    if p.suffix in ['.js','.mjs','.css']:
        for ref in re.findall(r'''(?:from\s*|import\s*\(?|url\(\s*)["'](\.[^"']+)["']''',p.read_text(encoding='utf-8')):
            child=(p.parent/ref).resolve()
            if not child.is_relative_to(target): raise ValueError('Relative import escaped source')
            if child.suffix in ['.js','.mjs','.css','.json','.svg','.woff','.woff2']: pending.append(child)
for entry in pins['updatedFiles']:
    b=(target/entry['path']).read_bytes().replace(b'\r\n',b'\n')
    if hashlib.sha256(b).hexdigest()!=entry['afterSha256']: raise ValueError('Applied source hash mismatch')
for entry in pins.get('uiPatches', []):
    patch = HERE/entry['path']
    if hashlib.sha256(patch.read_bytes()).hexdigest() != entry['sha256']:
        raise ValueError('UI patch hash mismatch')
    for check in [True, False]:
        subprocess.run(['git', 'apply', *(['--check'] if check else []), '--directory', target.relative_to(ROOT).as_posix(), str(patch)], cwd=ROOT, check=True)
print(json.dumps(dict(cleanBase=revision,patchesVerified=True,closureFiles=len(seen),sourceCheckoutModified=False,networkRequests=0)))
