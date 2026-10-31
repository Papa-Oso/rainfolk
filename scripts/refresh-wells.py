"""Finite local snapshot refresh; no network, scheduler activation or publication."""
import argparse
from contextlib import contextmanager
from contextvars import ContextVar
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
GIB = 1024**3
INPUT_CAP = 512*1024**2
META_CAP = 16*1024**2
POLICY = 'usgwd-local-refresh-v1'
ACTIVE_BUDGET = ContextVar('refresh-budget',default=None)


class Lifetime:
    """Hard parent watchdog also covers blocked local file I/O."""
    def __init__(self,seconds):
        self.started=time.time();self.deadline=self.started+seconds;self.child=None
        self.timer=threading.Timer(seconds,self.expire);self.timer.daemon=True;self.timer.start()

    def expire(self):
        try:
            if self.child is not None:
                self.child.kill()
                self.child.wait(timeout=2)
        finally:
            os._exit(124)  # Persisted running marker prevents an uncertain retry.

    def tighten(self,deadline):
        if deadline < self.deadline:
            self.timer.cancel();self.deadline=deadline
            self.timer=threading.Timer(max(0,deadline-time.time()),self.expire)
            self.timer.daemon=True;self.timer.start()

    def close(self):
        self.timer.cancel()


def sha(b):
    return hashlib.sha256(b).hexdigest()


def encode(v):
    return json.dumps(v, sort_keys=True, separators=(',', ':'), ensure_ascii=False, allow_nan=False).encode()


def pairs(items):
    result = {}
    for k, v in items:
        if k in result:
            raise ValueError('duplicate JSON member')
        result[k] = v
    return result


def read(path, cap=META_CAP):
    with Path(path).open('rb') as f:
        b = f.read(cap+1)
    if len(b) > cap:
        raise ValueError('metadata byte cap')
    return b


def load(path):
    return json.loads(read(path), object_pairs_hook=pairs)


def atomic(path, value):
    b = encode(value)
    if len(b) > META_CAP:
        raise ValueError('checkpoint byte cap')
    pending = path.with_suffix('.pending')
    with pending.open('wb') as f:
        f.write(b); f.flush(); os.fsync(f.fileno())
    pending.replace(path)


def save(path, payload):
    atomic(path, dict(schemaVersion=1, sha256=sha(encode(payload)), payload=payload))


def checkpoint(path, default):
    if not path.exists():
        return default
    e = load(path)
    if e.get('schemaVersion') != 1 or e.get('sha256') != sha(encode(e.get('payload'))):
        raise ValueError('checkpoint integrity')
    return e['payload']


@contextmanager
def writer(path):
    # OS advisory lock releases on process death; a running child is separately
    # recorded before launch, so an uncertain parent crash still refuses resume.
    with path.open('a+b') as f:
        if f.seek(0, 2) == 0:
            f.write(b'0'); f.flush()
        f.seek(0)
        if os.name == 'nt':
            import msvcrt
            msvcrt.locking(f.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
        try:
            yield
        finally:
            if os.name == 'nt':
                f.seek(0); msvcrt.locking(f.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(f, fcntl.LOCK_UN)


def fingerprint(path, deadline):
    if path.stat().st_size > INPUT_CAP:
        raise ValueError('source byte/time budget')
    digest = hashlib.sha256(); size = 0
    with path.open('rb') as f:
        while b := f.read(1024*1024):
            size += len(b)
            if size > INPUT_CAP or time.time() >= deadline:
                raise ValueError('source byte/time budget')
            digest.update(b)
    return digest.hexdigest(), size


def pinned(root, ref):
    relative = Path(ref['path'])
    if relative.anchor or relative.drive or '..' in relative.parts:
        raise ValueError('unsafe artifact reference')
    p = root/relative
    if any(q.is_symlink() or (hasattr(q,'is_junction') and q.is_junction()) for q in [p,*p.parents]):
        raise ValueError('linked artifact path refused')
    if sha(read(p)) != ref['sha256']:
        raise ValueError('pinned artifact integrity')
    return p


def reference(root, path):
    return dict(path=path.relative_to(root).as_posix(), sha256=sha(read(path)))


def peak_memory(child):
    if os.name == 'nt':
        import ctypes
        from ctypes import wintypes
        class PMC(ctypes.Structure):
            _fields_ = [('cb', wintypes.DWORD), ('PageFaultCount', wintypes.DWORD)]+[(k, ctypes.c_size_t) for k in ['PeakWorkingSetSize', 'WorkingSetSize', 'QuotaPeakPagedPoolUsage', 'QuotaPagedPoolUsage', 'QuotaPeakNonPagedPoolUsage', 'QuotaNonPagedPoolUsage', 'PagefileUsage', 'PeakPagefileUsage']]
        p = PMC(); p.cb = ctypes.sizeof(p)
        query = ctypes.WinDLL('psapi').GetProcessMemoryInfo
        query.argtypes = [wintypes.HANDLE, ctypes.POINTER(PMC), wintypes.DWORD]
        if not query(int(child._handle), ctypes.byref(p), p.cb):
            if child.poll() is not None:
                return 0
            raise ValueError('memory monitor unavailable')
        return int(p.PeakWorkingSetSize)
    status = Path(f'/proc/{child.pid}/status')
    if not status.exists() and child.poll() is not None:
        return 0
    for line in status.read_text().splitlines():
        if line.startswith('VmHWM:'):
            return int(line.split()[1])*1024
    raise ValueError('memory monitor unavailable')


def invoke(args, job, jobfile, seconds):
    if job['attempts'] >= 24 or time.time()+seconds > job['deadlineAt']:
        raise ValueError('remaining persisted operation/time reservation exhausted')
    if shutil.disk_usage(jobfile.parent).free < 24*GIB:
        raise ValueError('free disk guard')
    job['attempts'] += 1
    job['running'] = True
    save(jobfile, job)  # Crash here or after spawn is deliberately uncertain.
    prefix = f"attempt-{job['attempts']:02}"
    stdout = jobfile.parent/(prefix+'.stdout.log')
    stderr = jobfile.parent/(prefix+'.stderr.log')
    peak = 0; start = time.monotonic(); category = None
    with stdout.open('wb') as out, stderr.open('wb') as err:
        child = subprocess.Popen(args, stdout=out, stderr=err, close_fds=True)
        budget=ACTIVE_BUDGET.get()
        if budget is not None:budget.child=child
        try:
            job['childPid'] = child.pid; save(jobfile, job)
            while child.poll() is None:
                peak = max(peak, peak_memory(child))
                if time.monotonic()-start >= seconds or time.time() >= job['deadlineAt']:
                    category = 'child deadline'
                elif peak > 2*GIB:
                    category = 'child observed memory guard'
                elif stdout.stat().st_size > 1024**2 or stderr.stat().st_size > 1024**2:
                    category = 'child diagnostic byte guard'
                elif shutil.disk_usage(jobfile.parent).free < 24*GIB:
                    category = 'free disk guard'
                if category:
                    child.kill(); break
                time.sleep(.1)
        except BaseException:
            child.kill(); child.wait()
            raise  # Running marker requires explicit stopped-child acknowledgment.
        finally:
            if budget is not None:budget.child=None
        child.wait()
        peak = max(peak, peak_memory(child))
    if peak > 2*GIB:
        category = 'child observed memory guard'
    if stdout.stat().st_size > 1024**2 or stderr.stat().st_size > 1024**2:
        category = 'child diagnostic byte guard'
    job['running'] = False
    job['lastChild'] = dict(exitCode=child.returncode, category=category, peakBytes=peak,
                            elapsedSeconds=time.monotonic()-start,attempt=job['attempts'])
    diagnostic = dict(job['lastChild'])
    diagnostic['command'] = args[1]
    diagnostic['stdout'] = dict(path=stdout.name,bytes=stdout.stat().st_size,sha256=sha(read(stdout,1024**2)) if stdout.stat().st_size<=1024**2 else None)
    diagnostic['stderr'] = dict(path=stderr.name,bytes=stderr.stat().st_size,sha256=sha(read(stderr,1024**2)) if stderr.stat().st_size<=1024**2 else None)
    atomic(jobfile.parent/(prefix+'.json'),diagnostic)
    if child.returncode or category:
        job['failed'] = True; save(jobfile, job)
        raise ValueError('child failed; last candidate retained; manual --resume required')
    try:
        result = json.loads(read(stdout, 1024**2), object_pairs_hook=pairs)
    except (ValueError,OSError):
        job['failed'] = True;save(jobfile,job)
        raise ValueError('invalid child summary; last candidate retained') from None
    save(jobfile, job)
    return result


def configuration(config_path):
    config = load(config_path)
    if set(config)-{'schemaVersion','profile','runSeconds','sources','schedule'}:
        raise ValueError('unsupported refresh config fields')
    if config.get('schemaVersion') != 1 or config.get('profile') != 'usgwd-local-snapshot':
        raise ValueError('unsupported refresh profile')
    sources = config.get('sources', [])
    seconds = config.get('runSeconds', 7200)
    if not isinstance(seconds, int) or not 1800 <= seconds <= 21600 or not 1 <= len(sources) <= 50:
        raise ValueError('finite refresh budgets')
    keys = [s['key'] for s in sources]
    if any(set(s) != {'key','csv','receipt'} for s in sources):
        raise ValueError('unsupported source profile fields')
    if len(set(keys)) != len(keys) or any(not re.fullmatch('[a-z0-9][a-z0-9-]{0,63}', k) for k in keys):
        raise ValueError('invalid or duplicate source key')
    if config.get('schedule', {}).get('enabled', False) is not False:
        raise ValueError('schedule activation is not supported by this local command')
    return config, sources, seconds


def run(config_path, root, steps=32, resume=False, ack_stopped=False):
    budget=Lifetime(21600)
    token=ACTIVE_BUDGET.set(budget)
    try:
        return _run(config_path,root,steps,resume,ack_stopped)
    finally:
        ACTIVE_BUDGET.reset(token);budget.close()


def _run(config_path, root, steps=32, resume=False, ack_stopped=False):
    for path in [Path(config_path).absolute(),Path(root).absolute()]:
        if any(q.is_symlink() or (hasattr(q,'is_junction') and q.is_junction()) for q in [path,*path.parents]):
            raise ValueError('linked refresh config/output refused')
    config_path = Path(config_path).resolve(); root = Path(root).resolve()
    config, sources, seconds = configuration(config_path)
    if not 1 <= steps <= 32:
        raise ValueError('finite step allowance')
    config_sha = sha(encode(config))
    budget=ACTIVE_BUDGET.get()
    invocation_deadline = budget.started+seconds
    budget.tighten(invocation_deadline)
    if root.exists() and not (root/'refresh-state.json').exists():
        raise ValueError('fresh directory or existing owned refresh state required')
    root.mkdir(parents=True, exist_ok=True)
    with writer(root/'runner.lock'):
        state = checkpoint(root/'refresh-state.json', dict(kind='rainfolk-local-refresh', policy=POLICY, sources={}))
        if state['kind'] != 'rainfolk-local-refresh' or state.get('policy') != POLICY:
            raise ValueError('refresh state owner')
        # Marker exists before any child or source copy; never adopt dataset dirs.
        save(root/'refresh-state.json', state)
        checked = []
        for item in sources:
            if time.time() >= invocation_deadline:
                raise ValueError('refresh invocation deadline')
            csv = config_path.parent/item['csv']
            receipt_path = config_path.parent/item['receipt']
            if any(q.is_symlink() or (hasattr(q,'is_junction') and q.is_junction()) for p in [csv,receipt_path] for q in [p,*p.parents]):
                raise ValueError('linked inbox path refused')
            csv=csv.resolve();receipt_path=receipt_path.resolve()
            receipt = load(receipt_path)
            if csv.suffix.lower() != '.csv' or receipt.get('resourceId') != '8b02895f02c14dd1a749bcc5584a5c55':
                raise ValueError('USGWD CSV/receipt profile required')
            receipt_bytes = encode(receipt)
            contract = dict(receipt);contract.pop('checkedAt',None)
            contract_sha = sha(encode(contract))
            source_sha, size = fingerprint(csv, invocation_deadline)
            if receipt.get('inputSha256') != source_sha or receipt.get('inputBytes') != size:
                raise ValueError('inbox receipt/source fingerprint mismatch')
            scope = [receipt.get('resourceId'), receipt.get('state')]
            previous = state['sources'].get(item['key'])
            if previous:
                if previous['scope'] != scope:
                    raise ValueError('source key cannot change namespace/state')
                pinned(root, previous['manifest']); pinned(root, previous['index']); pinned(root, previous['completion'])
            identity = sha(encode([scope, receipt.get('releaseSha256'), source_sha]))
            checked.append(item['key'])
            if previous and previous['fingerprint'] == identity:
                if previous.get('sourceContractSha256') != contract_sha:
                    raise ValueError('unchanged source bytes but receipt contract changed; review required')
                previous['localCheckedAt'] = datetime.now(timezone.utc).isoformat()
                save(root/'refresh-state.json', state)
                continue
            directory = root/'runs'/item['key']/identity
            jobfile = directory/'job.json'
            binary = ROOT/'crates/us-groundwater/target/debug'/('well-jobs.exe' if os.name=='nt' else 'well-jobs')
            index_binary = binary.with_name('well_index.exe' if os.name=='nt' else 'well_index')
            toolchain = dict(jobs=sha(read(binary,64*1024**2)), index=sha(read(index_binary,64*1024**2)), runner=sha(read(Path(__file__))))
            if not directory.exists():
                if shutil.disk_usage(root).free < 80*GIB:
                    raise ValueError('80GiB free reservation required for new state refresh')
                directory.mkdir(parents=True)
                job = dict(fingerprint=identity, policy=POLICY, toolchain=toolchain, configSha256=config_sha, receiptSha256=sha(receipt_bytes),
                           startedAt=time.time(), deadlineAt=invocation_deadline, attempts=0, stage='import', running=False,
                           previous=previous, scope=scope, phaseSeconds=900)
                save(jobfile, job)
            job = checkpoint(jobfile, None)
            if job['configSha256'] != config_sha or job['receiptSha256'] != sha(receipt_bytes) or job.get('policy') != POLICY or job.get('toolchain') != toolchain:
                raise ValueError('job config/receipt/toolchain changed; no budget reset')
            if job.get('running'):
                if not ack_stopped:
                    raise ValueError('uncertain child; verify stopped before --ack-stopped')
                stale_index_lock = directory/'map/.well-index.lock'
                if stale_index_lock.exists():
                    if job['stage'] != 'export' or read(stale_index_lock, 128).decode().strip() != str(job.get('childPid')):
                        raise ValueError('index lock does not match acknowledged child')
                    stale_index_lock.unlink()
                job['running'] = False; job['stoppedAcknowledgedAt'] = datetime.now(timezone.utc).isoformat()
                save(jobfile, job)
            if job.get('failed') and not resume:
                raise ValueError('failed refresh held; explicit --resume required')
            if time.time() >= job['deadlineAt']:
                raise ValueError('persisted refresh deadline expired; no automatic renewal')
            budget.tighten(job['deadlineAt'])
            saved_csv = directory/'source.csv'
            saved_receipt = directory/'source.json'
            if not saved_csv.exists():
                pending = directory/'source.pending'
                with csv.open('rb') as f, pending.open('wb') as out:
                    for b in iter(lambda:f.read(1024**2), b''):
                        if out.tell()+len(b)>INPUT_CAP or time.time()>=job['deadlineAt']:
                            raise ValueError('snapshot copy byte/time guard')
                        out.write(b)
                    out.flush(); os.fsync(out.fileno())
                if fingerprint(pending, job['deadlineAt']) != (source_sha, size):
                    raise ValueError('inbox changed during snapshot copy')
                pending.replace(saved_csv)
            if not saved_receipt.exists():
                atomic(saved_receipt, receipt)  # Repair crash between CSV rename and receipt save.
            if fingerprint(saved_csv, job['deadlineAt']) != (source_sha, size) or read(saved_receipt) != receipt_bytes:
                raise ValueError('saved snapshot integrity')
            for _ in range(steps):
                if job['stage'] == 'import':
                    result = invoke([str(binary),'import-usgwd-segmented',str(saved_csv),str(saved_receipt),str(directory/'normalized')], job, jobfile, 300)
                    if result.get('snapshotFullyRead'):
                        job['manifest'] = reference(root, directory/'normalized/usgwd-segmented-manifest.json')
                        job['stage'] = 'diff'
                elif job['stage'] == 'diff':
                    before = str(pinned(root, job['previous']['manifest'])) if job['previous'] else '-'
                    result = invoke([str(binary),'diff-usgwd',before,str(pinned(root,job['manifest'])),str(directory/'diff'),'900'], job,jobfile,900)
                    report = directory/'diff'/result['path']
                    if sha(read(report)) != result['sha256']:
                        job['failed']=True;save(jobfile,job)
                        raise ValueError('diff result integrity')
                    job['diff'] = reference(root, report); job['requiresReview'] = result['requiresReview']; job['stage'] = 'export'
                elif job['stage'] == 'export':
                    result = invoke([str(index_binary),'--usgwd',str(pinned(root,job['manifest'])),str(directory/'map'),'900'],job,jobfile,900)
                    index_path = directory/'map/index.json'
                    if sha(read(index_path)) != result['indexSha256']:
                        job['failed']=True;save(jobfile,job)
                        raise ValueError('export result integrity')
                    job['index'] = reference(root, index_path); job['stage'] = 'complete'
                else:
                    break
                job['failed'] = False; save(jobfile, job)
            if job['stage'] != 'complete':
                return dict(status='paused', key=item['key'], stage=job['stage'], attempts=job['attempts'], deadlineAt=job['deadlineAt'])
            completion = dict(schemaVersion=1,kind='rainfolk-refresh-candidate',fingerprint=identity,
                              manifest=job['manifest'],index=job['index'],diff=job['diff'],previous=job['previous'],
                              requiresReview=job['requiresReview'],activation='manual-only',remoteSourceChecked=False,
                              execution=dict(policy=POLICY,toolchain=job['toolchain'],configSha256=job['configSha256'],attempts=job['attempts'],startedAt=job['startedAt'],deadlineAt=job['deadlineAt']))
            complete_path = directory/'complete.json'
            if complete_path.exists() and load(complete_path) != completion:
                raise ValueError('immutable completion mismatch')
            if not complete_path.exists(): atomic(complete_path, completion)
            entry = dict(fingerprint=identity,scope=scope,sourceContractSha256=contract_sha,manifest=job['manifest'],index=job['index'],completion=reference(root,complete_path),
                         localCheckedAt=datetime.now(timezone.utc).isoformat(),requiresReview=job['requiresReview'])
            state['sources'][item['key']] = entry
            save(root/'refresh-state.json', state)
            return dict(status='changed-state-candidate-ready',key=item['key'],affectedState=scope[1],candidate=entry,
                        reusedSources=[k for k in state['sources'] if k!=item['key']],publication='manual-only',remoteSourceChecked=False)
        return dict(status='unchanged',checkedSources=checked,reusedSources=checked,remoteSourceChecked=False)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('config',type=Path);p.add_argument('directory',type=Path)
    p.add_argument('--steps',type=int,default=32)
    p.add_argument('--resume',action='store_true');p.add_argument('--ack-stopped',action='store_true')
    p.add_argument('--plan',action='store_true',help='Validate config and print the inactive entry point; no state/source writes')
    args=p.parse_args()
    try:
        if args.plan:
            config,sources,seconds=configuration(args.config)
            print(json.dumps(dict(policy=POLICY,profile=config['profile'],sources=[s['key'] for s in sources],runSeconds=seconds,
                entryPoint=['python','-B','scripts/refresh-wells.py',str(args.config),str(args.directory)],
                networkRequests=0,scheduleInstalled=False,publication='manual-only')))
            return
        print(json.dumps(run(args.config,args.directory,args.steps,args.resume,args.ack_stopped)))
    except Exception as e:
        # Errors from our fixed commands contain no source record values.
        print('refresh stopped: '+str(e),file=__import__('sys').stderr)
        raise SystemExit(1)


if __name__=='__main__':
    main()
