import { memo, useEffect, useMemo, useState } from "react";
import type { SupabaseClient } from "@supabase/supabase-js";
import { ResponsiveContainer, ComposedChart, Line, Area, XAxis, YAxis, Tooltip, CartesianGrid } from "recharts";
import "./desktop-pulse.css";

type Point = { t: number; bpm: number | null; low?: number; high?: number; band?: [number, number]; samples?: number };
type History = { step_ms: number; points: Point[]; summary: { average: number | null; low: number | null; high: number | null; coverage: number; observed_seconds: number; samples: number }; available_from_ms: number | null };
type Episode = { start_ms: number; end_ms: number; duration_minutes: number; provisional?: boolean };
type Insight = { period_key: string; timezone: string; calculated_at: string; result: {
  sleep: { status: string; episodes: Episode[]; duration_minutes: number; coverage: number; motion_seconds: number; note: string };
  activity: { episodes: Episode[]; duration_minutes: number; threshold_bpm: number; note: string };
  pulse: { rr_intervals: number; rmssd_ms: number | null; coverage: number };
} };
const spans = { "1h": 3600000, "24h": 86400000, "7d": 604800000 };
function localValue(ms: number) { const d = new Date(ms); return new Date(ms - d.getTimezoneOffset() * 60000).toISOString().slice(0, 16); }
function duration(minutes: number) { const total = Math.round(minutes); return total >= 60 ? `${Math.floor(total / 60)}h ${total % 60}m` : `${total}m`; }
function clock(ms: number) { return new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }); }
function dayKey(ms: number) { return localValue(ms).slice(0, 10); }

function DesktopPulse({ client, owner }: { client: SupabaseClient; owner: string }) {
  const [span, setSpan] = useState<keyof typeof spans | "custom">("1h");
  const [anchor, setAnchor] = useState<number | null>(null);
  const [now, setNow] = useState(Date.now());
  const [customStart, setCustomStart] = useState(localValue(Date.now() - 86400000));
  const [customEnd, setCustomEnd] = useState(localValue(Date.now()));
  const [history, setHistory] = useState<History | null>(null);
  const [insight, setInsight] = useState<Insight | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => { const timer = window.setInterval(() => setNow(Date.now()), 30000); return () => window.clearInterval(timer); }, []);
  const end = span === "custom" ? new Date(customEnd).getTime() : anchor ?? now;
  const start = span === "custom" ? new Date(customStart).getTime() : end - spans[span];
  useEffect(() => {
    const controller = new AbortController();
    if (!Number.isFinite(start) || !Number.isFinite(end) || end <= start || end - start > 31 * 86400000) {
      setError("Choose a period of up to 31 days."); setHistory(null); setLoading(false); return;
    }
    setLoading(true); setError("");
    const load = async () => {
      const [series, report] = await Promise.all([
        client.rpc("whoop_pulse_history", { start_ms: Math.floor(start), end_ms: Math.floor(end), bucket_ms: Math.max(1000, Math.ceil((end - start) / 900)) }).abortSignal(controller.signal),
        client.from("whoop_insights").select("period_key,timezone,result,calculated_at").eq("owner_id", owner).lte("period_key", dayKey(Math.min(end - 1, Date.now()))).gte("period_key", dayKey(start - 86400000)).order("period_key", { ascending: false }).limit(1).abortSignal(controller.signal),
      ]);
      if (controller.signal.aborted) return;
      if (series.error || report.error) { setError((series.error || report.error)!.message); setHistory(null); setInsight(null); }
      else { setHistory(series.data as History); setInsight((report.data?.[0] as Insight) || null); }
      setLoading(false);
    };
    void load().catch(e => { if (!controller.signal.aborted) { setError(String(e)); setLoading(false); } });
    return () => controller.abort();
  }, [client, owner, start, end]);
  const points = useMemo(() => {
    const result: Point[] = [];
    for (const point of history?.points || []) {
      const previous = result.at(-1);
      if (previous && point.t - previous.t > (history?.step_ms || 1000) * 1.5) result.push({ t: previous.t + (history?.step_ms || 1000), bpm: null });
      result.push({ ...point, band: point.low != null && point.high != null ? [point.low, point.high] : undefined });
    }
    return result;
  }, [history]);
  function clearView() { setHistory(null); setInsight(null); }
  function move(direction: number) {
    if (!Number.isFinite(start) || !Number.isFinite(end) || end <= start) return;
    clearView();
    const width = end - start;
    if (span === "custom") { setCustomStart(localValue(start + direction * width)); setCustomEnd(localValue(end + direction * width)); }
    else setAnchor(Math.min(Date.now(), end + direction * width));
  }
  const sleep = insight?.result.sleep;
  const activity = insight?.result.activity;
  return <div className="desktop-pulse">
    <section className="panel history-panel" aria-label="Pulse history">
      <div className="history-heading"><h2>Pulse history</h2><div className="history-ranges" aria-label="History range">{(["1h", "24h", "7d", "custom"] as const).map(value => <button key={value} aria-pressed={span === value} onClick={() => { if (span === value && anchor === null) return; clearView(); setSpan(value); setAnchor(null); }}>{value === "custom" ? "Custom" : value}</button>)}</div></div>
      {span === "custom" && <div className="history-dates"><label>From<input type="datetime-local" value={customStart} onChange={e => { clearView(); setCustomStart(e.target.value); }} /></label><label>To<input type="datetime-local" value={customEnd} onChange={e => { clearView(); setCustomEnd(e.target.value); }} /></label></div>}
      <div className="history-navigation"><div><button aria-label="Previous period" onClick={() => move(-1)}>‹</button><button aria-label="Next period" disabled={end >= Date.now() - 60000} onClick={() => move(1)}>›</button></div><small>{Number.isFinite(start) && Number.isFinite(end) ? `${new Date(start).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" })} — ${new Date(end).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" })}` : "Select dates"}</small><button className="text-button" onClick={() => { if (span === "1h" && anchor === null) return; clearView(); setSpan("1h"); setAnchor(null); }}>Back to live</button></div>
      {error ? <p role="alert">{error}</p> : <><div className="history-chart" aria-busy={loading}><ResponsiveContainer width="100%" height="100%"><ComposedChart data={points} margin={{ top: 12, right: 12, bottom: 0, left: 0 }}>
        <CartesianGrid vertical={false} stroke="#294637" />
        <XAxis type="number" dataKey="t" domain={[start, end]} scale="time" tickFormatter={t => end - start > 86400000 ? new Date(t).toLocaleDateString([], { month: "short", day: "numeric" }) : clock(t)} minTickGap={60} stroke="#a6b8ae" />
        <YAxis width={40} domain={["dataMin - 5", "dataMax + 5"]} stroke="#a6b8ae" />
        <Tooltip contentStyle={{ background: "#102119", borderColor: "#42604d", borderRadius: 10 }} labelFormatter={t => new Date(Number(t)).toLocaleString()} formatter={(value, name) => name === "band" ? [Array.isArray(value) ? value.join("–") : value, "Range"] : [value, "BPM"]} />
        <Area dataKey="band" stroke="none" fill="#67e8a0" fillOpacity={0.12} isAnimationActive={false} connectNulls={false} />
        <Line dataKey="bpm" type="linear" stroke="#67e8a0" strokeWidth={1.8} dot={false} isAnimationActive={false} connectNulls={false} />
      </ComposedChart></ResponsiveContainer></div>
      {history?.summary.samples ? <p className="history-summary">Average <strong>{history.summary.average} BPM</strong><span>Range {history.summary.low}–{history.summary.high}</span><span>{Math.round(history.summary.coverage * 100)}% recorded</span>{loading && <span>Updating…</span>}</p> : <p className="history-summary">{loading ? "Loading history…" : "No readings recorded in this period."}</p>}
      {history?.available_from_ms && <small className="history-availability">Recorded history starts {new Date(history.available_from_ms).toLocaleDateString()}. Gaps remain visible.</small>}</>}
    </section>
    <div className="desktop-insights">
      <section className="panel insight-card"><div className="insight-heading"><h2>Sleep</h2><small>{insight?.period_key || "Collecting"}</small></div>
        {sleep?.episodes.length ? <><div className="insight-value">{duration(sleep.duration_minutes)}<small> estimated sleep</small></div><p>{clock(sleep.episodes[0].start_ms)} — {clock(sleep.episodes.at(-1)!.end_ms)}{sleep.episodes.some(e => e.provisional) ? " · ongoing / incomplete" : ""}</p></> : <><div className="insight-value muted">—</div><p>{sleep && sleep.motion_seconds >= 3600 ? "No sustained sleep detected in the recorded motion." : "Collecting motion data for a sleep estimate."}</p></>}
        <details><summary>Details</summary><p>{sleep?.note || "Estimates update automatically as sensor history arrives."}</p><p>Motion coverage: {Math.round((sleep?.coverage || 0) * 100)}%. Missing data is excluded.</p><p>Method: sustained wrist angle within 5° for at least 5 minutes; bouts of 30 minutes or more. Adapted from van Hees (2015).</p>{insight?.result.pulse.rmssd_ms != null && <p>Recorded RR RMSSD: {insight.result.pulse.rmssd_ms} ms ({insight.result.pulse.rr_intervals} intervals).</p>}</details>
      </section>
      <section className="panel insight-card"><div className="insight-heading"><h2>Activity</h2><small>{insight?.period_key || "Collecting"}</small></div><div className="insight-value">{activity ? duration(activity.duration_minutes) : "—"}<small> elevated pulse</small></div><p>{activity?.episodes.length ? `${activity.episodes.length} sustained period${activity.episodes.length === 1 ? "" : "s"}` : "No sustained elevated-pulse periods recorded."}</p><details><summary>Details</summary><p>{activity?.note || "Activity estimates update automatically."}</p><p>At least {activity?.threshold_bpm || 120} BPM for 10 consecutive minutes. Gaps break periods.</p><p>Pulse coverage: {Math.round((insight?.result.pulse.coverage || 0) * 100)}%.</p>{activity?.episodes.map(e => <p key={e.start_ms}>{clock(e.start_ms)} — {clock(e.end_ms)} · {duration(e.duration_minutes)}</p>)}</details></section>
    </div>
  </div>;
}

export default memo(DesktopPulse);
