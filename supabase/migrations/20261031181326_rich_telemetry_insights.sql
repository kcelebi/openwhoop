create table public.whoop_telemetry (
 owner_id uuid not null references auth.users(id) on delete cascade,
 id text not null, kind text not null check (kind in ('raw_ble','reading')),
 recorded_at_ms bigint, received_at_ms bigint not null, payload jsonb not null,
 primary key(owner_id,id)
);
create index whoop_telemetry_time on public.whoop_telemetry(owner_id,kind,recorded_at_ms desc);
alter table public.whoop_telemetry enable row level security;
revoke all on public.whoop_telemetry from anon,authenticated;
grant select on public.whoop_telemetry to authenticated;
create policy whoop_telemetry_owner_read on public.whoop_telemetry for select to authenticated using ((select auth.uid())=owner_id);

create table public.whoop_insights (
 owner_id uuid not null references auth.users(id) on delete cascade,
 period_key text not null, period_start_ms bigint not null, period_end_ms bigint not null,
 timezone text not null, result jsonb not null, calculated_at timestamptz not null default now(),
 primary key(owner_id,period_key)
);
alter table public.whoop_insights enable row level security;
revoke all on public.whoop_insights from anon,authenticated;
grant select on public.whoop_insights to authenticated;
create policy whoop_insights_owner_read on public.whoop_insights for select to authenticated using ((select auth.uid())=owner_id);

create function public.whoop_pulse_history(start_ms bigint,end_ms bigint,bucket_ms bigint default 60000)
returns jsonb language plpgsql stable security invoker set search_path='' as $$
declare answer jsonb; step_ms bigint;
begin
 if auth.uid() is null then raise exception 'Sign in required'; end if;
 if end_ms <= start_ms or end_ms-start_ms > 2678400000 then raise exception 'Choose a range of up to 31 days'; end if;
 step_ms := greatest(1000,coalesce(bucket_ms,60000),ceil((end_ms-start_ms)::numeric/1500)::bigint);
 with selected as materialized (
  select recorded_at_ms,bpm from public.whoop_hr_samples
  where owner_id=(select auth.uid()) and recorded_at_ms >= start_ms and recorded_at_ms < end_ms
 ), buckets as (
  select (recorded_at_ms/step_ms)*step_ms as t,round(avg(bpm)::numeric,1) as bpm,min(bpm) as low,max(bpm) as high,
   count(*) as samples,count(distinct recorded_at_ms/1000) as observed_seconds
  from selected group by 1
 ), summary as (
  select count(*) as samples,round(avg(bpm)::numeric,1) as average,min(bpm) as low,max(bpm) as high,
   count(distinct recorded_at_ms/1000) as observed_seconds from selected
 )
 select jsonb_build_object('step_ms',step_ms,'points',coalesce((select jsonb_agg(to_jsonb(b) order by b.t) from buckets b),'[]'::jsonb),
  'summary',(select to_jsonb(s) || jsonb_build_object('coverage',least(1.0,s.observed_seconds::numeric/((end_ms-start_ms)::numeric/1000))) from summary s),
  'available_from_ms',(select recorded_at_ms from public.whoop_hr_samples where owner_id=(select auth.uid()) order by recorded_at_ms limit 1),
  'available_to_ms',(select recorded_at_ms from public.whoop_hr_samples where owner_id=(select auth.uid()) order by recorded_at_ms desc limit 1)) into answer;
 return answer;
end $$;
revoke all on function public.whoop_pulse_history(bigint,bigint,bigint) from public,anon;
grant execute on function public.whoop_pulse_history(bigint,bigint,bigint) to authenticated;
