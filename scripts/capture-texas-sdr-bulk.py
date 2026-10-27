"""Read one saved official WellData export into the existing offline Rust importer.
No network. Pipe delimiter, no qualifier, strict Windows-1252; real output stays ignored.
"""
import argparse
import csv
import hashlib
import json
import shutil
import stat
import subprocess
import threading
import time
import zipfile
from pathlib import Path, PurePosixPath

ROOT=Path(__file__).resolve().parents[1]
SOURCE='https://www.twdb.texas.gov/groundwater/data/SDRDownload.zip'
MEMBER='SDRDownload/WellData.txt'
HEADER=tuple(['WellReportTrackingNumber', 'DateSubmitted', 'OwnerName', 'OwnerAddress1', 'OwnerAddress2', 'OwnerCity', 'OwnerState', 'OwnerZip', 'OwnerOtherCountry', 'OwnerCountry', 'OwnerWellNumber', 'County', 'WellAddress1', 'WellAddress2', 'WellCity', 'WellZip', 'WellLocationDescription', 'NumberOfWellsDrilled', 'Elevation', 'CoordDDLat', 'Dlat', 'Mlat', 'Slat', 'CoordDDLong', 'Dlong', 'Mlong', 'Slong', 'GridNumber', 'LocationVerifiedByDriller', 'TypeOfWork', 'TypeOfWorkOtherDesc', 'OriginalWellReportTrackingNumber', 'ProposedUse', 'ProposedUseOtherDesc', 'TCEQApprovedPlans', 'PWSNo', 'DrillingStartDate', 'DrillingEndDate', 'SealMethod', 'SealMethodOtherDesc', 'DistanceToSepticOrOtherContamination', 'DistanceToSepticTank', 'DistanceToPropertyLine', 'DistanceVerificationMethod', 'ApprovedByVariance', 'SealedByDriller', 'SealedByName', 'SurfaceCompletion', 'SurfaceCompletionOtherDesc', 'CompletedByDriller', 'PumpType', 'PumpTypeOtherDesc', 'PumpDepth', 'ChemicalAnalysis', 'InjuriousWater', 'CompanyName', 'DrillerName', 'DrillerSigned', 'ApprenticeSigned', 'Comments', 'PluggedWithin48Hrs', 'PluggingReportTrackingNumber', 'KnownLocationError', 'HorizontalDatumType', 'DrillerAddress1', 'DrillerAddress2', 'DrillerCity', 'DrillerState', 'DrillerZip', 'DrillerOtherCountry', 'DrillerCountry', 'LicenseNumber', 'ApprenticeRegistrationNumber'])
FIELDS=('WellReportTrackingNumber','ProposedUse','CoordDDLat','CoordDDLong','DateSubmitted',
        'DrillingStartDate','DrillingEndDate','KnownLocationError','HorizontalDatumType',
        'LocationVerifiedByDriller','TypeOfWork','OriginalWellReportTrackingNumber','NumberOfWellsDrilled')
CAP=512*1024*1024
LINE_CAP=1024*1024
MAX_ROWS=1000000

def encoded(v):
    return (json.dumps(v,sort_keys=True,ensure_ascii=False,allow_nan=False,separators=(',',':'))+'\n').encode('utf-8')

def sha_file(path):
    h=hashlib.sha256()
    with path.open('rb') as f:
        while raw:=f.read(1048576):h.update(raw)
    return h.hexdigest()

def prepare(archive,receipt):
    meta=json.loads(receipt.read_text(encoding='utf-8'))
    if meta.get('sourceUrl')!=SOURCE or meta.get('status')!='complete':raise ValueError('unapproved source receipt')
    size=archive.stat().st_size
    if not 0<size<=CAP or meta.get('bytes')!=size or meta.get('sha256')!=sha_file(archive):raise ValueError('archive size/hash mismatch')
    with zipfile.ZipFile(archive) as z:
        infos=z.infolist()
        if len(infos)>4096 or sum(i.file_size for i in infos)>2*1024*1024*1024:raise ValueError('archive directory/expanded cap')
        seen=set()
        for i in infos:
            name=i.filename.replace('\\','/');p=PurePosixPath(name)
            if p.is_absolute() or '..' in p.parts or ':' in name or name.casefold() in seen or stat.S_ISLNK(i.external_attr>>16) or i.flag_bits&1:raise ValueError('unsafe/ambiguous ZIP directory')
            seen.add(name.casefold())
        info=z.getinfo(MEMBER)
        if info.file_size>CAP:raise ValueError('WellData expanded cap')
        with z.open(info) as f:raw=f.readline(LINE_CAP+1)
        if len(raw)>LINE_CAP or tuple(raw.decode('cp1252').rstrip('\r\n').split('|'))!=HEADER:raise ValueError('WellData header changed')
    return dict(schemaVersion=1,kind='rainfolk-texas-sdr-bulk-stream',sourceUrl=SOURCE,
        archiveSha256=meta['sha256'],archiveBytes=size,member=MEMBER,memberBytes=info.file_size,
        headerSha256=hashlib.sha256(raw).hexdigest(),retrievedAt=meta['finishedAt'],lastModified=meta['responseHeaders'].get('Last-Modified'),fields=FIELDS)

def batches(archive,header):
    csv.field_size_limit(65536)
    rows=[];used=0;count=0;size=0;sha=hashlib.sha256();indices=[HEADER.index(f) for f in FIELDS]
    with zipfile.ZipFile(archive) as z, z.open(MEMBER) as f:
        raw=f.readline(LINE_CAP+1);sha.update(raw);size+=len(raw)
        if hashlib.sha256(raw).hexdigest()!=header['headerSha256']:raise ValueError('header changed during read')
        while raw:=f.readline(LINE_CAP+1):
            size+=len(raw);count+=1
            if len(raw)>LINE_CAP or size>CAP or count>MAX_ROWS:raise ValueError('source line/byte/row cap')
            sha.update(raw)
            key=raw.split(b'|',1)[0]
            native=key.decode('ascii') if key.isdigit() and 0<len(key)<=15 else None
            row=dict(row=count+1,sha256=hashlib.sha256(raw).hexdigest(),id=native)
            try:
                values=next(csv.reader([raw.decode('cp1252').removesuffix('\n').removesuffix('\r')],delimiter='|',quoting=csv.QUOTE_NONE,strict=True))
                if len(values)!=len(HEADER):row['error']='column-count'
                else:
                    selected={k:values[i] for k,i in zip(FIELDS,indices)}
                    if any(len(v.encode('utf-8'))>1024 for v in selected.values()):row['error']='field-size'
                    else:row['values']=selected
            except UnicodeDecodeError:row['error']='invalid-cp1252'
            except csv.Error:row['error']='field-size'
            item_size=len(encoded(row))+1
            if rows and (len(rows)>=200 or used+item_size>LINE_CAP-256):
                yield dict(kind='batch',rows=rows);rows=[];used=0
            rows.append(row);used+=item_size
        if rows:yield dict(kind='batch',rows=rows)
    if size!=header['memberBytes']:raise ValueError('source member size mismatch')
    yield dict(kind='end',rows=count,memberBytes=size,memberSha256=sha.hexdigest())

def run(archive,receipt,output):
    header=prepare(archive,receipt)
    output=output.resolve();ignored=(ROOT/'.temp-gev').resolve()
    if not output.is_relative_to(ignored) or output.exists():raise ValueError('fresh ignored output directory required')
    if shutil.disk_usage(ROOT).free<8*1024*1024*1024:raise ValueError('insufficient output disk reserve')
    binary=ROOT/'crates/us-groundwater/target/debug'/('well-jobs.exe' if __import__('os').name=='nt' else 'well-jobs')
    child=subprocess.Popen([str(binary),'import-sdr-bulk',str(output)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    timer=threading.Timer(1800,child.kill);timer.start();started=time.monotonic();count=0
    try:
        child.stdin.write(encoded(header))
        for message in batches(archive,header):
            raw=encoded(message)
            if len(raw)>LINE_CAP:raise ValueError('projected batch byte cap')
            child.stdin.write(raw)
            if message['kind']=='batch':count+=len(message['rows'])
            if output.is_dir() and (count%5000<200 or message['kind']=='end'):
                (output/'reader-progress.json').write_bytes(encoded(dict(sourceRows=count,elapsedSeconds=time.monotonic()-started,networkRequests=0)))
        child.stdin.close();child.stdin=None
        stdout,stderr=child.communicate()
        if child.returncode:raise ValueError('Rust bulk admission failed: '+stderr.decode('utf-8',errors='replace')[:500])
        result=json.loads(stdout)
        if result['accounting']['inputRows']!=count:raise ValueError('reader/Rust row accounting mismatch')
        result.update(networkRequests=0,elapsedSeconds=time.monotonic()-started)
        (output/'processing-result.json').write_bytes(encoded(result))
        return result
    except Exception:
        if child.poll() is None:child.kill()
        child.wait()
        raise
    finally:
        timer.cancel()
        for pipe in (child.stdin,child.stdout,child.stderr):
            if pipe is not None:
                try:pipe.close()
                except OSError:pass  # Preserve the original rejection, not a stopped pipe's flush error.

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--archive',type=Path,required=True);p.add_argument('--source-metadata',type=Path,required=True)
    p.add_argument('--output',type=Path);p.add_argument('--run',action='store_true')
    args=p.parse_args()
    try:
        if not args.run:print(json.dumps(dict(dryRun=True,source=prepare(args.archive,args.source_metadata),maxRows=MAX_ROWS,maxMemberBytes=CAP,maxOutputBytes=4*1024*1024*1024,networkRequests=0)));return
        if args.output is None:p.error('--output required with --run')
        print(json.dumps(run(args.archive,args.source_metadata,args.output)))
    except (ValueError,OSError,zipfile.BadZipFile):
        print(json.dumps(dict(status='stopped',reason='bulk input, budget or validation failure; record values omitted',networkRequests=0)))
        raise SystemExit(1) from None

if __name__=='__main__':main()
