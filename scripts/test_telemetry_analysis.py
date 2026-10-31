import importlib.util
from pathlib import Path
import sqlite3
import tempfile
import unittest
from datetime import date,datetime,timezone
from zoneinfo import ZoneInfo
spec=importlib.util.spec_from_file_location('telemetry_analysis',Path(__file__).with_name('analyze-telemetry.py'))
a=importlib.util.module_from_spec(spec);spec.loader.exec_module(a)

class AnalysisTests(unittest.TestCase):
 def records(self,start,minutes,bpm=60,contact=1,rr=None):
  return [(start+i*5000,{'bpm':bpm,'rr_ms':rr or [],'sensor_data':{'skin_contact':contact,'accel_gravity':[0,0,1]}}) for i in range(minutes*12)]
 def test_sustained_angle_sleep_has_fractional_duration_and_provisional_end(self):
  start=1_700_000_000_000//5000*5000
  epochs=a.epochs_from_readings(self.records(start,90))
  result=a.sleep_estimate(epochs,start,start+8*3600000,start+90*60000)
  self.assertEqual(result['duration_minutes'],90)
  self.assertTrue(result['episodes'][0]['provisional'])
 def test_missing_motion_gap_is_never_counted_as_sleep(self):
  start=1_700_000_000_000//5000*5000
  epochs=a.epochs_from_readings(self.records(start,40)+self.records(start+60*60000,40))
  result=a.sleep_estimate(epochs,start,start+100*60000,start+101*60000)
  self.assertEqual(len(result['episodes']),2)
  self.assertEqual(result['duration_minutes'],80)
  self.assertEqual(result['coverage'],0.8)
 def test_off_wrist_and_bpm_only_do_not_produce_sleep(self):
  start=1_700_000_000_000//5000*5000
  for records in [self.records(start,90,contact=0),[(t,{'bpm':60}) for t,_ in self.records(start,90)]]:
   result=a.sleep_estimate(a.epochs_from_readings(records),start,start+90*60000,start+90*60000)
   self.assertEqual(result['episodes'],[])
   self.assertEqual(result['motion_seconds'],0)
 def test_activity_requires_continuity_and_hrv_requires_recorded_rr(self):
  start=int(datetime(2026,10,31,tzinfo=timezone.utc).timestamp()*1000)
  epochs=a.epochs_from_readings(self.records(start,12,bpm=140))
  result=a.analyze_day(epochs,date(2026,10,31),ZoneInfo('UTC'),start+86400000)
  self.assertEqual(result['activity']['duration_minutes'],12)
  self.assertIsNone(result['pulse']['rmssd_ms'])
  epochs=a.epochs_from_readings(self.records(start,6,bpm=140)+self.records(start+10*60000,6,bpm=140))
  result=a.analyze_day(epochs,date(2026,10,31),ZoneInfo('UTC'),start+86400000)
  self.assertEqual(result['activity']['episodes'],[])
 def test_rr_duplicates_do_not_double_count_and_rmssd_is_real(self):
  start=int(datetime(2026,10,31,tzinfo=timezone.utc).timestamp()*1000)
  records=self.records(start,10,rr=[800,820])
  result=a.analyze_day(a.epochs_from_readings(records+records),date(2026,10,31),ZoneInfo('UTC'),start+86400000)
  self.assertEqual(result['pulse']['rr_intervals'],240)
  self.assertEqual(result['pulse']['rmssd_ms'],20)
 def test_history_and_live_rr_do_not_count_the_same_epoch_twice(self):
  rows=[(10000,{'bpm':60,'source':'device_history','rr_ms':[800,820]}),
        (10200,{'bpm':60,'source':'gatt_live','rr_ms':[800,820]})]
  epochs=a.epochs_from_readings(rows)
  self.assertEqual(epochs[0]['rr'],[800,820])
 def test_spring_and_fall_days_use_local_calendar_boundaries(self):
  zone=ZoneInfo('America/New_York')
  day=date(2026,11,1)
  start=int(datetime(2026,11,1,tzinfo=zone).timestamp()*1000)
  end=int(datetime(2026,11,2,tzinfo=zone).timestamp()*1000)
  self.assertEqual((end-start)/3600000,25)
  result=a.analyze_day(a.epochs_from_readings(self.records(start,60)),day,zone,end)
  self.assertAlmostEqual(result['pulse']['coverage'],1/25)
 def test_backfill_preserves_rich_fields_and_is_idempotent(self):
  with tempfile.TemporaryDirectory() as directory:
   source=Path(directory)/'old.sqlite';target=Path(directory)/'new.sqlite'
   with sqlite3.connect(source) as c:
    c.execute('create table heart_rate(id integer,bpm integer,time text,rr_intervals text,sensor_data text,imu_data text)')
    c.execute('insert into heart_rate values(1,60,?,?,?,?)',('2026-10-31 03:00:00','800,820','{"accel_gravity":[0,0,1],"skin_contact":1}','[]'))
   with sqlite3.connect(target) as c:
    c.execute('create table telemetry_outbox(id text primary key,kind text,recorded_at_ms integer,received_at_ms integer,payload text,synced boolean)')
    state={};a.backfill(c,[source],ZoneInfo('America/New_York'),state);a.backfill(c,[source],ZoneInfo('America/New_York'),state)
    self.assertEqual(c.execute('select count(*) from telemetry_outbox').fetchone()[0],1)
    payload=c.execute('select payload from telemetry_outbox').fetchone()[0]
    self.assertIn('accel_gravity',payload);self.assertIn('rr_ms',payload)

if __name__=='__main__':unittest.main()
