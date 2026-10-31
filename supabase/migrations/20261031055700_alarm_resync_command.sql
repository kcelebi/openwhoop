alter table public.whoop_commands
  drop constraint whoop_commands_kind_check;
alter table public.whoop_commands
  add constraint whoop_commands_kind_check
  check (kind in ('buzzer', 'stop_buzzer', 'resync_alarm'));
