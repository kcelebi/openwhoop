#!/usr/bin/env python3
"""Automatic, versioned descriptive insights from preserved UTC telemetry.
Sleep: WHOOP adaptation of van Hees 2015 sustained arm-angle inactivity.
This is an estimate, not a sleep-stage classifier or validated WHOOP score.
https://doi.org/10.1371/journal.pone.0142533
"""
import argparse
from collections import deque
from datetime import datetime, time as day_time, timedelta, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import sqlite3
from statistics import mean, median
import urllib.request
import uuid
from zoneinfo import ZoneInfo

VERSION = 'whoop-angle-5deg-5min-v1'
EPOCH_MS = 5000


def compact(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False)


def backfill(connection, sources, zone, state):
    affected = set()
    for path in sources:
        path = Path(path).resolve()
        if not path.exists():
            continue
        key = hashlib.sha256(str(path).encode()).hexdigest()[:16]
        previous = state.get('legacy', {}).get(key, 0)
        source = sqlite3.connect(f'file:{path}?mode=ro', uri=True, timeout=2)
        source.row_factory = sqlite3.Row
        legacy_rows = source.execute('select * from heart_rate where id > ? order by id limit 5000', (previous,)).fetchall()
        packet_previous = state.get('legacy_packets', {}).get(key, 0)
        try:
            legacy_packets = source.execute('select * from packets where id > ? order by id limit 500', (packet_previous,)).fetchall()
        except sqlite3.Error:
            legacy_packets = []
        source.close()
        highest = previous
        for row in legacy_rows:
            highest = row['id']
            stamp = datetime.fromisoformat(row['time']).replace(tzinfo=zone)
            ms = round(stamp.timestamp() * 1000)
            sensor = json.loads(row['sensor_data']) if row['sensor_data'] else None
            imu = json.loads(row['imu_data']) if row['imu_data'] else []
            rr = [int(v) for v in row['rr_intervals'].strip('[]').replace(',', ' ').split() if v]
            # New native captures already have a normalized UTC reading. Never
            # skip richer legacy rows merely because a BPM-only row exists.
            if not sensor and not imu and not rr and connection.execute("select 1 from telemetry_outbox where kind='reading' and recorded_at_ms=? limit 1", (ms,)).fetchone():
                continue
            payload = {'source':'legacy_sqlite', 'bpm':row['bpm'], 'rr_ms':rr, 'imu_data':imu,
                       'sensor_data':sensor, 'legacy_time':row['time'], 'legacy_timezone':str(zone), 'decoder_version':1}
            connection.execute('insert or ignore into telemetry_outbox values(?,?,?,?,?,false)',
                (f'legacy:{key}:{row["id"]}', 'reading', ms, int(datetime.now(timezone.utc).timestamp()*1000), compact(payload)))
            affected.update([stamp.date(), stamp.date()+timedelta(days=1)])
        state.setdefault('legacy', {})[key] = highest
        for row in legacy_packets:
            characteristic = str(uuid.UUID(bytes=bytes(row['uuid']))) if isinstance(row['uuid'], bytes) else str(row['uuid'])
            payload = {'characteristic':characteristic,'bytes':list(bytes(row['bytes'])),
                       'legacy_source':key,'legacy_packet_id':row['id'],'original_received_time_unknown':True,'decoder_version':1}
            connection.execute('insert or ignore into telemetry_outbox values(?,?,?,?,?,false)',
                (f'legacy_packet:{key}:{row["id"]}','raw_ble',None,int(datetime.now(timezone.utc).timestamp()*1000),compact(payload)))
            state.setdefault('legacy_packets', {})[key] = row['id']
    connection.commit()
    return affected


def angle(gravity):
    if not isinstance(gravity, list) or len(gravity) != 3:
        return None
    if not all(isinstance(v, (int,float)) and math.isfinite(v) for v in gravity):
        return None
    x,y,z = gravity
    magnitude = math.sqrt(x*x+y*y+z*z)
    if not 0.5 <= magnitude <= 1.5:
        return None
    return math.degrees(math.atan2(z, math.sqrt(x*x+y*y)))


def epochs_from_readings(readings):
    buckets = {}
    for ms, payload in readings:
        bucket = buckets.setdefault(ms//EPOCH_MS*EPOCH_MS, {'bpm':{}, 'angles':[], 'rr':{}, 'rr_priority':-1, 'seconds':set()})
        sensor = payload.get('sensor_data') or {}
        gatt = payload.get('gatt') or {}
        off_wrist = sensor.get('skin_contact') == 0 or gatt.get('skin_contact') is False
        bpm = payload.get('bpm')
        if not off_wrist and isinstance(bpm,(int,float)) and 20 <= bpm <= 250:
            bucket['bpm'][ms//1000] = bpm
            bucket['seconds'].add(ms//1000)
        if not off_wrist:
            a = angle(sensor.get('accel_gravity'))
            if a is not None:
                bucket['angles'].append(a)
            rr = tuple(v for v in payload.get('rr_ms',[]) if isinstance(v,(int,float)) and 250 <= v <= 2000)
            if rr:
                priority={'device_history':3,'gatt_live':2,'legacy_sqlite':1}.get(payload.get('source'),0)
                if priority>bucket['rr_priority']:
                    bucket['rr'].clear();bucket['rr_priority']=priority
                if priority==bucket['rr_priority']:
                    bucket['rr'][(ms,rr)] = rr
    return [{'t':t, 'bpm':mean(b['bpm'].values()) if b['bpm'] else None,
             'angle':median(b['angles']) if b['angles'] else None, 'motion_samples':len(b['angles']),
             'seconds':len(b['seconds']), 'rr':[v for seq in b['rr'].values() for v in seq]}
            for t,b in sorted(buckets.items())]


def bouts(epochs, predicate, minimum_ms):
    result = []
    start = end = None
    for e in epochs:
        if predicate(e):
            if start is None or e['t'] - end > EPOCH_MS:
                if start is not None and end+EPOCH_MS-start >= minimum_ms:
                    result.append({'start_ms':start,'end_ms':end+EPOCH_MS,'duration_minutes':(end+EPOCH_MS-start)/60000})
                start = e['t']
            end = e['t']
        else:
            if start is not None and end+EPOCH_MS-start >= minimum_ms:
                result.append({'start_ms':start,'end_ms':end+EPOCH_MS,'duration_minutes':(end+EPOCH_MS-start)/60000})
            start = end = None
    if start is not None and end+EPOCH_MS-start >= minimum_ms:
        result.append({'start_ms':start,'end_ms':end+EPOCH_MS,'duration_minutes':(end+EPOCH_MS-start)/60000})
    return result


def sleep_estimate(epochs, start_ms, end_ms, now_ms):
    data = [e for e in epochs if start_ms <= e['t'] < min(end_ms, now_ms)]
    # Five-minute rolling angle window; missing/off-wrist epochs break the
    # window. Mark the full qualifying window, never fill recording gaps.
    window = deque()
    sleeping = set()
    last = None
    for e in data:
        if e['angle'] is None or (last is not None and e['t']-last > EPOCH_MS):
            window.clear()
        last = e['t']
        if e['angle'] is None:
            continue
        window.append(e)
        while window and e['t']-window[0]['t'] >= 300000:
            window.popleft()
        if len(window) >= 60 and max(x['angle'] for x in window)-min(x['angle'] for x in window) <= 5:
            sleeping.update(x['t'] for x in window)
    episodes = bouts(data, lambda e:e['t'] in sleeping, 30*60000)
    for episode in episodes:
        episode['provisional'] = episode['end_ms'] >= min(end_ms,now_ms)-60000
    observed = sum(e['angle'] is not None for e in data)*5
    possible = max(1,(min(end_ms,now_ms)-start_ms)/1000)
    return {'method':VERSION, 'status':'estimated' if episodes else 'collecting' if observed<3600 else 'no_sustained_sleep',
            'episodes':episodes, 'duration_minutes':sum(e['duration_minutes'] for e in episodes),
            'motion_seconds':observed, 'coverage':min(1,observed/possible),
            'window_start_ms':start_ms,'window_end_ms':end_ms,
            'note':'Estimated sustained inactivity from wrist angle. Quiet wake can resemble sleep; gaps and off-wrist data are excluded.'}


def analyze_day(epochs, day, zone, now_ms):
    def stamp(d,h=0):return int(datetime.combine(d,day_time(h),zone).timestamp()*1000)
    start,end = stamp(day),stamp(day+timedelta(days=1))
    pulse = [e for e in epochs if start<=e['t']<min(end,now_ms) and e['bpm'] is not None]
    activity = bouts(pulse,lambda e:e['bpm']>=120,10*60000)
    rr_pairs = []
    rr_count = 0
    last_rr = None
    last_time = None
    for e in pulse:
        if last_time is not None and e['t']-last_time>10000:
            last_rr = None
        for rr in e['rr']:
            if last_rr is not None:rr_pairs.append((rr-last_rr)**2)
            last_rr=rr
            rr_count+=1
        if e['rr']:last_time=e['t']
    rmssd = round(math.sqrt(mean(rr_pairs)),1) if len(rr_pairs)>=119 else None
    observed = len(pulse)*5
    return {'method_version':1,'pulse':{'observed_seconds':observed,'coverage':min(1,observed/max(1,(min(end,now_ms)-start)/1000)),
            'average_bpm':round(mean(e['bpm'] for e in pulse),1) if pulse else None,
            'rr_intervals':rr_count,'rmssd_ms':rmssd},
            'sleep':sleep_estimate(epochs,stamp(day-timedelta(days=1),12),stamp(day,12),now_ms),
            'activity':{'method':'elevated-pulse-120bpm-10min-v1','threshold_bpm':120,'episodes':activity,
                        'duration_minutes':sum(e['duration_minutes'] for e in activity),
                        'note':'Sustained elevated pulse, not a confirmed workout classification.'}}


def process(database, state_path, legacy_sources, zone, now=None):
    now = now or datetime.now(timezone.utc)
    now_ms = int(now.timestamp()*1000)
    state_file=Path(state_path)
    try:state=json.loads(state_file.read_text())
    except (OSError,ValueError):state={}
    with sqlite3.connect(database,timeout=10) as connection:
        affected=backfill(connection,legacy_sources,zone,state)
        # Late-arriving buffered history automatically recomputes older dates.
        for (ms,) in connection.execute("select distinct (recorded_at_ms/86400000)*86400000 from telemetry_outbox where kind='reading' and received_at_ms>?",(state.get('last_run_ms',0),)):
            d=datetime.fromtimestamp(ms/1000,zone).date()
            affected.update([d,d+timedelta(days=1),d+timedelta(days=2)])
        today=now.astimezone(zone).date()
        affected.update([today,today+timedelta(days=1),today-timedelta(days=1)])
        results=[]
        for day in sorted(affected):
            if day>today+timedelta(days=1):continue
            start=int(datetime.combine(day-timedelta(days=1),day_time(12),zone).timestamp()*1000)
            end=int(datetime.combine(day+timedelta(days=1),day_time(0),zone).timestamp()*1000)
            if start>now_ms:continue
            records=((ms,json.loads(payload)) for ms,payload in connection.execute("select recorded_at_ms,payload from telemetry_outbox where kind='reading' and recorded_at_ms>=? and recorded_at_ms<? order by recorded_at_ms",(start,end)))
            epochs=epochs_from_readings(records)
            if not epochs:continue
            result=analyze_day(epochs,day,zone,now_ms)
            results.append({'period_key':str(day),'period_start_ms':int(datetime.combine(day,day_time(0),zone).timestamp()*1000),
                'period_end_ms':end,'timezone':str(zone),'result':result,'calculated_at':now.isoformat()})
    return results,state,now_ms


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--database',required=True)
    parser.add_argument('--state',required=True)
    parser.add_argument('--legacy',action='append',default=[])
    parser.add_argument('--timezone',default=os.getenv('OPENWHOOP_TIMEZONE','America/New_York'))
    args=parser.parse_args()
    results,state,now_ms=process(args.database,args.state,args.legacy,ZoneInfo(args.timezone))
    # Keep the computed report durable locally even if the network is down.
    result_file=Path(args.state).with_name('insights.json')
    temporary=result_file.with_suffix('.tmp');temporary.write_text(compact(results));temporary.replace(result_file)
    if results:
        owner=os.environ['OPENWHOOP_SUPABASE_OWNER_ID']
        payload=[dict(row,owner_id=owner) for row in results]
        key=os.environ['OPENWHOOP_SUPABASE_SECRET_KEY']
        request=urllib.request.Request(os.environ['OPENWHOOP_SUPABASE_URL'].rstrip('/')+'/rest/v1/whoop_insights?on_conflict=owner_id%2Cperiod_key',
            data=compact(payload).encode(),method='POST',headers={'apikey':key,'Authorization':'Bearer '+key,
                'Content-Type':'application/json','Prefer':'resolution=merge-duplicates,return=minimal'})
        with urllib.request.urlopen(request,timeout=20) as response:response.read()
    # Only advance the analysis watermark after cloud acknowledgement.
    state['last_run_ms']=now_ms
    path=Path(args.state);temporary=path.with_suffix('.tmp');temporary.write_text(compact(state));temporary.replace(path)
    print(f'Updated {len(results)} daily insight reports',flush=True)


if __name__=='__main__':main()
