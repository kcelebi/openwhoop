alter table public.whoop_relay_status
  add column battery_percent smallint check (battery_percent between 0 and 100),
  add column battery_updated_at timestamptz;
