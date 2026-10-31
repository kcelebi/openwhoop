import { useCallback, useEffect, useRef, useState } from "react";
import { createClient, type Session } from "@supabase/supabase-js";
import { ResponsiveContainer, LineChart, Line, XAxis, YAxis, Tooltip } from "recharts";
import "./hosted.css";

const supabase = createClient(
  import.meta.env.VITE_SUPABASE_URL as string,
  import.meta.env.VITE_SUPABASE_PUBLISHABLE_KEY as string,
  { auth: { persistSession: true, autoRefreshToken: true } },
);

type Alarm = {
  id: string;
  label: string;
  kind: string;
  cron_expr: string | null;
  timezone: string;
  one_time_unix: number | null;
  next_unix: number | null;
  enabled: boolean;
  last_rang_unix: number | null;
  last_missed_unix: number | null;
  last_unconfirmed_unix: number | null;
};
type Sample = { recorded_at_ms: number; bpm: number };
type Relay = {
  last_seen_at: string;
  connected: boolean;
  battery_percent: number | null;
  battery_updated_at: string | null;
  strap_alarm_enabled: boolean | null;
  strap_alarm_unix: number | null;
};
type Command = { kind: string; status: string; error: string | null };
type AlarmDraft = { id?: string; label: string; time: string; days: number[] };

const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const blankDraft = (): AlarmDraft => ({ label: "Alarm", time: "07:00", days: [0, 1, 2, 3, 4, 5, 6] });

function cronParts(alarm: Alarm): { time: string; days: number[] } {
  const fields = (alarm.cron_expr || "").trim().split(/\s+/);
  if (fields.length !== 6) return { time: "07:00", days: [0, 1, 2, 3, 4, 5, 6] };
  const minute = Number(fields[1]);
  const hour = Number(fields[2]);
  const time = `${String(hour).padStart(2, "0")}:${String(minute).padStart(2, "0")}`;
  const dayField = fields[5];
  const days = dayField === "*" ? [0, 1, 2, 3, 4, 5, 6]
    : dayField === "Mon-Fri" ? [1, 2, 3, 4, 5]
    : dayField.split(",").map(name => DAYS.indexOf(name)).filter(index => index >= 0);
  return { time, days };
}

function displayTime(time: string): string {
  const [hour, minute] = time.split(":").map(Number);
  return new Date(2020, 0, 1, hour, minute).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

function clockParts(time: string) {
  const [hour24, minute] = time.split(":").map(Number);
  return { hour: String(hour24 % 12 || 12), minute: String(minute).padStart(2, "0"), period: hour24 >= 12 ? "PM" : "AM" };
}

function changeClockPart(time: string, part: "hour" | "minute" | "period", value: string): string {
  const next = { ...clockParts(time), [part]: value };
  const hour24 = Number(next.hour) % 12 + (next.period === "PM" ? 12 : 0);
  return `${String(hour24).padStart(2, "0")}:${next.minute}`;
}

function repeatLabel(days: number[]): string {
  if (days.length === 7) return "Every day";
  if (days.join(",") === "1,2,3,4,5") return "Weekdays";
  if (days.join(",") === "0,6") return "Weekends";
  return days.map(day => DAYS[day]).join(" · ");
}

export default function HostedDashboard() {
  const [session, setSession] = useState<Session | null>(null);
  const [email, setEmail] = useState("");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [tab, setTab] = useState<"pulse" | "alarms">("pulse");
  const [alarms, setAlarms] = useState<Alarm[]>([]);
  const [samples, setSamples] = useState<Sample[]>([]);
  const [relay, setRelay] = useState<Relay | null>(null);
  const [command, setCommand] = useState<Command | null>(null);
  const [draft, setDraft] = useState<AlarmDraft | null>(null);
  const [lastSampleAt, setLastSampleAt] = useState<number | null>(null);
  const [lastHeartbeatAt, setLastHeartbeatAt] = useState<number | null>(null);
  const [now, setNow] = useState(Date.now());
  const lastSampleRef = useRef<number | null>(null);
  const lastHeartbeatRef = useRef<string | null>(null);
  const refreshBusy = useRef(false);

  useEffect(() => {
    void supabase.auth.getSession().then(({ data }) => setSession(data.session));
    const { data } = supabase.auth.onAuthStateChange((_event, next) => setSession(next));
    return () => data.subscription.unsubscribe();
  }, []);

  const refresh = useCallback(async () => {
    if (!session?.user.id || refreshBusy.current) return;
    refreshBusy.current = true;
    try {
      const owner = session.user.id;
      const [hr, alarmRows, status, commands] = await Promise.all([
        supabase.from("whoop_hr_samples").select("recorded_at_ms,bpm").eq("owner_id", owner).order("recorded_at_ms", { ascending: false }).limit(180),
        supabase.from("whoop_alarms").select("*").eq("owner_id", owner).order("next_unix", { ascending: true, nullsFirst: false }),
        supabase.from("whoop_relay_status").select("*").eq("owner_id", owner).maybeSingle(),
        supabase.from("whoop_commands").select("kind,status,error").eq("owner_id", owner).order("created_at", { ascending: false }).limit(1),
      ]);
      const error = hr.error || alarmRows.error || status.error || commands.error;
      if (error) throw error;
      const points = ((hr.data || []) as Sample[]).reverse();
      const latest = points.at(-1)?.recorded_at_ms;
      if (latest != null) {
        if (lastSampleRef.current != null && latest > lastSampleRef.current) setLastSampleAt(Date.now());
        lastSampleRef.current = latest;
      }
      const nextRelay = status.data as Relay | null;
      if (nextRelay && lastHeartbeatRef.current != null && nextRelay.last_seen_at !== lastHeartbeatRef.current) {
        setLastHeartbeatAt(Date.now());
      }
      lastHeartbeatRef.current = nextRelay?.last_seen_at || null;
      setSamples(points);
      setAlarms(((alarmRows.data || []) as Alarm[]).sort((a, b) => cronParts(a).time.localeCompare(cronParts(b).time)));
      setRelay(nextRelay);
      setCommand((commands.data?.[0] || null) as Command | null);
      setMessage(current => current.startsWith("Could not refresh") ? "" : current);
    } catch (error) {
      setMessage(`Could not refresh: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      refreshBusy.current = false;
    }
  }, [session?.user.id]);

  useEffect(() => {
    if (!session) return;
    void refresh();
    const timer = window.setInterval(() => void refresh(), 3000);
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    return () => { window.clearInterval(timer); window.removeEventListener("focus", onFocus); };
  }, [session, refresh]);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  async function act(work: () => Promise<void>) {
    setBusy(true);
    setMessage("");
    try { await work(); await refresh(); }
    catch (error) { setMessage(error instanceof Error ? error.message : String(error)); }
    finally { setBusy(false); }
  }

  async function saveAlarm() {
    if (!session || !draft) return;
    const [hour, minute] = draft.time.split(":").map(Number);
    if (!Number.isInteger(hour) || hour < 0 || hour > 23 || !Number.isInteger(minute) || minute < 0 || minute > 59) throw new Error("Choose a valid time.");
    if (!draft.days.length) throw new Error("Choose at least one day.");
    const days = [...draft.days].sort((a, b) => a - b);
    const cron_expr = `0 ${minute} ${hour} * * ${days.length === 7 ? "*" : days.map(day => DAYS[day]).join(",")}`;
    const row = { label: draft.label.trim() || "Alarm", kind: "cron", cron_expr,
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone, one_time_unix: null,
      updated_at: new Date().toISOString() };
    const result = draft.id
      ? await supabase.from("whoop_alarms").update(row).eq("id", draft.id)
      : await supabase.from("whoop_alarms").insert({ ...row, owner_id: session.user.id, enabled: true });
    if (result.error) throw result.error;
    setDraft(null);
    setMessage("Saved. Waiting for Whoop confirmation.");
  }

  async function toggle(alarm: Alarm) {
    const { error } = await supabase.from("whoop_alarms").update({ enabled: !alarm.enabled, updated_at: new Date().toISOString() }).eq("id", alarm.id);
    if (error) throw error;
  }
  async function remove(alarm: Alarm) {
    const { error } = await supabase.from("whoop_alarms").delete().eq("id", alarm.id);
    if (error) throw error;
  }
  async function sendCommand(kind: "buzzer" | "stop_buzzer" | "resync_alarm") {
    if (!session) return;
    const { error } = await supabase.from("whoop_commands").insert({ owner_id: session.user.id, kind,
      expires_at: new Date(Date.now() + 30_000).toISOString() });
    if (error) throw error;
    if (kind === "resync_alarm") setMessage("Resync requested. Checking the strap…");
  }

  const live = lastSampleAt != null && now - lastSampleAt < 30_000;
  const macOnline = live || (lastHeartbeatAt != null && now - lastHeartbeatAt < 30_000);
  const bpm = live ? samples.at(-1)?.bpm : null;
  const batteryFresh = relay?.battery_updated_at && now - new Date(relay.battery_updated_at).getTime() < 10 * 60_000;
  const battery = macOnline && batteryFresh ? relay?.battery_percent : null;
  const nextAlarm = alarms.filter(a => a.enabled && a.next_unix).sort((a, b) => (a.next_unix || 0) - (b.next_unix || 0))[0];
  const onStrap = macOnline && relay?.strap_alarm_enabled && nextAlarm?.next_unix === relay.strap_alarm_unix;

  if (!session) return <main className="hosted auth"><div className="auth-brand"><img src={`${import.meta.env.BASE_URL}k-mark.svg`} alt="" /><h1>OpenWhoop</h1></div><p>Your private Whoop dashboard.</p>
    <form onSubmit={event => { event.preventDefault(); void act(async () => {
      const { error } = await supabase.auth.signInWithOtp({ email, options: {
        emailRedirectTo: window.location.origin + import.meta.env.BASE_URL, shouldCreateUser: false,
      } });
      if (error) throw error;
      setMessage("Check your email for the sign-in link.");
    }); }}>
      <input type="email" required autoComplete="email" placeholder="Email address" value={email} onChange={event => setEmail(event.target.value)} />
      <button disabled={busy}>Send sign-in link</button>
    </form><p role="status">{message}</p></main>;

  return <main className="hosted">
    <header className="hosted-header"><div><div className="brand-line"><img src={`${import.meta.env.BASE_URL}k-mark.svg`} alt="" /><small>OPENWHOOP</small></div><h1>{tab === "pulse" ? "Pulse" : "Alarms"}</h1></div>
      <div className="header-right"><div className="header-status"><span className="battery-state" aria-label={battery == null ? "Whoop battery unavailable" : `Whoop battery ${battery}%`}><span className="battery-icon" aria-hidden="true" />{battery == null ? "—" : `${battery}%`}</span><span className="relay-state"><span className={`led ${live ? "on" : ""}`} />{live ? "Live" : macOnline ? "Mac online" : "Mac offline"}</span></div>
        <button className="text-button" onClick={() => void supabase.auth.signOut()}>Sign out</button></div></header>
    <nav aria-label="Dashboard"><button className={tab === "pulse" ? "active" : ""} onClick={() => setTab("pulse")}>Pulse</button>
      <button className={tab === "alarms" ? "active" : ""} onClick={() => setTab("alarms")}>Alarms</button></nav>

    {tab === "pulse" ? <><section className="hero"><small>HEART RATE</small><div className="bpm">{bpm ?? "—"}<span> BPM</span></div>
      <p>{live ? "Live from your Whoop" : "Waiting for the next strap sample"}</p></section>
      <section className="panel"><h2>Recent pulse</h2><div className="chart"><ResponsiveContainer width="100%" height="100%"><LineChart data={samples}>
        <XAxis dataKey="recorded_at_ms" tickFormatter={value => new Date(value).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })} minTickGap={35} />
        <YAxis domain={["dataMin - 10", "dataMax + 10"]} width={35} />
        <Tooltip labelFormatter={value => new Date(Number(value)).toLocaleString()} formatter={value => [`${value} BPM`, "Heart rate"]} />
        <Line dataKey="bpm" type="monotone" stroke="#67e8a0" strokeWidth={2} dot={false} isAnimationActive={false} />
      </LineChart></ResponsiveContainer></div></section></> : <>
      <section className="alarm-heading"><p>{!nextAlarm ? "No alarm armed" : onStrap ? `Synced to Whoop · ${displayTime(cronParts(nextAlarm).time)}` : macOnline ? "Checking Whoop alarm…" : "Mac offline · waiting to sync"}</p>
        <button className="add-alarm" onClick={() => setDraft(blankDraft())}>+ Add</button></section>
      {draft && <section className="panel alarm-editor"><div className="editor-title"><h2>{draft.id ? "Edit alarm" : "New alarm"}</h2><button className="text-button" onClick={() => setDraft(null)}>Cancel</button></div>
        <div className="time-field"><span>Time</span><div className="time-picker" role="group" aria-label="Alarm time">
          <select aria-label="Hour" value={clockParts(draft.time).hour} onChange={event => setDraft({ ...draft, time: changeClockPart(draft.time, "hour", event.target.value) })}>
            {Array.from({ length: 12 }, (_, index) => <option key={index + 1} value={index + 1}>{index + 1}</option>)}
          </select><span aria-hidden="true">:</span>
          <select aria-label="Minute" value={clockParts(draft.time).minute} onChange={event => setDraft({ ...draft, time: changeClockPart(draft.time, "minute", event.target.value) })}>
            {Array.from({ length: 60 }, (_, index) => String(index).padStart(2, "0")).map(minute => <option key={minute} value={minute}>{minute}</option>)}
          </select>
          <select aria-label="AM or PM" value={clockParts(draft.time).period} onChange={event => setDraft({ ...draft, time: changeClockPart(draft.time, "period", event.target.value) })}>
            <option value="AM">AM</option><option value="PM">PM</option>
          </select>
        </div></div>
        <details className="repeat-settings"><summary>Repeat <span>{repeatLabel(draft.days)}</span></summary>
          <div className="days">{DAYS.map((name, index) => <button key={name} type="button" className={draft.days.includes(index) ? "selected" : ""}
            aria-pressed={draft.days.includes(index)} onClick={() => setDraft({ ...draft,
              days: draft.days.includes(index) ? draft.days.filter(day => day !== index) : [...draft.days, index].sort() })}>{name}</button>)}</div>
          <label>Label<input maxLength={120} value={draft.label} onChange={event => setDraft({ ...draft, label: event.target.value })} /></label>
        </details>
        <div className="editor-actions"><button disabled={busy} onClick={() => void act(saveAlarm)}>Save alarm</button>
          {draft.id && <button className="delete" disabled={busy} onClick={() => void act(async () => {
            const alarm = alarms.find(item => item.id === draft.id); if (alarm) await remove(alarm); setDraft(null);
          })}>Delete</button>}</div></section>}
      <section className="alarm-list" aria-label="Saved alarms">{alarms.length === 0 ? <div className="empty-alarms">No alarms yet. Add a time to start.</div> : alarms.map(alarm => {
        const { time, days } = cronParts(alarm);
        const state = !alarm.enabled ? "Off" : !alarm.next_unix ? "Waiting for Mac" : onStrap && nextAlarm?.id === alarm.id ? "Synced to Whoop" : nextAlarm?.id === alarm.id ? "Syncing to Whoop" : "Queued";
        return <div className={`alarm-row ${alarm.enabled ? "" : "off"}`} key={alarm.id}>
          <button className="alarm-open" onClick={() => setDraft({ id: alarm.id, label: alarm.label, time, days })} aria-label={`Edit ${alarm.label}`}>
            <span className="alarm-time">{alarm.kind === "cron" ? displayTime(time) : new Date((alarm.one_time_unix || 0) * 1000).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}</span>
            <span className="alarm-sub">{alarm.label === "Alarm" ? (alarm.kind === "cron" ? repeatLabel(days) : "Once") : `${alarm.label} · ${alarm.kind === "cron" ? repeatLabel(days) : "Once"}`}</span>
            {(state !== "Queued" && state !== "Off") && <span className={`alarm-status ${state === "Synced to Whoop" ? "synced" : ""}`}>{state}</span>}
          </button><label className="switch" aria-label={`Enable ${alarm.label}`}><input type="checkbox" checked={alarm.enabled} disabled={busy} onChange={() => void act(() => toggle(alarm))} /><span /></label>
        </div>;
      })}</section>
      <div className="resync-row"><button disabled={busy || !macOnline} onClick={() => void act(() => sendCommand("resync_alarm"))}>↻ Resync Whoop</button></div>
      <details className="alarm-tools"><summary>Test & controls</summary><section className="haptic-tools"><button disabled={busy || !macOnline} onClick={() => void act(() => sendCommand("buzzer"))}>Test buzz</button>
        <button disabled={busy || !macOnline} onClick={() => void act(() => sendCommand("stop_buzzer"))}>Stop buzz</button>
        <small>The tested buzz pattern can last 10–15 seconds; stop may lag.</small>
        {command && <small>Last command: {command.kind} · {command.status}</small>}</section></details>
    </>}
    {message && <p role="status" className="message">{message}</p>}
  </main>;
}
