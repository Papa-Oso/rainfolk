"""Synthetic snapshots through the real Rust importer/diff/export commands."""
import importlib.util
import csv
import subprocess
import json
from pathlib import Path
from unittest import mock
import unittest
import test_capture_usgwd as fixtures

spec=importlib.util.spec_from_file_location('local_snapshot_refresh',Path(__file__).with_name('refresh-wells.py'))
refresh=importlib.util.module_from_spec(spec);spec.loader.exec_module(refresh)


class RefreshTests(unittest.TestCase):
    setUp=fixtures.UsgwdTests.setUp
    tearDown=fixtures.UsgwdTests.tearDown
    source=fixtures.UsgwdTests.source
    run_import=fixtures.UsgwdTests.run_import

    def config(self):
        path=self.root/'refresh.json'
        path.write_text(json.dumps(dict(schemaVersion=1,profile='usgwd-local-snapshot',runSeconds=7200,
            sources=[dict(key='new-hampshire',csv='source.csv',receipt='source.json')],schedule=dict(enabled=False,cadence='daily'))))
        return path

    def run_refresh(self,**kwargs):
        with mock.patch.object(refresh.shutil,'disk_usage',return_value=type('Disk',(),{'free':1024**4})()):
            return refresh.run(self.config(),self.root/'refresh',**kwargs)

    def state(self):
        return refresh.checkpoint(self.root/'refresh/refresh-state.json',None)

    def report(self):
        entry=self.state()['sources']['new-hampshire']
        completion=refresh.load(refresh.pinned(self.root/'refresh',entry['completion']))
        return refresh.load(refresh.pinned(self.root/'refresh',completion['diff']))

    def job(self):
        paths=list((self.root/'refresh/runs/new-hampshire').glob('*/job.json'))
        return max(paths,key=lambda p:p.stat().st_mtime_ns)

    def test_first_snapshot_and_unchanged_skip_all_native_processing(self):
        self.source();first=self.run_refresh()
        self.assertEqual(first['status'],'changed-state-candidate-ready')
        self.assertEqual(self.report()['counts']['added'],2)
        index=self.root/'refresh'/first['candidate']['index']['path'];raw=index.read_bytes()
        with mock.patch.object(refresh,'invoke',side_effect=AssertionError('unchanged input must not invoke Rust')):
            second=self.run_refresh()
        self.assertEqual(second['status'],'unchanged');self.assertFalse(second['remoteSourceChecked'])
        self.assertEqual(raw,index.read_bytes());self.assertEqual(len(list((self.root/'refresh/runs/new-hampshire').iterdir())),1)

    def test_added_changed_missing_keep_history_aliases_and_no_deletion(self):
        self.source();first=self.run_refresh();old_index=self.root/'refresh'/first['candidate']['index']['path'];old_raw=old_index.read_bytes()
        changed=dict(self.rows[0],**{'Well Depth (Feet)':'250'})
        added=dict(self.rows[0],**{'Well ID':'SYN_3'})
        self.source([changed,added]);second=self.run_refresh();report=self.report()
        self.assertEqual(report['counts'],dict(added=1,changed=1,missing=1,quarantined=0,unchanged=0))
        self.assertTrue(report['requiresReview']);self.assertEqual(old_index.read_bytes(),old_raw)
        events=[]
        completion=refresh.load(refresh.pinned(self.root/'refresh',second['candidate']['completion']))
        report_path=refresh.pinned(self.root/'refresh',completion['diff'])
        for ref in report['deltas']:
            events.extend(refresh.load(refresh.pinned(report_path.parent,ref))['events'])
        self.assertTrue(all(e['delete'] is False and e['physicalWellStatus'] is None for e in events))
        missing=next(e for e in events if e['kind']=='missing')
        self.assertIsNone(missing['after']);self.assertEqual(missing['before']['rawStateRecordId'],'invented-shared-state-id')
        self.assertEqual(completion['activation'],'manual-only');self.assertEqual(completion['previous']['index'],first['candidate']['index'])

    def test_semantic_unchanged_rows_ignore_snapshot_provenance_changes(self):
        self.source();self.run_refresh()
        self.source(self.rows+[dict(self.rows[0],**{'Well ID':'SYN_3'})]);self.run_refresh()
        self.assertEqual(self.report()['counts']['unchanged'],2);self.assertEqual(self.report()['counts']['added'],1)

    def test_changed_receipt_contract_holds_even_if_input_bytes_are_unchanged(self):
        self.source();self.run_refresh();old=self.state()['sources']['new-hampshire']['index']
        path=self.root/'source.json';receipt=json.loads(path.read_bytes());receipt['attribution']='Changed invented attribution requiring review';path.write_text(json.dumps(receipt))
        with self.assertRaisesRegex(ValueError,'receipt contract changed'):
            self.run_refresh()
        self.assertEqual(self.state()['sources']['new-hampshire']['index'],old)

    def test_only_changed_state_exports_and_other_state_reference_is_reused(self):
        self.source();config=self.config()
        def nevada(depth):
            with (self.root/'nevada.csv').open('w',newline='',encoding='utf-8') as f:
                w=csv.DictWriter(f,self.header);w.writeheader();w.writerow(dict(self.rows[0],State='Nevada',**{'Well ID':'SYN_NV_1','Well Depth (Feet)':depth}))
            receipt=json.loads((self.root/'source.json').read_bytes());b=(self.root/'nevada.csv').read_bytes()
            receipt.update(state='Nevada',sourceUrl=f'https://www.hydroshare.org/resource/{fixtures.RESOURCE}/data/contents/USGWD-Tabular/USGWD_Nevada.csv',inputSha256=refresh.sha(b),inputBytes=len(b))
            (self.root/'nevada.json').write_text(json.dumps(receipt))
        nevada('100');c=json.loads(config.read_bytes());c['sources'].append(dict(key='nevada',csv='nevada.csv',receipt='nevada.json'));config.write_text(json.dumps(c))
        def run():
            with mock.patch.object(refresh.shutil,'disk_usage',return_value=type('Disk',(),{'free':1024**4})()):
                return refresh.run(config,self.root/'refresh')
        nh=run();nv=run();self.assertEqual(nv['key'],'nevada')
        old_nh=self.state()['sources']['new-hampshire']['index'];old_bytes=refresh.pinned(self.root/'refresh',old_nh).read_bytes()
        nevada('101');result=run()
        self.assertEqual(result['key'],'nevada');self.assertEqual(result['reusedSources'],['new-hampshire'])
        self.assertEqual(self.state()['sources']['new-hampshire']['index'],old_nh)
        self.assertEqual(refresh.pinned(self.root/'refresh',old_nh).read_bytes(),old_bytes)

    def test_cross_release_generated_ids_never_upsert_by_state_alias(self):
        self.source();self.run_refresh();self.source(release='c'*64);self.run_refresh()
        report=self.report();self.assertEqual(report['counts']['added'],2);self.assertEqual(report['counts']['missing'],2)
        self.assertTrue(report['requiresReview']);self.assertEqual(report['counts']['changed'],0)

    def test_conflicting_id_is_quarantine_transition_not_deletion(self):
        self.source();self.run_refresh()
        self.source(self.rows+[dict(self.rows[0],County='Invented conflict')]);self.run_refresh()
        report=self.report();self.assertEqual(report['counts']['quarantined'],1)
        self.assertEqual(report['counts']['missing'],0);self.assertEqual(report['after']['accounting']['quarantinedRows'],2)

    def test_pause_resume_and_completed_noop_preserve_reservations(self):
        self.source();a=self.run_refresh(steps=1);self.assertEqual(a['stage'],'diff')
        path=self.job();deadline=refresh.checkpoint(path,None)['deadlineAt']
        b=self.run_refresh(steps=1);self.assertEqual(b['stage'],'export')
        c=self.run_refresh(steps=1);self.assertEqual(c['status'],'changed-state-candidate-ready')
        job=refresh.checkpoint(path,None);self.assertEqual(job['attempts'],3);self.assertEqual(job['deadlineAt'],deadline)
        self.assertEqual(self.run_refresh()['status'],'unchanged')

    def test_missing_receipt_after_committed_source_copy_is_repaired_from_pin(self):
        self.source();self.run_refresh(steps=1);path=self.job()
        receipt=path.parent/'source.json';before=receipt.read_bytes();receipt.unlink()
        result=self.run_refresh()
        self.assertEqual(result['status'],'changed-state-candidate-ready');self.assertEqual(receipt.read_bytes(),before)

    def test_failed_export_holds_last_candidate_and_explicit_resume_recovers(self):
        self.source();first=self.run_refresh();old=self.state()['sources']['new-hampshire']['index']
        self.source(self.rows+[dict(self.rows[0],**{'Well ID':'SYN_3'})]);self.run_refresh(steps=2)
        invoke=refresh.invoke
        def fail_export(args,job,path,seconds):
            if '--usgwd' in args:
                args=[str(fixtures.BINARY),'diff-usgwd','-',str(refresh.pinned(self.root/'refresh',job['manifest'])),str(path.parent/'failed-diff'),'0']
            return invoke(args,job,path,seconds)
        with mock.patch.object(refresh,'invoke',side_effect=fail_export):
            with self.assertRaisesRegex(ValueError,'child failed'):
                self.run_refresh()
        self.assertEqual(self.state()['sources']['new-hampshire']['index'],old)
        jobpath=self.job();failed=refresh.checkpoint(jobpath,None)
        with self.assertRaisesRegex(ValueError,'explicit --resume'):
            self.run_refresh()
        result=self.run_refresh(resume=True)
        self.assertEqual(result['status'],'changed-state-candidate-ready')
        recovered=refresh.checkpoint(jobpath,None)
        self.assertEqual(recovered['deadlineAt'],failed['deadlineAt']);self.assertGreater(recovered['attempts'],failed['attempts'])
        self.assertTrue((self.root/'refresh'/first['candidate']['index']['path']).exists())
        attempts=[refresh.load(p) for p in jobpath.parent.glob('attempt-*.json')]
        self.assertEqual(len(attempts),recovered['attempts'])
        self.assertTrue(any(a['exitCode']!=0 for a in attempts))
        self.assertTrue(all((jobpath.parent/a['stderr']['path']).exists() for a in attempts))

    def test_uncertain_child_refuses_automatic_restart_and_expiry_never_renews(self):
        self.source();self.run_refresh(steps=1);path=self.job();job=refresh.checkpoint(path,None)
        job['running']=True;refresh.save(path,job)
        with self.assertRaisesRegex(ValueError,'uncertain child'):
            self.run_refresh()
        self.run_refresh(steps=1,ack_stopped=True)
        job=refresh.checkpoint(path,None);job['deadlineAt']=0;refresh.save(path,job)
        with self.assertRaisesRegex(ValueError,'deadline expired'):
            self.run_refresh(resume=True)
        self.assertEqual(refresh.checkpoint(path,None)['deadlineAt'],0)

    def test_receipt_checkpoint_and_last_export_tamper_fail_closed(self):
        self.source();self.run_refresh();entry=self.state()['sources']['new-hampshire']
        index=self.root/'refresh'/entry['index']['path'];index.write_bytes(b'{}')
        with self.assertRaisesRegex(ValueError,'pinned artifact integrity'):
            self.run_refresh()
        e=json.loads((self.root/'refresh/refresh-state.json').read_bytes());e['payload']['sources']={}
        (self.root/'refresh/refresh-state.json').write_text(json.dumps(e))
        with self.assertRaisesRegex(ValueError,'checkpoint integrity'):
            self.run_refresh()

    def test_inbox_hash_change_and_unowned_output_refused(self):
        self.source();self.root.joinpath('source.csv').write_bytes(b'invented mismatched data')
        with self.assertRaisesRegex(ValueError,'receipt/source fingerprint'):
            self.run_refresh()
        other=self.root/'unowned';other.mkdir();(other/'keep.txt').write_text('untouched')
        with self.assertRaisesRegex(ValueError,'fresh directory'):
            refresh.run(self.config(),other)
        self.assertEqual((other/'keep.txt').read_text(),'untouched')

    def test_concurrent_owner_and_duplicate_config_keys_refuse(self):
        self.source();self.run_refresh()
        with refresh.writer(self.root/'refresh/runner.lock'):
            with self.assertRaises((OSError,BlockingIOError)):
                self.run_refresh()
        path=self.config();path.write_text('{"schemaVersion":1,"schemaVersion":1}')
        with self.assertRaisesRegex(ValueError,'duplicate JSON member'):
            refresh.run(path,self.root/'refresh')

    def test_native_diff_determinism_and_partition_tamper(self):
        paths=self.source();self.run_import(paths,'old',allowance=None)
        paths=self.source(self.rows+[dict(self.rows[0],**{'Well ID':'SYN_3'})]);self.run_import(paths,'new',allowance=None)
        args=[str(fixtures.BINARY),'diff-usgwd',str(self.root/'old/usgwd-manifest.json'),str(self.root/'new/usgwd-manifest.json'),str(self.root/'diff'),'900']
        first=subprocess.run(args,capture_output=True,check=True);second=subprocess.run(args,capture_output=True,check=True)
        self.assertEqual(first.stdout,second.stdout)
        manifest=refresh.load(self.root/'new/usgwd-manifest.json')['payload']
        part=self.root/'new'/manifest['partitions'][0]['path'];part.write_bytes(part.read_bytes()+b' ')
        args[-2]=str(self.root/'tampered-diff')
        result=subprocess.run(args,capture_output=True)
        self.assertNotEqual(result.returncode,0);self.assertFalse((self.root/'tampered-diff').exists())

    def test_plan_is_inactive_and_policy_toolchain_budget_guards_refuse(self):
        self.source();config=self.config();out=self.root/'plan-only'
        result=subprocess.run([__import__('sys').executable,str(Path(refresh.__file__)),str(config),str(out),'--plan'],capture_output=True,check=True)
        plan=json.loads(result.stdout);self.assertFalse(plan['scheduleInstalled']);self.assertEqual(plan['networkRequests'],0)
        self.assertFalse(out.exists())
        self.run_refresh(steps=1);path=self.job();job=refresh.checkpoint(path,None)
        job['toolchain']['jobs']='0'*64;refresh.save(path,job)
        with self.assertRaisesRegex(ValueError,'toolchain changed'):
            self.run_refresh()
        c=json.loads(config.read_bytes());c['schedule']['enabled']=True;config.write_text(json.dumps(c))
        with self.assertRaisesRegex(ValueError,'schedule activation'):
            refresh.run(config,self.root/'refresh')

    def test_parent_watchdog_stops_blocked_local_work(self):
        # Isolated process exercises the real hard stop without terminating tests.
        code="import importlib.util,time; s=importlib.util.spec_from_file_location('r',"+repr(str(Path(refresh.__file__)))+"); m=importlib.util.module_from_spec(s);s.loader.exec_module(m);m.Lifetime(1);time.sleep(10)"
        result=subprocess.run([__import__('sys').executable,'-c',code],capture_output=True,timeout=5)
        self.assertEqual(result.returncode,124)


if __name__=='__main__':unittest.main()
