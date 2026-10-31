-- OpenWhoop's hosted data is separate from the Mac's local SeaORM database.
-- Every browser-accessible row belongs to a Supabase Auth user.
create table public.whoop_hr_samples (
  owner_id uuid not null references auth.users(id) on delete cascade,
  recorded_at_ms bigint not null,
  bpm smallint not null check (bpm between 20 and 250),
  source text not null default 'gatt_live',
  received_at_ms bigint not null,
  primary key (owner_id, recorded_at_ms)
);
create index whoop_hr_samples_recent_idx
  on public.whoop_hr_samples (owner_id, recorded_at_ms desc);

create table public.whoop_alarms (
  id uuid primary key default gen_random_uuid(),
  owner_id uuid not null references auth.users(id) on delete cascade,
  label text not null check (char_length(label) between 1 and 120),
  kind text not null check (kind in ('once', 'cron')),
  cron_expr text,
  timezone text not null,
  one_time_unix bigint,
  next_unix bigint,
  enabled boolean not null default true,
  last_rang_unix bigint,
  last_missed_unix bigint,
  last_unconfirmed_unix bigint,
  last_programmed_unix bigint,
  updated_at timestamptz not null default now(),
  check ((kind = 'once' and one_time_unix is not null and cron_expr is null)
      or (kind = 'cron' and cron_expr is not null and one_time_unix is null))
);
create index whoop_alarms_next_idx
  on public.whoop_alarms (owner_id, enabled, next_unix);

create table public.whoop_commands (
  id uuid primary key default gen_random_uuid(),
  owner_id uuid not null references auth.users(id) on delete cascade,
  kind text not null check (kind in ('buzzer', 'stop_buzzer')),
  status text not null default 'pending'
    check (status in ('pending', 'done', 'failed', 'expired')),
  created_at timestamptz not null default now(),
  expires_at timestamptz not null,
  completed_at timestamptz,
  error text
);
create index whoop_commands_pending_idx
  on public.whoop_commands (owner_id, created_at)
  where status = 'pending';

create table public.whoop_relay_status (
  owner_id uuid primary key references auth.users(id) on delete cascade,
  last_seen_at timestamptz not null default now(),
  connected boolean not null default false,
  current_bpm smallint,
  current_unix_ms bigint,
  strap_alarm_enabled boolean,
  strap_alarm_unix bigint,
  last_error text
);

alter table public.whoop_hr_samples enable row level security;
alter table public.whoop_alarms enable row level security;
alter table public.whoop_commands enable row level security;
alter table public.whoop_relay_status enable row level security;

revoke all on public.whoop_hr_samples, public.whoop_alarms,
  public.whoop_commands, public.whoop_relay_status from anon;

grant select on public.whoop_hr_samples to authenticated;
grant select, insert, update, delete on public.whoop_alarms to authenticated;
grant select, insert on public.whoop_commands to authenticated;
grant select on public.whoop_relay_status to authenticated;

create policy whoop_hr_owner_read on public.whoop_hr_samples
  for select to authenticated using ((select auth.uid()) = owner_id);

create policy whoop_alarm_owner_read on public.whoop_alarms
  for select to authenticated using ((select auth.uid()) = owner_id);
create policy whoop_alarm_owner_insert on public.whoop_alarms
  for insert to authenticated with check ((select auth.uid()) = owner_id);
create policy whoop_alarm_owner_update on public.whoop_alarms
  for update to authenticated using ((select auth.uid()) = owner_id)
  with check ((select auth.uid()) = owner_id);
create policy whoop_alarm_owner_delete on public.whoop_alarms
  for delete to authenticated using ((select auth.uid()) = owner_id);

create policy whoop_command_owner_read on public.whoop_commands
  for select to authenticated using ((select auth.uid()) = owner_id);
create policy whoop_command_owner_insert on public.whoop_commands
  for insert to authenticated with check ((select auth.uid()) = owner_id);

create policy whoop_relay_owner_read on public.whoop_relay_status
  for select to authenticated using ((select auth.uid()) = owner_id);
