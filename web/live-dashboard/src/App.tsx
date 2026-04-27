import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type Dispatch,
  type SetStateAction,
} from "react";
import {
  ResponsiveContainer,
  Scatter,
  ScatterChart,
  Tooltip,
  XAxis,
  YAxis,
  CartesianGrid,
} from "recharts";

type SessionPhase =
  | "searching"
  | "connecting"
  | "syncing"
  | "catching_up"
  | "live_warmup"
  | "live"
  | "out_of_sync"
  | "reconnecting"
  | "offline";

type HeartRateMsg = {
  type: "heart_rate";
  unix_ms: number;
  bpm: number;
  time_local: string;
  rr_count: number;
  imu_count?: number;
  skin_contact?: number | null;
  signal_quality?: number | null;
  received_at_ms?: number;
};

type SessionStateMsg = {
  type: "session_state";
  state: SessionPhase;
  reason?: string;
  state_seq?: number;
  received_at_ms?: number;
};

type WsMsg =
  | HeartRateMsg
  | { type: "status"; message: string }
  | SessionStateMsg
  | { type: "lag_probe"; behind_wall_ms?: number; threshold_ms?: number }
  | {
      type: "connection_health";
      last_notification_age_ms?: number | null;
      reconnect_attempt?: number;
      scan_cycle?: number;
    }
  | { type: "history_batch_end"; readings_committed: number }
  | { type: "version"; harvard: string; boylston: string }
  | {
      type: "battery";
      percent: number | null;
      confidence?: string;
      source?: string;
      raw_tail_hex?: string;
      received_at_ms?: number;
    }
  | { type: "history_complete" }
  | { type: "history_batch_start" }
  | { type: "alarm_state"; enabled?: boolean; unix?: number }
  | { type: string; [key: string]: unknown };

const MAX_FEED = 48;

/** Dev: Vite proxies `/api` and `/ws` to Studio. For static hosting (e.g. GitHub Pages), set `VITE_STUDIO_ORIGIN` at build time to `https://your-api.example.com` (no trailing slash). */
const STUDIO_ORIGIN =
  (import.meta.env.VITE_STUDIO_ORIGIN as string | undefined)?.replace(
    /\/$/,
    "",
  ) ?? "";

function resolveApiUrl(path: string): string {
  if (!path.startsWith("/") || !STUDIO_ORIGIN) return path;
  return `${STUDIO_ORIGIN}${path}`;
}

function studioWebSocketUrl(): string {
  if (STUDIO_ORIGIN) {
    try {
      const u = new URL(STUDIO_ORIGIN);
      const wsProto = u.protocol === "https:" ? "wss:" : "ws:";
      return `${wsProto}//${u.host}/ws`;
    } catch {
      /* fall through */
    }
  }
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${location.host}/ws`;
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null;
}

function parseMsg(raw: string): WsMsg | null {
  try {
    const j: unknown = JSON.parse(raw);
    if (!isRecord(j) || typeof j.type !== "string") return null;
    return j as WsMsg;
  } catch {
    return null;
  }
}

function isHeartRateMsg(m: WsMsg): m is HeartRateMsg {
  return (
    m.type === "heart_rate" &&
    typeof (m as HeartRateMsg).bpm === "number" &&
    typeof (m as HeartRateMsg).unix_ms === "number"
  );
}

async function api(path: string, init?: RequestInit): Promise<unknown> {
  const headers = new Headers(init?.headers);
  if (init?.body != null && !headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }
  const r = await fetch(resolveApiUrl(path), { ...init, headers });
  const text = await r.text();
  let j: unknown = text;
  try {
    j = text ? JSON.parse(text) : null;
  } catch {
    /* plain */
  }
  if (!r.ok) {
    const err =
      isRecord(j) && typeof j.error === "string"
        ? j.error
        : typeof j === "string"
          ? j
          : text;
    throw new Error(err || r.statusText);
  }
  return j;
}

function wsLabel(state: "idle" | "connecting" | "open" | "closed"): string {
  switch (state) {
    case "idle":
      return "Starting…";
    case "connecting":
      return "Connecting…";
    case "open":
      return "Connected";
    case "closed":
      return "Disconnected";
    default:
      return state;
  }
}

type TabId = "pulse" | "trends" | "alarms" | "device" | "analysis";

const tabBtn = (active: boolean): CSSProperties => ({
  padding: "0.45rem 0.85rem",
  borderRadius: 8,
  border: `1px solid ${active ? "var(--accent)" : "var(--border)"}`,
  background: active ? "rgba(63, 185, 80, 0.12)" : "transparent",
  color: active ? "var(--accent)" : "var(--muted)",
  cursor: "pointer",
  fontSize: "0.82rem",
  fontWeight: active ? 600 : 400,
});

type FeedEntry = { id: string; at: number; text: string };

function pushFeed(set: Dispatch<SetStateAction<FeedEntry[]>>, text: string) {
  const id = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  set((prev) => [...prev.slice(-(MAX_FEED - 1)), { id, at: Date.now(), text }]);
}

function formatTimeLabel(ms: number): string {
  return new Date(ms).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

type HrPoint = { t_ms: number; bpm: number };

/** Recharts + DevTools struggle with 10k+ scatter points; keep UI responsive. */
const MAX_CHART_POINTS = 2500;

const MAX_PULSE_RAW_POINTS = 6000;
const SYNCING_LAG_MS = 15_000;
const HR_DRAIN_MIN_PER_FRAME = 1;
const HR_DRAIN_MAX_PER_FRAME = 24;

function downsampleHrPoints(points: HrPoint[], max: number): HrPoint[] {
  if (points.length <= max) return points;
  const step = Math.ceil(points.length / max);
  const out: HrPoint[] = [];
  for (let i = 0; i < points.length; i += step) {
    out.push(points[i]!);
  }
  const last = points[points.length - 1]!;
  if (out[out.length - 1]!.t_ms !== last.t_ms) out.push(last);
  return out;
}

type AlarmItem = {
  id: number;
  label: string;
  kind: "cron" | "once" | string;
  cron_expr?: string | null;
  one_time_unix?: number | null;
  next_unix?: number | null;
  last_rang_unix?: number | null;
  enabled: boolean;
};

/** Axis labels + tooltips (data instants are UTC ms from the API). */
const HR_CHART_TZ = "America/New_York";

/** If the strap stops sending, `phase` can stay "live" until the next packet — hide "Live" when stale. */
const LIVE_SAMPLE_MAX_AGE_MS = 45_000;
const LIVE_WINDOW_MS = 2 * 60_000;
const LIVE_TRANSITION_MS = 1_500;

function sessionLabel(state: SessionPhase | null): string {
  switch (state) {
    case "searching":
      return "Searching";
    case "connecting":
      return "Connecting";
    case "syncing":
      return "Syncing";
    case "catching_up":
      return "Catching up";
    case "live_warmup":
      return "Confirming live";
    case "live":
      return "Live";
    case "out_of_sync":
      return "Out of sync";
    case "reconnecting":
      return "Reconnecting";
    case "offline":
      return "Offline";
    default:
      return "Starting";
  }
}

function hrChartTickMs(v: number): string {
  return new Date(v).toLocaleTimeString("en-US", {
    timeZone: HR_CHART_TZ,
    hour: "2-digit",
    minute: "2-digit",
  });
}

function hrChartTooltipLabel(v: number): string {
  return new Date(v).toLocaleString("en-US", {
    timeZone: HR_CHART_TZ,
    dateStyle: "medium",
    timeStyle: "medium",
  });
}

function HrScatterDot(props: {
  cx?: number;
  cy?: number;
  fill?: string;
  fillOpacity?: number;
}) {
  const { cx, cy, fill, fillOpacity } = props;
  if (cx == null || cy == null) return null;
  return (
    <circle
      cx={cx}
      cy={cy}
      r={2.25}
      fill={fill ?? "var(--accent)"}
      fillOpacity={fillOpacity ?? 0.62}
    />
  );
}

function BatteryIndicator({ percent }: { percent: number | null }) {
  const pct =
    percent != null
      ? Math.max(0, Math.min(100, Math.round(percent)))
      : null;
  const fillW = pct != null ? (18 * pct) / 100 : 0;
  return (
    <div
      data-testid="live-battery"
      title={pct != null ? `Battery ${pct}%` : undefined}
      style={{
        display: "flex",
        alignItems: "center",
        gap: "0.65rem",
        padding: "0.4rem 0.95rem",
        borderRadius: 999,
        background:
          "linear-gradient(160deg, rgba(255,255,255,0.07) 0%, rgba(255,255,255,0.02) 100%)",
        border: "1px solid rgba(255,255,255,0.1)",
        boxShadow:
          "0 1px 0 rgba(255,255,255,0.06) inset, 0 8px 24px rgba(0,0,0,0.35)",
        flexShrink: 0,
      }}
    >
      <svg width="30" height="15" viewBox="0 0 30 15" aria-hidden>
        <defs>
          <linearGradient id="bat-cap" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="rgba(255,255,255,0.35)" />
            <stop offset="100%" stopColor="rgba(255,255,255,0.08)" />
          </linearGradient>
        </defs>
        <rect
          x="0.5"
          y="2.5"
          width="23"
          height="10"
          rx="2.5"
          fill="none"
          stroke="url(#bat-cap)"
          strokeWidth="1"
        />
        <rect
          x="24.5"
          y="5.5"
          width="3.5"
          height="4"
          rx="0.75"
          fill="rgba(255,255,255,0.25)"
        />
        {pct != null && fillW > 0.5 ? (
          <rect
            x="2"
            y="4.5"
            width={fillW}
            height="6"
            rx="1.25"
            fill="var(--accent)"
            opacity={0.88}
          />
        ) : null}
      </svg>
      <span
        style={{
          fontSize: "0.8rem",
          fontWeight: 600,
          letterSpacing: "0.04em",
          fontVariantNumeric: "tabular-nums",
          color: "var(--text)",
          minWidth: "2.5rem",
          textAlign: "right",
        }}
      >
        {pct != null ? `${pct}%` : "—"}
      </span>
    </div>
  );
}

export default function App() {
  const [tab, setTab] = useState<TabId>("pulse");
  const [wsState, setWsState] = useState<
    "idle" | "connecting" | "open" | "closed"
  >("idle");
  const [sessionState, setSessionState] = useState<SessionStateMsg | null>(null);
  const [lastLagMs, setLastLagMs] = useState<number | null>(null);
  const [lastHr, setLastHr] = useState<HeartRateMsg | null>(null);
  const [pulseSeries, setPulseSeries] = useState<HrPoint[]>([]);
  const [lastBattery, setLastBattery] = useState<{
    percent: number | null;
    confidence?: string;
    source?: string;
  } | null>(null);
  const [feed, setFeed] = useState<FeedEntry[]>([]);
  const [apiBusy, setApiBusy] = useState(false);
  const [apiError, setApiError] = useState<string | null>(null);

  const [autoRefreshInsights, setAutoRefreshInsights] = useState(true);
  const [insightsLoading, setInsightsLoading] = useState(false);
  const [sleepCard, setSleepCard] = useState<string>("");
  const [exerciseCard, setExerciseCard] = useState<string>("");
  const [vitalsCard, setVitalsCard] = useState<string>("");

  const [hrRange, setHrRange] = useState<"1h" | "24h" | "7d">("24h");
  const [hrSeries, setHrSeries] = useState<HrPoint[]>([]);
  const [hrSeriesStatus, setHrSeriesStatus] = useState<string>("");

  const chartHrPoints = useMemo(
    () => downsampleHrPoints(hrSeries, MAX_CHART_POINTS),
    [hrSeries],
  );

  const [liveTransitionStartMs, setLiveTransitionStartMs] = useState<
    number | null
  >(null);
  const [transitionTick, setTransitionTick] = useState(0);
  const prevSessionRef = useRef<SessionPhase | null>(null);

  const pulseChartPoints = useMemo(() => {
    if (pulseSeries.length === 0) return [];
    const state = sessionState?.state ?? null;
    const latestTs = pulseSeries[pulseSeries.length - 1]!.t_ms;
    if (state !== "live" && state !== "live_warmup") {
      return downsampleHrPoints(pulseSeries, MAX_CHART_POINTS);
    }
    const liveStart = latestTs - LIVE_WINDOW_MS;
    let effectiveStart = liveStart;
    if (liveTransitionStartMs != null) {
      const p = Math.max(
        0,
        Math.min(1, (Date.now() - liveTransitionStartMs) / LIVE_TRANSITION_MS),
      );
      const eased = 1 - (1 - p) * (1 - p);
      const fullStart = pulseSeries[0]!.t_ms;
      effectiveStart = fullStart + (liveStart - fullStart) * eased;
    }
    const windowed = pulseSeries.filter((p) => p.t_ms >= effectiveStart);
    return downsampleHrPoints(windowed, MAX_CHART_POINTS);
  }, [pulseSeries, sessionState?.state, liveTransitionStartMs, transitionTick]);

  const [alarmMode, setAlarmMode] = useState<"cron" | "once">("once");
  const [alarmLabel, setAlarmLabel] = useState<string>("Morning alarm");
  const [alarmLocal, setAlarmLocal] = useState<string>("");
  const [alarmCronExpr, setAlarmCronExpr] = useState<string>("0 7 * * Mon-Fri");
  const [alarmItems, setAlarmItems] = useState<AlarmItem[]>([]);
  const [alarmListStatus, setAlarmListStatus] = useState<string>("");
  const [strapAlarm, setStrapAlarm] = useState<{
    enabled?: boolean;
    unix?: number;
  } | null>(null);
  const [deviceNote, setDeviceNote] = useState<string>("");

  const [computeOut, setComputeOut] = useState<string>("");

  const wsRef = useRef<WebSocket | null>(null);
  const hrQueueRef = useRef<HrPoint[]>([]);
  const drainRafRef = useRef<number | null>(null);

  useEffect(() => {
    const prev = prevSessionRef.current;
    const next = sessionState?.state ?? null;
    if (next === "live" && prev !== "live") {
      setLiveTransitionStartMs(Date.now());
    }
    prevSessionRef.current = next;
  }, [sessionState?.state]);

  useEffect(() => {
    if (liveTransitionStartMs == null) return;
    let raf = 0;
    const tick = () => {
      setTransitionTick((v) => v + 1);
      if (Date.now() - liveTransitionStartMs >= LIVE_TRANSITION_MS) {
        setLiveTransitionStartMs(null);
        return;
      }
      raf = window.requestAnimationFrame(tick);
    };
    raf = window.requestAnimationFrame(tick);
    return () => window.cancelAnimationFrame(raf);
  }, [liveTransitionStartMs]);

  const runApi = useCallback(
    async (label: string, fn: () => Promise<unknown>) => {
      setApiBusy(true);
      setApiError(null);
      try {
        return await fn();
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        setApiError(`${label}: ${msg}`);
        throw e;
      } finally {
        setApiBusy(false);
      }
    },
    [],
  );

  const drainPulseQueueFrame = useCallback(() => {
    drainRafRef.current = null;
    if (hrQueueRef.current.length === 0) return;
    const backlog = hrQueueRef.current.length;
    const perFrame = Math.max(
      HR_DRAIN_MIN_PER_FRAME,
      Math.min(HR_DRAIN_MAX_PER_FRAME, Math.ceil(backlog / 30)),
    );
    const queued = hrQueueRef.current.splice(0, perFrame);
    setPulseSeries((prev) => {
      const next = [...prev, ...queued];
      if (next.length > MAX_PULSE_RAW_POINTS) {
        return next.slice(-MAX_PULSE_RAW_POINTS);
      }
      return next;
    });
    if (hrQueueRef.current.length > 0) {
      drainRafRef.current = window.requestAnimationFrame(drainPulseQueueFrame);
    }
  }, []);

  const schedulePulseDrain = useCallback(() => {
    if (drainRafRef.current != null) return;
    drainRafRef.current = window.requestAnimationFrame(drainPulseQueueFrame);
  }, [drainPulseQueueFrame]);

  const refreshInsights = useCallback(async () => {
    setInsightsLoading(true);
    setApiError(null);
    try {
      const [sleepJ, exJ, vitalsJ] = await Promise.all([
        api("/api/insights/sleep"),
        api("/api/insights/exercise"),
        api("/api/insights/vitals"),
      ]);

      const s = sleepJ as Record<string, unknown>;
      if (s.empty) setSleepCard(String(s.message ?? "No sleep data yet."));
      else
        setSleepCard(
          `Sleep · ${String(s.consistency_last_7_cycles ?? "").slice(0, 200)}${String(s.consistency_last_7_cycles ?? "").length > 200 ? "…" : ""}`,
        );

      const ex = exJ as Record<string, unknown>;
      if (ex.empty) setExerciseCard(String(ex.message ?? "No activities yet."));
      else
        setExerciseCard(
          `Activity · ${String(ex.metrics_last_7 ?? "").slice(0, 220)}${String(ex.metrics_last_7 ?? "").length > 220 ? "…" : ""}`,
        );

      const v = vitalsJ as { latest?: unknown[] };
      const n = v.latest?.length ?? 0;
      setVitalsCard(
        n === 0
          ? "No recent vitals rows."
          : `Latest ${n} samples loaded (stress, SpO₂, temperature where available).`,
      );
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setApiError(`Insights: ${msg}`);
    } finally {
      setInsightsLoading(false);
    }
  }, []);

  const loadAlarmSchedules = useCallback(async () => {
    try {
      const j = (await api("/api/alarms")) as { items?: AlarmItem[] };
      const items = Array.isArray(j.items) ? j.items : [];
      setAlarmItems(items);
      setAlarmListStatus(
        items.length === 0 ? "No alarms yet." : `${items.length} alarms`,
      );
    } catch (e) {
      setAlarmListStatus(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const loadHeartSeries = useCallback(async (range: "1h" | "24h" | "7d") => {
    setHrSeriesStatus("Loading…");
    try {
      const j = (await api(
        `/api/insights/heart-rate-series?range=${encodeURIComponent(range)}`,
      )) as { points?: HrPoint[]; count?: number };
      const pts = Array.isArray(j.points) ? j.points : [];
      setHrSeries(pts);
      setHrSeriesStatus(
        pts.length === 0 ? "No samples in this window." : `${pts.length} samples`,
      );
    } catch (e) {
      setHrSeries([]);
      setHrSeriesStatus(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    if (tab !== "trends") return;
    void loadHeartSeries(hrRange);
  }, [tab, hrRange, loadHeartSeries]);

  useEffect(() => {
    if (tab !== "trends" || !autoRefreshInsights) return;
    void refreshInsights();
    const id = window.setInterval(() => {
      void refreshInsights();
      void loadHeartSeries(hrRange);
    }, 60_000);
    return () => window.clearInterval(id);
  }, [tab, autoRefreshInsights, refreshInsights, loadHeartSeries, hrRange]);

  useEffect(() => {
    let cancelled = false;
    let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
    let attempt = 0;
    let everOpened = false;

    const url = studioWebSocketUrl();

    const connect = () => {
      if (cancelled) return;
      setWsState("connecting");
      const ws = new WebSocket(url);
      wsRef.current = ws;

      ws.onopen = () => {
        if (cancelled) {
          ws.close();
          return;
        }
        attempt = 0;
        setWsState("open");
        setPulseSeries([]);
        setLastHr(null);
        hrQueueRef.current = [];
        if (drainRafRef.current != null) {
          window.cancelAnimationFrame(drainRafRef.current);
          drainRafRef.current = null;
        }
        if (everOpened) {
          pushFeed(setFeed, "Live stream reconnected");
        } else {
          pushFeed(setFeed, "Live stream ready");
        }
        everOpened = true;
      };

      ws.onclose = () => {
        wsRef.current = null;
        if (cancelled) return;
        setWsState("closed");
        if (everOpened) {
          pushFeed(setFeed, "Live stream interrupted — reconnecting…");
        }
        const delay = Math.min(30_000, 800 * 2 ** Math.min(attempt, 5));
        attempt += 1;
        reconnectTimer = window.setTimeout(connect, delay);
      };

      ws.onmessage = (ev) => {
        const msg = parseMsg(String(ev.data));
        if (!msg) return;
        switch (msg.type) {
          case "heart_rate":
            if (isHeartRateMsg(msg)) {
              setLastHr(msg);
              hrQueueRef.current.push({ t_ms: msg.unix_ms, bpm: msg.bpm });
              schedulePulseDrain();
            }
            break;
          case "session_state": {
            const m = msg as SessionStateMsg;
            setSessionState(m);
            if (m.state === "out_of_sync" || m.state === "catching_up") {
              pushFeed(setFeed, `${sessionLabel(m.state)} · ${m.reason ?? "syncing backlog"}`);
            }
            if (m.state === "live") {
              pushFeed(setFeed, "Live confirmed · showing the last 2 minutes");
            }
            if (m.state === "offline") {
              pushFeed(setFeed, "Strap offline or out of range · scanning every minute");
            }
            break;
          }
          case "lag_probe": {
            const lag = (msg as { behind_wall_ms?: unknown }).behind_wall_ms;
            setLastLagMs(typeof lag === "number" ? lag : null);
            break;
          }
          case "battery": {
            const pct =
              typeof msg.percent === "number" || msg.percent === null
                ? msg.percent
                : null;
            setLastBattery({
              percent: pct,
              confidence:
                typeof (msg as { confidence?: unknown }).confidence === "string"
                  ? (msg as { confidence: string }).confidence
                  : undefined,
              source:
                typeof (msg as { source?: unknown }).source === "string"
                  ? (msg as { source: string }).source
                  : undefined,
            });
            break;
          }
          case "history_batch_start":
            pushFeed(setFeed, "Uploading a batch of readings…");
            break;
          case "history_batch_end":
            pushFeed(
              setFeed,
              `Saved ${(msg as { readings_committed: number }).readings_committed} heart-rate samples`,
            );
            break;
          case "history_complete":
            pushFeed(setFeed, "History checkpoint — continuing live updates");
            break;
          case "version":
            pushFeed(
              setFeed,
              `Strap firmware · ${(msg as { harvard: string }).harvard} / ${(msg as { boylston: string }).boylston}`,
            );
            break;
          case "strap_alarm_fired":
            pushFeed(setFeed, "Alarm fired on strap");
            break;
          case "device_event":
            pushFeed(
              setFeed,
              `Device · ${(msg as { command?: string }).command ?? "event"}`,
            );
            break;
          case "alarm_state": {
            const m = msg as { enabled?: boolean; unix?: number };
            setStrapAlarm({ enabled: m.enabled, unix: m.unix });
            break;
          }
          default:
            if (msg.type !== "status")
              pushFeed(setFeed, `Event · ${msg.type}`);
        }
      };
    };

    connect();
    return () => {
      cancelled = true;
      if (drainRafRef.current != null) {
        window.cancelAnimationFrame(drainRafRef.current);
        drainRafRef.current = null;
      }
      hrQueueRef.current = [];
      if (reconnectTimer !== undefined) window.clearTimeout(reconnectTimer);
      wsRef.current?.close();
      wsRef.current = null;
    };
  }, [pushFeed, schedulePulseDrain]);

  useEffect(() => {
    if (tab !== "alarms") return;
    void runApi("alarm-read", async () => {
      const j = (await api("/api/device/alarm")) as {
        enabled?: boolean;
        unix?: number;
      };
      setStrapAlarm({ enabled: j.enabled, unix: j.unix });
      await loadAlarmSchedules();
    }).catch(() => {});
  }, [tab, runApi, loadAlarmSchedules]);

  const lastReceivedAgeMs =
    lastHr?.received_at_ms != null
      ? Date.now() - lastHr.received_at_ms
      : null;
  const streamSampleRecent =
    lastReceivedAgeMs != null && lastReceivedAgeMs < LIVE_SAMPLE_MAX_AGE_MS;
  const lagBasedSyncing =
    lastLagMs != null
      ? lastLagMs > SYNCING_LAG_MS
      : wsState === "open" && lastHr == null;
  const streamStateLabel =
    wsState !== "open"
      ? wsLabel(wsState)
      : lagBasedSyncing
        ? "Syncing"
        : "Live";
  const showLivePill =
    lastHr != null &&
    !lagBasedSyncing &&
    streamSampleRecent;
  const pulseChartSyncing = wsState === "open" && lagBasedSyncing;

  return (
    <div
      style={{
        maxWidth: 960,
        margin: "0 auto",
        padding: "2rem 1.25rem 3rem",
      }}
    >
      <header
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "flex-start",
          gap: "1rem",
          marginBottom: "1.25rem",
        }}
      >
        <div style={{ minWidth: 0 }}>
          <h1
            style={{
              fontFamily: '"Fraunces", Georgia, serif',
              fontWeight: 700,
              fontSize: "1.85rem",
              letterSpacing: "-0.02em",
              margin: "0 0 0.35rem",
            }}
          >
            OpenWhoop Studio
          </h1>
          <p style={{ margin: 0, color: "var(--muted)", fontSize: "0.95rem" }}>
            Heart rate, recovery context, and strap controls in one place.
          </p>
        </div>
        <BatteryIndicator percent={lastBattery?.percent ?? null} />
      </header>

      <nav
        style={{
          display: "flex",
          flexWrap: "wrap",
          gap: "0.5rem",
          marginBottom: "1.25rem",
        }}
      >
        {(
          [
            ["pulse", "Pulse"],
            ["trends", "Trends"],
            ["alarms", "Alarms"],
            ["device", "Device"],
            ["analysis", "Analysis"],
          ] as const
        ).map(([id, label]) => (
          <button
            key={id}
            type="button"
            style={tabBtn(tab === id)}
            onClick={() => setTab(id)}
          >
            {label}
          </button>
        ))}
      </nav>

      {apiError ? (
        <div
          style={{
            marginBottom: "1rem",
            padding: "0.65rem 0.85rem",
            borderRadius: 8,
            background: "rgba(210, 153, 34, 0.12)",
            color: "var(--warn)",
            fontSize: "0.85rem",
          }}
        >
          {apiError}
        </div>
      ) : null}

      {tab === "pulse" ? (
        <>
          <div
            style={{
              border: "1px solid var(--border)",
              borderRadius: 12,
              background: "var(--surface)",
              padding: "1.25rem 1.5rem",
              marginBottom: "1.25rem",
            }}
          >
            <div
              style={{
                display: "flex",
                flexWrap: "wrap",
                alignItems: "stretch",
                gap: "1.5rem",
              }}
            >
              <div
                style={{
                  flex: "1 1 280px",
                  minWidth: 0,
                  position: "relative",
                }}
              >
                {pulseChartSyncing ? (
                  <div
                    style={{
                      position: "absolute",
                      top: 10,
                      left: 10,
                      zIndex: 2,
                      padding: "0.28rem 0.65rem",
                      borderRadius: 999,
                      fontSize: "0.68rem",
                      fontWeight: 600,
                      letterSpacing: "0.08em",
                      textTransform: "uppercase",
                      color: "var(--warn)",
                      background: "rgba(210, 153, 34, 0.14)",
                      border: "1px solid rgba(210, 153, 34, 0.35)",
                      pointerEvents: "none",
                    }}
                  >
                    Syncing
                  </div>
                ) : null}
                <div style={{ width: "100%", height: 300 }}>
                  {pulseChartPoints.length > 0 ? (
                    <ResponsiveContainer>
                      <ScatterChart
                        margin={{ top: 12, right: 12, bottom: 8, left: 4 }}
                      >
                        <CartesianGrid
                          strokeDasharray="3 3"
                          stroke="var(--border)"
                        />
                        <XAxis
                          type="number"
                          dataKey="t_ms"
                          domain={["dataMin", "dataMax"]}
                          tickFormatter={(v) => hrChartTickMs(v as number)}
                          stroke="var(--muted)"
                          tick={{ fill: "var(--muted)", fontSize: 10 }}
                        />
                        <YAxis
                          type="number"
                          dataKey="bpm"
                          name="BPM"
                          domain={["auto", "auto"]}
                          stroke="var(--muted)"
                          tick={{ fill: "var(--muted)", fontSize: 10 }}
                        />
                        <Tooltip
                          cursor={{ strokeDasharray: "3 3" }}
                          contentStyle={{
                            background: "var(--surface)",
                            border: "1px solid var(--border)",
                            borderRadius: 8,
                            color: "var(--text)",
                          }}
                          formatter={(value: number) => [
                            `${value} bpm`,
                            "Heart rate",
                          ]}
                          labelFormatter={(v) =>
                            hrChartTooltipLabel(v as number)
                          }
                        />
                        <Scatter
                          name="Heart rate"
                          data={pulseChartPoints}
                          shape={HrScatterDot}
                          fill="var(--accent)"
                          fillOpacity={0.62}
                        />
                      </ScatterChart>
                    </ResponsiveContainer>
                  ) : (
                    <div
                      style={{
                        height: "100%",
                        borderRadius: 8,
                        border: "1px dashed var(--border)",
                        background: "rgba(0,0,0,0.12)",
                      }}
                    />
                  )}
                </div>
              </div>

              <div
                className="pulse-readout-col"
                style={{
                  flex: "0 1 200px",
                  minWidth: 180,
                  display: "flex",
                  flexDirection: "column",
                  justifyContent: "space-between",
                  alignItems: "stretch",
                }}
              >
                <div
                  style={{
                    display: "flex",
                    alignItems: "baseline",
                    justifyContent: "space-between",
                    gap: "0.5rem",
                    marginBottom: "1rem",
                  }}
                >
                  <span
                    style={{
                      color: "var(--muted)",
                      fontSize: "0.72rem",
                      textTransform: "uppercase",
                      letterSpacing: "0.06em",
                    }}
                  >
                    Link
                  </span>
                  <span
                    data-testid="ws-state"
                    style={{
                      fontSize: "0.8rem",
                      fontWeight: 600,
                      color:
                        wsState === "open"
                          ? "var(--accent)"
                          : wsState === "connecting"
                            ? "var(--warn)"
                            : "var(--muted)",
                    }}
                  >
                    {wsLabel(wsState)}
                  </span>
                </div>
                <div
                  style={{
                    display: "flex",
                    alignItems: "baseline",
                    justifyContent: "space-between",
                    gap: "0.5rem",
                    marginBottom: "0.8rem",
                  }}
                >
                  <span
                    style={{
                      color: "var(--muted)",
                      fontSize: "0.72rem",
                      textTransform: "uppercase",
                      letterSpacing: "0.06em",
                    }}
                  >
                    Stream
                  </span>
                  <span
                    style={{
                      fontSize: "0.8rem",
                      fontWeight: 600,
                      color:
                        streamStateLabel === "Live"
                          ? "var(--accent)"
                          : streamStateLabel === "Offline"
                            ? "var(--warn)"
                            : "var(--muted)",
                    }}
                  >
                    {streamStateLabel}
                  </span>
                </div>
                <div style={{ textAlign: "center", flex: 1, display: "flex", flexDirection: "column", justifyContent: "center" }}>
                  {lastHr ? (
                    <>
                      <div
                        data-testid="live-bpm"
                        style={{
                          fontFamily: '"Fraunces", Georgia, serif',
                          fontSize: "3.75rem",
                          fontWeight: 700,
                          lineHeight: 1,
                          color: "var(--accent)",
                        }}
                      >
                        {lastHr.bpm}
                      </div>
                      <div
                        style={{
                          marginTop: "0.45rem",
                          color: "var(--muted)",
                          fontSize: "0.88rem",
                        }}
                      >
                        bpm · {lastHr.time_local}
                      </div>
                      {showLivePill ? (
                        <div
                          style={{
                            marginTop: "0.35rem",
                            fontSize: "0.75rem",
                            color: "var(--accent)",
                          }}
                        >
                          Live
                        </div>
                      ) : null}
                      {lastHr.received_at_ms != null ? (
                        <div
                          data-testid="live-received-at"
                          data-received-at={String(lastHr.received_at_ms)}
                          style={{
                            marginTop: "0.3rem",
                            fontSize: "0.7rem",
                            color: "var(--muted)",
                          }}
                        >
                          {new Date(lastHr.received_at_ms).toLocaleString()}
                        </div>
                      ) : null}
                      {sessionState?.reason ? (
                        <div
                          style={{
                            marginTop: "0.25rem",
                            fontSize: "0.68rem",
                            color: "var(--muted)",
                          }}
                        >
                          {sessionState.reason}
                        </div>
                      ) : null}
                      {lastLagMs != null ? (
                        <div
                          style={{
                            marginTop: "0.2rem",
                            fontSize: "0.68rem",
                            color: "var(--muted)",
                          }}
                        >
                          lag {Math.round(lastLagMs / 1000)}s
                        </div>
                      ) : null}
                    </>
                  ) : wsState === "open" ? (
                    <div className="studio-syncing">Syncing</div>
                  ) : (
                    <p style={{ margin: 0, color: "var(--muted)", fontSize: "0.88rem" }}>
                      Waiting…
                    </p>
                  )}
                </div>
              </div>
            </div>
          </div>

          <div
            style={{
              border: "1px solid var(--border)",
              borderRadius: 12,
              background: "var(--surface)",
              padding: "1rem 1.25rem",
            }}
          >
            <div
              style={{
                color: "var(--muted)",
                fontSize: "0.75rem",
                textTransform: "uppercase",
                letterSpacing: "0.06em",
                marginBottom: "0.75rem",
              }}
            >
              Activity
            </div>
            <ul
              style={{
                margin: 0,
                padding: 0,
                listStyle: "none",
                maxHeight: 240,
                overflow: "auto",
              }}
            >
              {feed.length === 0 ? (
                <li style={{ color: "var(--muted)", fontSize: "0.85rem" }}>
                  No events yet.
                </li>
              ) : (
                [...feed].reverse().map((e) => (
                  <li
                    key={e.id}
                    style={{
                      fontSize: "0.78rem",
                      color: "var(--muted)",
                      padding: "0.25rem 0",
                      borderBottom: "1px solid var(--border)",
                    }}
                  >
                    <span style={{ color: "var(--text)", marginRight: "0.5rem" }}>
                      {formatTimeLabel(e.at)}
                    </span>
                    {e.text}
                  </li>
                ))
              )}
            </ul>
          </div>
        </>
      ) : null}

      {tab === "trends" ? (
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: "1.25rem",
          }}
        >
          <div
            style={{
              display: "flex",
              flexWrap: "wrap",
              alignItems: "center",
              gap: "0.75rem",
              padding: "0.75rem 1rem",
              borderRadius: 12,
              border: "1px solid var(--border)",
              background: "var(--surface)",
            }}
          >
            <span style={{ fontSize: "0.85rem", color: "var(--muted)" }}>
              Refresh insights automatically
            </span>
            <button
              type="button"
              onClick={() => setAutoRefreshInsights((v) => !v)}
              style={{
                ...tabBtn(autoRefreshInsights),
                padding: "0.35rem 0.75rem",
              }}
            >
              {autoRefreshInsights ? "On" : "Off"}
            </button>
            <button
              type="button"
              disabled={insightsLoading}
              style={tabBtn(false)}
              onClick={() => void refreshInsights()}
            >
              {insightsLoading ? "Refreshing…" : "Refresh now"}
            </button>
          </div>

          <div
            style={{
              border: "1px solid var(--border)",
              borderRadius: 12,
              background: "var(--surface)",
              padding: "1.25rem",
            }}
          >
            <div
              style={{
                display: "flex",
                flexWrap: "wrap",
                justifyContent: "space-between",
                gap: "0.75rem",
                marginBottom: "1rem",
              }}
            >
              <span
                style={{
                  fontSize: "0.8rem",
                  textTransform: "uppercase",
                  letterSpacing: "0.06em",
                  color: "var(--muted)",
                }}
              >
                Heart rate
              </span>
              <div style={{ display: "flex", gap: "0.35rem", flexWrap: "wrap" }}>
                {(["1h", "24h", "7d"] as const).map((r) => (
                  <button
                    key={r}
                    type="button"
                    style={tabBtn(hrRange === r)}
                    onClick={() => setHrRange(r)}
                  >
                    {r === "1h"
                      ? "Last hour"
                      : r === "24h"
                        ? "Last day"
                        : "Last week"}
                  </button>
                ))}
              </div>
            </div>
            <p style={{ margin: "0 0 0.75rem", fontSize: "0.8rem", color: "var(--muted)" }}>
              {hrSeriesStatus}
            </p>
            <div style={{ width: "100%", height: 280 }}>
              <ResponsiveContainer>
                <ScatterChart margin={{ top: 8, right: 8, bottom: 8, left: 8 }}>
                  <CartesianGrid strokeDasharray="3 3" stroke="var(--border)" />
                  <XAxis
                    type="number"
                    dataKey="t_ms"
                    domain={["dataMin", "dataMax"]}
                    tickFormatter={(v) => hrChartTickMs(v as number)}
                    stroke="var(--muted)"
                    tick={{ fill: "var(--muted)", fontSize: 11 }}
                  />
                  <YAxis
                    type="number"
                    dataKey="bpm"
                    name="BPM"
                    domain={["auto", "auto"]}
                    stroke="var(--muted)"
                    tick={{ fill: "var(--muted)", fontSize: 11 }}
                  />
                  <Tooltip
                    cursor={{ strokeDasharray: "3 3" }}
                    contentStyle={{
                      background: "var(--surface)",
                      border: "1px solid var(--border)",
                      borderRadius: 8,
                      color: "var(--text)",
                    }}
                    formatter={(value: number) => [`${value} bpm`, "Heart rate"]}
                    labelFormatter={(v) => hrChartTooltipLabel(v as number)}
                  />
                  <Scatter
                    name="Heart rate"
                    data={chartHrPoints}
                    shape={HrScatterDot}
                    fill="var(--accent)"
                    fillOpacity={0.62}
                  />
                </ScatterChart>
              </ResponsiveContainer>
            </div>
          </div>

          <div
            style={{
              display: "grid",
              gap: "0.75rem",
              gridTemplateColumns: "repeat(auto-fit, minmax(240px, 1fr))",
            }}
          >
            <div
              style={{
                border: "1px solid var(--border)",
                borderRadius: 12,
                padding: "1rem",
                background: "var(--surface)",
              }}
            >
              <div
                style={{
                  fontSize: "0.72rem",
                  textTransform: "uppercase",
                  letterSpacing: "0.06em",
                  color: "var(--muted)",
                  marginBottom: "0.5rem",
                }}
              >
                Sleep
              </div>
              <p style={{ margin: 0, fontSize: "0.88rem", lineHeight: 1.45 }}>
                {sleepCard || "Open Trends to load, or wait for auto-refresh."}
              </p>
            </div>
            <div
              style={{
                border: "1px solid var(--border)",
                borderRadius: 12,
                padding: "1rem",
                background: "var(--surface)",
              }}
            >
              <div
                style={{
                  fontSize: "0.72rem",
                  textTransform: "uppercase",
                  letterSpacing: "0.06em",
                  color: "var(--muted)",
                  marginBottom: "0.5rem",
                }}
              >
                Activity
              </div>
              <p style={{ margin: 0, fontSize: "0.88rem", lineHeight: 1.45 }}>
                {exerciseCard || "—"}
              </p>
            </div>
            <div
              style={{
                border: "1px solid var(--border)",
                borderRadius: 12,
                padding: "1rem",
                background: "var(--surface)",
              }}
            >
              <div
                style={{
                  fontSize: "0.72rem",
                  textTransform: "uppercase",
                  letterSpacing: "0.06em",
                  color: "var(--muted)",
                  marginBottom: "0.5rem",
                }}
              >
                Vitals
              </div>
              <p style={{ margin: 0, fontSize: "0.88rem", lineHeight: 1.45 }}>
                {vitalsCard || "—"}
              </p>
            </div>
          </div>
        </div>
      ) : null}

      {tab === "alarms" ? (
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: "1.25rem",
          }}
        >
          <div
            style={{
              borderRadius: 16,
              padding: "1.5rem",
              background:
                "linear-gradient(135deg, rgba(63,185,80,0.14), rgba(88,214,232,0.1))",
              border: "1px solid rgba(88,214,232,0.35)",
              backdropFilter: "blur(12px)",
            }}
          >
            <h2
              style={{
                margin: "0 0 0.5rem",
                fontFamily: '"Fraunces", Georgia, serif',
                fontSize: "1.15rem",
              }}
            >
              Alarms
            </h2>
            <p style={{ margin: "0 0 1rem", fontSize: "0.88rem", color: "var(--muted)" }}>
              Configure multiple cron or one-time alarms. Scheduler keeps the next upcoming alarm
              programmed on the strap while it is in range.
            </p>
            {strapAlarm != null ? (
              <div
                style={{
                  fontSize: "0.9rem",
                  padding: "0.75rem 1rem",
                  borderRadius: 12,
                  background: "rgba(0,0,0,0.25)",
                  border: "1px solid var(--border)",
                }}
              >
                <div>
                  Status:{" "}
                  <strong>{strapAlarm.enabled ? "Scheduled" : "Off"}</strong>
                </div>
                {strapAlarm.unix != null && strapAlarm.enabled ? (
                  <div style={{ marginTop: "0.35rem", color: "var(--muted)" }}>
                    Next:{" "}
                    {new Date(strapAlarm.unix * 1000).toLocaleString()}
                  </div>
                ) : null}
              </div>
            ) : (
              <p style={{ color: "var(--muted)", fontSize: "0.85rem" }}>Loading…</p>
            )}
          </div>

          <div
            style={{
              border: "1px solid var(--border)",
              borderRadius: 12,
              background: "var(--surface)",
              padding: "1.25rem",
            }}
          >
            <h3 style={{ margin: "0 0 0.75rem", fontSize: "1rem" }}>Create alarm</h3>
            <div style={{ display: "flex", gap: "0.5rem", marginBottom: "0.75rem" }}>
              <button
                type="button"
                style={tabBtn(alarmMode === "once")}
                onClick={() => setAlarmMode("once")}
              >
                One-time
              </button>
              <button
                type="button"
                style={tabBtn(alarmMode === "cron")}
                onClick={() => setAlarmMode("cron")}
              >
                Cron
              </button>
            </div>
            <div
              style={{
                display: "flex",
                flexWrap: "wrap",
                gap: "0.75rem",
                alignItems: "center",
              }}
            >
              <input
                type="text"
                value={alarmLabel}
                onChange={(e) => setAlarmLabel(e.target.value)}
                placeholder="Label"
                style={{
                  padding: "0.45rem 0.6rem",
                  borderRadius: 8,
                  border: "1px solid var(--border)",
                  background: "var(--bg)",
                  color: "var(--text)",
                  minWidth: 180,
                }}
              />
              {alarmMode === "once" ? (
              <input
                type="datetime-local"
                value={alarmLocal}
                onChange={(e) => setAlarmLocal(e.target.value)}
                style={{
                  padding: "0.45rem 0.6rem",
                  borderRadius: 8,
                  border: "1px solid var(--border)",
                  background: "var(--bg)",
                  color: "var(--text)",
                }}
              />
              ) : (
                <input
                  type="text"
                  value={alarmCronExpr}
                  onChange={(e) => setAlarmCronExpr(e.target.value)}
                  placeholder="Cron e.g. 0 7 * * Mon-Fri"
                  style={{
                    padding: "0.45rem 0.6rem",
                    borderRadius: 8,
                    border: "1px solid var(--border)",
                    background: "var(--bg)",
                    color: "var(--text)",
                    minWidth: 300,
                  }}
                />
              )}
              <button
                type="button"
                disabled={
                  apiBusy ||
                  !alarmLabel.trim() ||
                  (alarmMode === "once" ? !alarmLocal : !alarmCronExpr.trim())
                }
                style={tabBtn(false)}
                onClick={() =>
                  void runApi("create-alarm", async () => {
                    const body =
                      alarmMode === "once"
                        ? {
                            label: alarmLabel.trim(),
                            kind: "once",
                            one_time_unix: Math.floor(
                              new Date(alarmLocal).getTime() / 1000,
                            ),
                          }
                        : {
                            label: alarmLabel.trim(),
                            kind: "cron",
                            cron_expr: alarmCronExpr.trim(),
                          };
                    await api("/api/alarms", {
                      method: "POST",
                      body: JSON.stringify(body),
                    });
                    await loadAlarmSchedules();
                    const st = (await api("/api/device/alarm")) as {
                      enabled?: boolean;
                      unix?: number;
                    };
                    setStrapAlarm({ enabled: st.enabled, unix: st.unix });
                    setDeviceNote("Alarm saved. Scheduler will program the next trigger.");
                  })
                }
              >
                Save alarm
              </button>
            </div>
            <p style={{ margin: "0.6rem 0 0", color: "var(--muted)", fontSize: "0.8rem" }}>
              Cron uses standard 5-field format in local time.
            </p>
          </div>

          <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap" }}>
            <button
              type="button"
              disabled={apiBusy}
              style={tabBtn(false)}
              onClick={() =>
                void runApi("alarm-refresh", async () => {
                  const j = (await api("/api/device/alarm")) as {
                    enabled?: boolean;
                    unix?: number;
                  };
                  setStrapAlarm({ enabled: j.enabled, unix: j.unix });
                })
              }
            >
              Refresh from strap
            </button>
          </div>
          <div
            style={{
              border: "1px solid var(--border)",
              borderRadius: 12,
              background: "var(--surface)",
              padding: "1rem",
            }}
          >
            <div
              style={{
                marginBottom: "0.5rem",
                fontSize: "0.8rem",
                textTransform: "uppercase",
                letterSpacing: "0.06em",
                color: "var(--muted)",
              }}
            >
              Saved alarms
            </div>
            <p style={{ margin: "0 0 0.6rem", color: "var(--muted)", fontSize: "0.8rem" }}>
              {alarmListStatus}
            </p>
            <div style={{ display: "flex", flexDirection: "column", gap: "0.55rem" }}>
              {alarmItems.map((a) => (
                <div
                  key={a.id}
                  style={{
                    border: "1px solid var(--border)",
                    borderRadius: 10,
                    padding: "0.65rem 0.75rem",
                    display: "flex",
                    justifyContent: "space-between",
                    gap: "0.75rem",
                    alignItems: "center",
                  }}
                >
                  <div>
                    <div style={{ fontWeight: 600, fontSize: "0.88rem" }}>
                      {a.label} · {a.kind}
                    </div>
                    <div style={{ fontSize: "0.78rem", color: "var(--muted)" }}>
                      {a.kind === "cron" && a.cron_expr
                        ? `cron: ${a.cron_expr}`
                        : a.one_time_unix
                          ? `one-time: ${new Date(a.one_time_unix * 1000).toLocaleString()}`
                          : "—"}
                    </div>
                    <div style={{ fontSize: "0.78rem", color: "var(--muted)" }}>
                      next:{" "}
                      {a.next_unix != null
                        ? new Date(a.next_unix * 1000).toLocaleString()
                        : "—"}{" "}
                      · last rang:{" "}
                      {a.last_rang_unix != null
                        ? new Date(a.last_rang_unix * 1000).toLocaleString()
                        : "—"}
                    </div>
                  </div>
                  <div style={{ display: "flex", gap: "0.45rem" }}>
                    <button
                      type="button"
                      style={tabBtn(a.enabled)}
                      onClick={() =>
                        void runApi("toggle-alarm", async () => {
                          await api(`/api/alarms/${a.id}`, {
                            method: "PATCH",
                            body: JSON.stringify({ enabled: !a.enabled }),
                          });
                          await loadAlarmSchedules();
                        })
                      }
                    >
                      {a.enabled ? "Enabled" : "Disabled"}
                    </button>
                    <button
                      type="button"
                      style={tabBtn(false)}
                      onClick={() =>
                        void runApi("delete-alarm", async () => {
                          await api(`/api/alarms/${a.id}`, { method: "DELETE" });
                          await loadAlarmSchedules();
                        })
                      }
                    >
                      Delete
                    </button>
                  </div>
                </div>
              ))}
            </div>
          </div>
          {deviceNote ? (
            <p style={{ margin: 0, fontSize: "0.85rem", color: "var(--muted)" }}>
              {deviceNote}
            </p>
          ) : null}
        </div>
      ) : null}

      {tab === "device" ? (
        <div
          style={{
            border: "1px solid var(--border)",
            borderRadius: 12,
            background: "var(--surface)",
            padding: "1.25rem",
            display: "flex",
            flexDirection: "column",
            gap: "1rem",
          }}
        >
          <p style={{ margin: 0, color: "var(--muted)", fontSize: "0.9rem" }}>
            Motion capture and diagnostics. Battery updates automatically on the Pulse tab; use
            refresh only if you want an immediate reading.
          </p>
          <div style={{ display: "flex", flexWrap: "wrap", gap: "0.5rem" }}>
            <button
              type="button"
              disabled={apiBusy}
              style={tabBtn(false)}
              onClick={() =>
                void runApi("battery", async () => {
                  await api("/api/device/battery", { method: "POST" });
                  setDeviceNote("Battery request sent; watch Pulse for the level.");
                })
              }
            >
              Refresh battery
            </button>
            <button
              type="button"
              disabled={apiBusy}
              style={tabBtn(false)}
              onClick={() =>
                void runApi("imu-r7", async () => {
                  await api("/api/device/imu/r7-toggle", { method: "POST" });
                  setDeviceNote("R7 motion logging toggled.");
                })
              }
            >
              Toggle motion logging
            </button>
          </div>
          <div style={{ display: "flex", flexWrap: "wrap", gap: "0.5rem" }}>
            <button
              type="button"
              disabled={apiBusy}
              style={tabBtn(false)}
              onClick={() =>
                void runApi("imu-live", async () => {
                  await api("/api/device/imu/mode", {
                    method: "POST",
                    body: JSON.stringify({ enable: true, historical: false }),
                  });
                  setDeviceNote("Live motion mode on.");
                })
              }
            >
              Motion · live
            </button>
            <button
              type="button"
              disabled={apiBusy}
              style={tabBtn(false)}
              onClick={() =>
                void runApi("imu-off", async () => {
                  await api("/api/device/imu/mode", {
                    method: "POST",
                    body: JSON.stringify({ enable: false, historical: false }),
                  });
                  setDeviceNote("Motion mode off.");
                })
              }
            >
              Motion · off
            </button>
            <button
              type="button"
              disabled={apiBusy}
              style={tabBtn(false)}
              onClick={() =>
                void runApi("imu-hist", async () => {
                  await api("/api/device/imu/mode", {
                    method: "POST",
                    body: JSON.stringify({ enable: true, historical: true }),
                  });
                  setDeviceNote("Historical motion capture on.");
                })
              }
            >
              Motion · historical
            </button>
          </div>
          {deviceNote ? (
            <p style={{ margin: 0, fontSize: "0.85rem", color: "var(--muted)" }}>
              {deviceNote}
            </p>
          ) : null}
        </div>
      ) : null}

      {tab === "analysis" ? (
        <div
          style={{
            border: "1px solid var(--border)",
            borderRadius: 12,
            background: "var(--surface)",
            padding: "1.25rem",
            display: "flex",
            flexDirection: "column",
            gap: "0.75rem",
          }}
        >
          <p style={{ margin: 0, color: "var(--muted)", fontSize: "0.9rem" }}>
            Recompute derived metrics on your stored readings. Large libraries take longer.
          </p>
          <div style={{ display: "flex", flexWrap: "wrap", gap: "0.5rem" }}>
            {(
              [
                ["/api/compute/stress", "Stress"],
                ["/api/compute/spo2", "SpO₂"],
                ["/api/compute/skin-temp", "Skin temperature"],
                ["/api/compute/detect-events", "Sleep & activity detection"],
              ] as const
            ).map(([path, label]) => (
              <button
                key={path}
                type="button"
                disabled={apiBusy}
                style={tabBtn(false)}
                onClick={() =>
                  void runApi(path, async () => {
                    const j = await api(path, { method: "POST" });
                    setComputeOut(JSON.stringify(j, null, 2));
                  })
                }
              >
                Run {label}
              </button>
            ))}
          </div>
          {computeOut ? (
            <pre
              style={{
                margin: 0,
                fontSize: "0.75rem",
                color: "var(--muted)",
                maxHeight: 200,
                overflow: "auto",
              }}
            >
              {computeOut}
            </pre>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
