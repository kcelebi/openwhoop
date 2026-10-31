-- Supabase's automatic-RLS event trigger is not a client RPC. Restrict only
-- this trigger helper; owner-driven DDL/event-trigger execution remains valid.
do $$
begin
 if exists(select 1 from pg_proc p join pg_namespace n on n.oid=p.pronamespace
           where n.nspname='public' and p.proname='rls_auto_enable'
           and p.pronargs=0 and p.prorettype='event_trigger'::regtype) then
  revoke execute on function public.rls_auto_enable() from public,anon,authenticated;
 end if;
end $$;
