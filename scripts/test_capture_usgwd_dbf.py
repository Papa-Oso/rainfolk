import importlib.util,unittest,tempfile,struct,hashlib,json,zipfile,csv
from pathlib import Path
spec=importlib.util.spec_from_file_location('dbf',Path(__file__).with_name('usgwd-dbf.py'));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class DbfTests(unittest.TestCase):
 def fixture(self):
  fields=[]
  for name,kind in zip(m.ALIASES,m.TYPES):
   d=bytearray(32);d[:len(name)]=name.encode();d[11]=ord(kind);d[16]=80 if kind=='C' else 24;fields.append(d)
  row=['']*29;row[0]='SYNTH_1';row[2]='-80.125';row[3]='34.5';row[4]='Invented caf\u00e9';row[5]='South Carolina';row[6]='00123';row[9]='000123456789';row[12:15]=['0','0','0'];row[28]='0';row[15]='-2.00'
  record=b' '+b''.join(v.encode('latin-1').ljust(d[16],b' ')for v,d in zip(row,fields));h=bytearray(32);h[0]=3;struct.pack_into('<IHH',h,4,1,961,len(record));return bytes(h)+b''.join(fields)+b'\r'+record+b'\x1a'
 def test_preserves_identifiers_negative_measurements_and_encoding(self):
  row=list(m.rows(self.fixture()))[0];self.assertEqual(row[4],'Invented caf\u00e9');self.assertEqual(row[6],'00123');self.assertEqual(row[9],'000123456789');self.assertEqual(row[15],'-2.00')
 def test_deleted_truncated_and_wrong_schema_rejected(self):
  b=self.fixture()
  for altered in (b[:961]+b'*'+b[962:],b[:-2],b[:32]+b'Wrong'+b[37:]):
   with self.assertRaises(ValueError):list(m.rows(altered))
 def test_conversion_provenance_and_checksum(self):
  with tempfile.TemporaryDirectory(dir=Path(__file__).resolve().parents[1]/'.temp-gev')as t:
   p=Path(t);z=p/'source.zip';template=p/'template.json';out=p/'derived.csv';receipt=p/'receipt.json'
   template.write_text(json.dumps(dict(state='South Carolina',resourceId='8b02895f02c14dd1a749bcc5584a5c55')))
   with zipfile.ZipFile(z,'w')as f:
    f.writestr('USGWD_South Carolina.dbf',self.fixture());f.writestr('USGWD_South Carolina.prj',m.PRJ);f.writestr('USGWD_South Carolina.cpg','ISO-8859-1')
   with self.assertRaisesRegex(ValueError,'checksum'):m.convert(z,template,out,receipt,'0'*32)
   self.assertFalse(out.exists());v=m.convert(z,template,out,receipt,hashlib.md5(z.read_bytes()).hexdigest());self.assertEqual(v['recordCount'],1);s=json.loads(receipt.read_bytes());self.assertEqual(s['inputSha256'],hashlib.sha256(out.read_bytes()).hexdigest());self.assertIn('Geospatial',s['sourceUrl']);self.assertEqual(s['schemaVersion'],2)
   with out.open(encoding='utf-8',newline='')as f:r=list(csv.reader(f))
   self.assertEqual(r[0],m.HEADER);self.assertEqual(r[1][15],'-2.00')
if __name__=='__main__':unittest.main()
