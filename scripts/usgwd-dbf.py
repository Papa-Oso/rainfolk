"""Offline USGWD DBF attributes -> existing CSV importer; no acquisition.

The published ZIP checksum must match a separately retained publisher manifest.
Character fields are fixed-width DBF values, not a reconstruction of the CSV.
All source members remain in the original ZIP; derived bytes get a new hash.
"""
import argparse,csv,hashlib,io,json,struct,zipfile
from pathlib import Path,PurePosixPath

ALIASES=['Well ID','ID-State','Longitude','Latitude','County','State','FIPS','Aquifer-Sp','Aquifer-Br','HUC12','HUC12-Name','xyVerified','F_County','F_State','F_US','Well Depth','Scr Depth','Len of Scr','Capacity','Litho Data','Sur Ele','Status','Yr Constr','Yr Report','USGS Categ','Irr-State','Irr-USGWD','Quality','F_Dup']
HEADER=['Well ID','Well ID (State)','Longitude','Latitude','County','State','FIPS','Aquifer-Specific','Aquifer-Broad','Subwatershed-HUC12','Subwatershed-Name','Location Verified','Flag County','Flag State','Flag US','Well Depth (Feet)','Screen Depth (Feet)','Length of Screen (Feet)','Well Capacity (GPM)','Lithological Data','Surface Elevation (Feet)','Status','Year Well was Constructed','Year Reported','USGS Water Use Category','Irrigation Subcategory (State)','Irrigation Subcategory (Land Use)','Water Quality (Potable/Non-Potable)','Flag Duplicate']
TYPES=['C','C','N','N','C','C','C','C','C','C','C','C','N','N','N','N','N','N','N','C','N','C','C','C','C','C','C','C','N']
PRJ='GEOGCS["GCS_North_American_1983",DATUM["D_North_American_1983",SPHEROID["GRS_1980",6378137.0,298.257222101]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]]'

def rows(raw):
    if len(raw)<33 or raw[0]!=3:raise ValueError('DBF header/version')
    count,header,size=struct.unpack_from('<IHH',raw,4)
    if not 0<count<=2000000 or header!=32+32*29+1 or len(raw) not in (header+count*size,header+count*size+1):raise ValueError('DBF size/count')
    if raw[header-1]!=13 or (len(raw)>header+count*size and raw[-1]!=26):raise ValueError('DBF terminator')
    fields=[]
    for i,(name,kind) in enumerate(zip(ALIASES,TYPES)):
        d=raw[32+i*32:64+i*32];actual=d[:11].split(b'\0')[0].decode('ascii')
        if actual!=name or chr(d[11])!=kind or not 0<d[16]<=254:raise ValueError('DBF field contract')
        fields.append((kind,d[16]))
    if 1+sum(n for _,n in fields)!=size:raise ValueError('DBF row size')
    for i in range(count):
        record=raw[header+i*size:header+(i+1)*size]
        if record[0]!=32:raise ValueError('DBF deleted/invalid record marker')
        row=[];at=1
        for kind,n in fields:
            b=record[at:at+n];at+=n
            if b'\0' in b:raise ValueError('DBF embedded NUL')
            # Only fixed-width padding is removed. Numeric spelling is retained.
            row.append(b.decode('latin-1').rstrip(' ') if kind=='C' else b.decode('ascii').strip(' '))
        yield row

def convert(archive,template,out,receipt,expected_md5):
    if out.exists() or receipt.exists():raise ValueError('output already exists')
    if archive.stat().st_size>64*1024**2:raise ValueError('archive cap')
    digest=hashlib.md5();sha=hashlib.sha256()
    with archive.open('rb') as f:
        for b in iter(lambda:f.read(1048576),b''):digest.update(b);sha.update(b)
    if digest.hexdigest()!=expected_md5:raise ValueError('publisher ZIP checksum mismatch')
    source=json.loads(template.read_bytes());state=source['state'];stem='USGWD_'+state
    with zipfile.ZipFile(archive) as z:
        infos=z.infolist()
        if len(infos)>8 or sum(i.file_size for i in infos)>128*1024**2 or len({i.filename for i in infos})!=len(infos):raise ValueError('archive member cap/duplicates')
        for i in infos:
            if PurePosixPath(i.filename).name!=i.filename or ':' in i.filename or '\\' in i.filename or (i.external_attr>>16)&0o170000==0o120000:raise ValueError('unsafe archive member')
        if z.read(stem+'.cpg').decode('ascii').strip()!='ISO-8859-1' or z.read(stem+'.prj').decode('ascii').strip()!=PRJ:raise ValueError('source encoding/CRS contract')
        raw=z.read(stem+'.dbf')
    # Validate every row before creating final derived files (bounded <=128 MiB).
    count=sum(1 for _ in rows(raw));temporary=out.with_suffix(out.suffix+'.partial')
    if temporary.exists():raise ValueError('partial already exists')
    with temporary.open('x',encoding='utf-8',newline='') as f:
        w=csv.writer(f,lineterminator='\r\n');w.writerow(HEADER);w.writerows(rows(raw))
    if temporary.stat().st_size>192*1024**2:raise ValueError('derived input cap')
    source.update(schemaVersion=2,inputSha256=hashlib.sha256(temporary.read_bytes()).hexdigest(),inputBytes=temporary.stat().st_size,sourceUrl=f'https://www.hydroshare.org/resource/{source["resourceId"]}/data/contents/USGWD-Geospatial/USGWD_{state.replace(" ","_")}.zip',derivation=dict(policy='usgwd-dbf-attributes-v1',archiveSha256=sha.hexdigest(),archiveMd5=digest.hexdigest(),member=stem+'.dbf',memberSha256=hashlib.sha256(raw).hexdigest(),sourceCrs='EPSG:4269',recordCount=count))
    temporary.rename(out);receipt.write_bytes((json.dumps(source,indent=2)+'\n').encode())
    return source['derivation']

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('archive',type=Path);p.add_argument('template',type=Path);p.add_argument('output_csv',type=Path);p.add_argument('output_receipt',type=Path);p.add_argument('--publisher-md5',required=True);a=p.parse_args()
    print(json.dumps(convert(a.archive,a.template,a.output_csv,a.output_receipt,a.publisher_md5)))
