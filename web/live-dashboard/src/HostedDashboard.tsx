import { useCallback, useEffect, useState } from "react";
import { createClient, type Session } from "@supabase/supabase-js";
import { ResponsiveContainer, LineChart, Line, XAxis, YAxis, Tooltip } from "recharts";
import "./hosted.css";

const url = import.meta.env.VITE_SUPABASE_URL as string;
const key = import.meta.env.VITE_SUPABASE_PUBLISHABLE_KEY as string;
const supabase = createClient(url, key, { auth: { persistSession: true, autoRefreshToken: true } });
type Alarm = { id: string; label: string; kind: string; cron_expr: string | null; timezone: string; one_time_unix: number | null; next_unix: number | null; enabled: boolean; last_rang_unix: number | null; last_missed_unix: number | null; last_unconfirmed_unix: number | null; last_programmed_unix: number | null };
type Sample = { recorded_at_ms: number; bpm: number };
type Relay = { last_seen_at: string; connected: boolean; current_bpm: number | null; current_unix_ms: number | null };
type Command = { id: string; kind: string; status: string; error: string | null };

function readable(unix: number | null) { return unix ? new Date(unix * 1000).toLocaleString() : "—"; }
function localInputValue(date: Date) { const adjusted = new Date(date.getTime() - date.getTimezoneOffset() * 60_000); return adjusted.toISOString().slice(0, 16); }

export default function HostedDashboard() {
  const [session, setSession] = useState<Session | null>(null);
  const [email, setEmail] = useState("");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);
  const [alarms, setAlarms] = useState<Alarm[]>([]);
  const [samples, setSamples] = useState<Sample[]>([]);
  const [relay, setRelay] = useState<Relay | null>(null);
  const [commands, setCommands] = useState<Command[]>([]);
  const [label, setLabel] = useState("Alarm");
  const [when, setWhen] = useState(() => localInputValue(new Date(Date.now() + 10 * 60_000)));
  const [repeat, setRepeat] = useState("once");
  const [tab, setTab] = useState<"pulse" | "alarms">("pulse");
  useEffect(() => {
    void supabase.auth.getSession().then(({ data }) => setSession(data.session));
    const { data } = supabase.auth.onAuthStateChange((_event, next) => setSession(next));
    return () => data.subscription.unsubscribe();
  }, []);
  const refresh = useCallback(async () => {
    if (!session?.user.id) return;
    const owner = session.user.id;
    const [hr, as, rs, cs] = await Promise.all([
      supabase.from("whoop_hr_samples").select("recorded_at_ms,bpm").eq("owner_id", owner).order("recorded_at_ms", { ascending: false }).limit(180),
      supabase.from("whoop_alarms").select("*").eq("owner_id", owner).order("next_unix", { ascending: true, nullsFirst: false }),
      supabase.from("whoop_relay_status").select("*").eq("owner_id", owner).maybeSingle(),
      supabase.from("whoop_commands").select("id,kind,status,error").eq("owner_id", owner).order("created_at", { ascending: false }).limit(8),
    ]);
    const error = hr.error || as.error || rs.error || cs.error;
    if (error) { setMessage(error.message); return; }
    setSamples(((hr.data || []) as Sample[]).reverse());
    setAlarms((as.data || []) as Alarm[]);
    setRelay(rs.data as Relay | null);
    setCommands((cs.data || []) as Command[]);
  }, [session?.user.id]);
  useEffect(() => {
    if (!session) return;
    void refresh();
    const id = window.setInterval(() => void refresh(), 3000);
    return () => window.clearInterval(id);
  }, [session, refresh]);
  async function act(work: () => Promise<void>) {
    setBusy(true); setMessage("");
    try { await work(); await refresh(); } catch (error) { setMessage(error instanceof Error ? error.message : String(error)); }
    finally { setBusy(false); }
  }
  async function createAlarm() {
    if (!session) return;
    const date = new Date(when);
    const unix = Math.floor(date.getTime() / 1000);
    if (!Number.isFinite(unix) || (repeat === "once" && unix <= Date.now() / 1000 + 20)) throw new Error("Choose a one-time alarm at least 20 seconds in the future.");
    const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    const cron_expr = repeat === "once" ? null : `0 ${date.getMinutes()} ${date.getHours()} * * ${repeat === "weekdays" ? "Mon-Fri" : "*"}`;
    const { error } = await supabase.from("whoop_alarms").insert({ owner_id: session.user.id, label: label.trim() || "Alarm", kind: repeat === "once" ? "once" : "cron", cron_expr, timezone, one_time_unix: repeat === "once" ? unix : null, enabled: true });
    if (error) throw error;
    setMessage("Alarm saved. Waiting for Mac and strap confirmation.");
  }
  async function toggle(alarm: Alarm) {
    if (alarm.kind === "once" && !alarm.enabled && (alarm.one_time_unix || 0) <= Date.now() / 1000) throw new Error("This one-time alarm has passed. Create a new alarm.");
    const { error } = await supabase.from("whoop_alarms").update({ enabled: !alarm.enabled, updated_at: new Date().toISOString() }).eq("id", alarm.id);
    if (error) throw error;
  }
  async function remove(alarm: Alarm) { const { error } = await supabase.from("whoop_alarms").delete().eq("id", alarm.id); if (error) throw error; }
  async function command(kind: "buzzer" | "stop_buzzer") {
    if (!session) return;
    const { error } = await supabase.from("whoop_commands").insert({ owner_id: session.user.id, kind, expires_at: new Date(Date.now() + 30_000).toISOString() });
    if (error) throw error;
  }
  const fresh = relay && Date.now() - new Date(relay.last_seen_at).getTime() < 30_000;
  const bpmFresh = relay?.current_unix_ms && Date.now() - relay.current_unix_ms < 45_000;
  if (!session) return <main className="hosted auth"><h1>OpenWhoop</h1><p>Sign in to your private dashboard.</p><form onSubmit={e => { e.preventDefault(); void act(async () => { const { error } = await supabase.auth.signInWithOtp({ email, options: { emailRedirectTo: window.location.origin + import.meta.env.BASE_URL, shouldCreateUser: false } }); if (error) throw error; setMessage("Check your email for the sign-in link."); }); }}><input type="email" required placeholder="Email address" value={email} onChange={e => setEmail(e.target.value)} /><button disabled={busy}>Send sign-in link</button></form><p role="status">{message}</p></main>;
  return <main className="hosted"><header><div><small>OPENWHOOP · PERSONAL</small><h1>{tab === "pulse" ? "Live pulse" : "Alarms"}</h1></div><div className="relay-state"><span className={fresh && relay?.connected ? "led on" : "led"}/>{fresh ? (relay?.connected ? "Mac + strap live" : "Mac online · strap waiting") : "Mac offline"}</div></header>
    <nav aria-label="Dashboard"><button className={tab === "pulse" ? "active" : ""} onClick={() => setTab("pulse")}>Pulse</button><button className={tab === "alarms" ? "active" : ""} onClick={() => setTab("alarms")}>Alarms</button><button onClick={() => void supabase.auth.signOut()}>Sign out</button></nav>
    {tab === "pulse" ? <><section className="hero"><small>HEART RATE</small><div className="bpm">{fresh && bpmFresh ? relay?.current_bpm ?? "—" : "—"}<span> BPM</span></div><p>{fresh && bpmFresh ? "Live from your Whoop" : "Waiting for a fresh strap sample"}</p></section><section className="panel"><h2>Recent pulse</h2><div className="chart"><ResponsiveContainer width="100%" height="100%"><LineChart data={samples}><XAxis dataKey="recorded_at_ms" tickFormatter={v => new Date(v).toLocaleTimeString([], {hour:"2-digit",minute:"2-digit"})} minTickGap={35}/><YAxis domain={["dataMin - 10", "dataMax + 10"]} width={35}/><Tooltip labelFormatter={v => new Date(Number(v)).toLocaleString()} formatter={v => [`${v} BPM`, "Heart rate"]}/><Line dataKey="bpm" type="monotone" stroke="#67e8a0" strokeWidth={2} dot={false} isAnimationActive={false}/></LineChart></ResponsiveContainer></div></section></> : <><section className="panel"><h2>Buzz now</h2><p>Send a short buzz or stop it from this phone.</p><div className="actions"><button disabled={busy || !fresh} onClick={() => void act(() => command("buzzer"))}>Buzz Whoop</button><button disabled={busy || !fresh} onClick={() => void act(() => command("stop_buzzer"))}>Stop buzz</button></div>{commands[0] && <small>Latest command: {commands[0].kind} · {commands[0].status}{commands[0].error ? ` · ${commands[0].error}` : ""}</small>}</section><section className="panel"><h2>New alarm</h2><div className="alarm-form"><label>Name<input value={label} maxLength={120} onChange={e => setLabel(e.target.value)}/></label><label>Time<input type="datetime-local" value={when} onChange={e => setWhen(e.target.value)}/></label><label>Repeat<select value={repeat} onChange={e => setRepeat(e.target.value)}><option value="once">Once</option><option value="daily">Every day</option><option value="weekdays">Weekdays</option></select></label><button disabled={busy} onClick={() => void act(createAlarm)}>Save alarm</button></div><small>The Mac programs the next enabled alarm on the strap. Keep the Mac awake and in Bluetooth range.</small></section><section className="panel"><h2>Saved alarms</h2>{alarms.length === 0 ? <p>No alarms yet.</p> : <div className="alarm-list">{alarms.map(a => <div className="alarm-row" key={a.id}><div><strong>{a.label}</strong><div className="alarm-time">{readable(a.next_unix || a.one_time_unix)}</div><small>{a.kind === "cron" ? `${a.cron_expr} · ${a.timezone}` : "Once"} · {a.last_rang_unix ? "Rang" : a.last_missed_unix ? "Missed" : a.last_unconfirmed_unix ? "Fired, awaiting confirmation" : a.last_programmed_unix && a.next_unix ? "Programmed" : a.enabled ? "Waiting for Mac" : "Off"}</small></div><div className="row-actions"><label className="switch" aria-label={`Enable ${a.label}`}><input type="checkbox" checked={a.enabled} disabled={busy} onChange={() => void act(() => toggle(a))}/><span/></label><button className="delete" disabled={busy} aria-label={`Delete ${a.label}`} onClick={() => void act(() => remove(a))}>Delete</button></div></div>)}</div>}</section></>}
    {message && <p role="status" className="message">{message}</p>}
  </main>;
}
