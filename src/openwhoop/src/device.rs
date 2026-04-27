use anyhow::anyhow;
use btleplug::{
    api::{Central, CharPropFlags, Characteristic, Peripheral as _, WriteType},
    platform::{Adapter, Peripheral},
};
use chrono::Utc;
use futures::StreamExt;
use openwhoop_codec::{
    WhoopData, WhoopPacket,
    constants::{
        CMD_FROM_STRAP, CMD_TO_STRAP, CommandNumber, DATA_FROM_STRAP, EVENTS_FROM_STRAP, MEMFAULT,
        PacketType, WHOOP_SERVICE,
    },
};
use openwhoop_entities::packets::Model;
use serde_json::json;
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep, timeout};
use uuid::Uuid;

use crate::{StudioDeviceJob, db::DatabaseHandler, openwhoop::OpenWhoop};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    Searching,
    Connecting,
    Syncing,
    CatchingUp,
    LiveWarmup,
    Live,
    OutOfSync,
    Reconnecting,
    Offline,
}

impl SessionState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Searching => "searching",
            Self::Connecting => "connecting",
            Self::Syncing => "syncing",
            Self::CatchingUp => "catching_up",
            Self::LiveWarmup => "live_warmup",
            Self::Live => "live",
            Self::OutOfSync => "out_of_sync",
            Self::Reconnecting => "reconnecting",
            Self::Offline => "offline",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SessionTracker {
    pub state: SessionState,
    pub state_seq: u64,
    live_probe_streak: u8,
}

impl Default for SessionTracker {
    fn default() -> Self {
        Self {
            state: SessionState::Searching,
            state_seq: 0,
            live_probe_streak: 0,
        }
    }
}

enum StudioPending {
    Battery {
        reply: oneshot::Sender<Result<serde_json::Value, String>>,
        gatt_percent: Option<u8>,
    },
    AlarmRead(oneshot::Sender<Result<serde_json::Value, String>>),
}

pub struct WhoopDevice {
    peripheral: Peripheral,
    whoop: OpenWhoop,
    debug_packets: bool,
    adapter: Adapter,
    studio_pending: Option<StudioPending>,
    last_notification_at: Option<Instant>,
}

impl WhoopDevice {
    const LAG_THRESHOLD_MS: i64 = 15_000;
    const LIVE_PROBE_INTERVAL: Duration = Duration::from_secs(5);
    const QUIET_HISTORY_NUDGE: Duration = Duration::from_secs(10);

    pub fn new(
        peripheral: Peripheral,
        adapter: Adapter,
        db: DatabaseHandler,
        debug_packets: bool,
    ) -> Self {
        Self::with_openwhoop(peripheral, adapter, OpenWhoop::new(db), debug_packets)
    }

    pub fn with_openwhoop(
        peripheral: Peripheral,
        adapter: Adapter,
        whoop: OpenWhoop,
        debug_packets: bool,
    ) -> Self {
        Self {
            peripheral,
            whoop,
            debug_packets,
            adapter,
            studio_pending: None,
            last_notification_at: None,
        }
    }

    fn emit_session_state(&self, tracker: &mut SessionTracker, state: SessionState, reason: &str) {
        if tracker.state == state {
            return;
        }
        tracker.state = state;
        tracker.state_seq += 1;
        self.whoop.emit_live_json(json!({
            "type": "session_state",
            "state": state.as_str(),
            "reason": reason,
            "state_seq": tracker.state_seq,
            "received_at_ms": Utc::now().timestamp_millis(),
        }));
    }

    fn emit_lag_probe(&self, lag_ms: i64, threshold_ms: i64) {
        self.whoop.emit_live_json(json!({
            "type": "lag_probe",
            "behind_wall_ms": lag_ms.max(0),
            "threshold_ms": threshold_ms,
            "source": "history_reading",
            "received_at_ms": Utc::now().timestamp_millis(),
        }));
    }

    fn emit_connection_health(&self, reconnect_attempt: u8, scan_cycle: u32) {
        let last_notification_age_ms = self
            .last_notification_at
            .map(|t| i64::try_from(t.elapsed().as_millis()).unwrap_or(i64::MAX));
        self.whoop.emit_live_json(json!({
            "type": "connection_health",
            "last_notification_age_ms": last_notification_age_ms,
            "reconnect_attempt": reconnect_attempt,
            "scan_cycle": scan_cycle,
            "received_at_ms": Utc::now().timestamp_millis(),
        }));
    }

    pub fn emit_external_session_state(
        &self,
        tracker: &mut SessionTracker,
        state: SessionState,
        reason: &str,
    ) {
        self.emit_session_state(tracker, state, reason);
    }

    fn next_state_from_lag_probe(
        current: SessionState,
        live_probe_streak: u8,
        lag_ms: i64,
        threshold_ms: i64,
    ) -> (SessionState, u8) {
        if lag_ms >= threshold_ms {
            return (SessionState::CatchingUp, 0);
        }
        let streak = live_probe_streak.saturating_add(1);
        let _ = current;
        (SessionState::Live, streak)
    }

    pub async fn connect(&mut self) -> anyhow::Result<()> {
        self.peripheral.connect().await?;
        let _ = self.adapter.stop_scan().await;
        self.peripheral.discover_services().await?;
        self.whoop.packet = None;
        info!("Connected; GATT services discovered");
        Ok(())
    }

    pub async fn is_connected(&mut self) -> anyhow::Result<bool> {
        let is_connected = self.peripheral.is_connected().await?;
        Ok(is_connected)
    }

    fn create_char(characteristic: Uuid) -> Characteristic {
        Characteristic {
            uuid: characteristic,
            service_uuid: WHOOP_SERVICE,
            properties: CharPropFlags::empty(),
            descriptors: BTreeSet::new(),
        }
    }

    async fn subscribe(&self, char: Uuid) -> anyhow::Result<()> {
        self.peripheral.subscribe(&Self::create_char(char)).await?;
        Ok(())
    }

    pub async fn initialize(&mut self) -> anyhow::Result<()> {
        self.subscribe(DATA_FROM_STRAP).await?;
        self.subscribe(CMD_FROM_STRAP).await?;
        self.subscribe(EVENTS_FROM_STRAP).await?;
        self.subscribe(MEMFAULT).await?;

        self.send_command(WhoopPacket::hello_harvard()).await?;
        self.send_command(WhoopPacket::set_time()?).await?;
        self.send_command(WhoopPacket::get_name()).await?;

        self.send_command(WhoopPacket::enter_high_freq_sync())
            .await?;
        info!("Initialization complete (notifications + time sync + high-freq sync)");
        Ok(())
    }

    pub async fn send_command(&mut self, packet: WhoopPacket) -> anyhow::Result<()> {
        let packet = packet.framed_packet()?;
        self.peripheral
            .write(
                &Self::create_char(CMD_TO_STRAP),
                &packet,
                WriteType::WithoutResponse,
            )
            .await?;
        Ok(())
    }

    pub async fn sync_history(&mut self, should_exit: Arc<AtomicBool>) -> anyhow::Result<()> {
        let mut notifications = self.peripheral.notifications().await?;

        self.send_command(WhoopPacket::history_start()).await?;
        info!(
            "Sent history_start; waiting for BLE notifications (HistoryReading lines = parsed samples)"
        );

        let mut rx_count: u64 = 0;

        'a: loop {
            if should_exit.load(Ordering::SeqCst) {
                break;
            }
            let notification = notifications.next();
            let sleep_ = sleep(Duration::from_secs(10));

            tokio::select! {
                _ = sleep_ => {
                    if self.on_sleep().await? {
                        error!("Whoop disconnected");
                        for _ in 0..5{
                            if self.connect().await.is_ok() {
                                self.initialize().await?;
                                self.send_command(WhoopPacket::history_start()).await?;
                                rx_count = 0;
                                continue 'a;
                            }

                            sleep(Duration::from_secs(10)).await;
                        }

                        break;
                    } else if rx_count == 0 {
                        info!("Still connected, no notifications yet — keep strap awake; waiting…");
                    } else {
                        info!(
                            "Still connected; received {} BLE notifications so far (see HistoryReading / batch logs above)",
                            rx_count
                        );
                    }
                },
                Some(notification) = notification => {
                    rx_count += 1;
                    let len = notification.value.len();
                    if rx_count <= 8 || rx_count % 200 == 0 {
                        info!(
                            "BLE rx #{} char={} len={} bytes",
                            rx_count, notification.uuid, len
                        );
                    }

                    let packet = match self.debug_packets {
                        true => self.whoop.store_packet(notification).await?,
                        false => Model { id: 0, uuid: notification.uuid, bytes: notification.value },
                    };

                    if let Some(packet) = self.whoop.handle_packet(packet).await?{
                        self.send_command(packet).await?;
                    }
                }
            }
        }

        info!("sync_history loop ended after {rx_count} BLE notifications");
        Ok(())
    }

    /// Like [`Self::sync_history`], but also serves Studio dashboard device API jobs from `jobs`.
    pub async fn sync_history_with_studio_jobs(
        &mut self,
        should_exit: Arc<AtomicBool>,
        jobs: &mut mpsc::Receiver<StudioDeviceJob>,
        tracker: &mut SessionTracker,
        scan_cycle: u32,
    ) -> anyhow::Result<()> {
        let mut notifications = self.peripheral.notifications().await?;

        self.emit_session_state(tracker, SessionState::Syncing, "notifications_ready");
        self.send_command(WhoopPacket::history_start()).await?;
        self.emit_session_state(tracker, SessionState::CatchingUp, "history_start_sent");
        info!("Sent history_start; studio device API + BLE notifications active");

        let mut rx_count: u64 = 0;

        'a: loop {
            if should_exit.load(Ordering::SeqCst) {
                break;
            }
            let notification = notifications.next();
            let sleep_ = sleep(Self::LIVE_PROBE_INTERVAL);

            tokio::select! {
                biased;
                Some(notification) = notification => {
                    self.last_notification_at = Some(Instant::now());
                    rx_count += 1;
                    let len = notification.value.len();
                    if rx_count <= 8 || rx_count % 200 == 0 {
                        info!(
                            "BLE rx #{} char={} len={} bytes",
                            rx_count, notification.uuid, len
                        );
                    }

                    if notification.uuid == CMD_FROM_STRAP {
                        if self.try_complete_studio_pending(&notification.value)? {
                            continue;
                        }
                    }

                    let packet = match self.debug_packets {
                        true => self.whoop.store_packet(notification).await?,
                        false => Model { id: 0, uuid: notification.uuid, bytes: notification.value },
                    };

                    if let Some(packet) = self.whoop.handle_packet(packet).await? {
                        self.send_command(packet).await?;
                    }
                    if self.whoop.take_history_refresh_requested() {
                        info!("HistoryComplete: issuing history_start for incremental window");
                        self.send_command(WhoopPacket::history_start()).await?;
                        self.emit_session_state(
                            tracker,
                            SessionState::CatchingUp,
                            "history_complete_refresh",
                        );
                        tracker.live_probe_streak = 0;
                    }
                }
                Some(job) = jobs.recv() => {
                    self.handle_studio_job(job).await?;
                }
                _ = sleep_ => {
                    self.emit_connection_health(0, scan_cycle);
                    if self.on_sleep().await? {
                        error!("Whoop disconnected");
                        self.emit_session_state(tracker, SessionState::Reconnecting, "ble_disconnected");
                        for attempt in 1..=3 {
                            self.emit_connection_health(attempt, scan_cycle);
                            if self.connect().await.is_ok() {
                                self.initialize().await?;
                                self.send_command(WhoopPacket::history_start()).await?;
                                self.emit_session_state(tracker, SessionState::Syncing, "reconnected");
                                self.emit_session_state(tracker, SessionState::CatchingUp, "history_start_after_reconnect");
                                tracker.live_probe_streak = 0;
                                rx_count = 0;
                                continue 'a;
                            }
                            sleep(Duration::from_secs(u64::from(attempt * 2))).await;
                        }
                        return Err(anyhow!("device disconnected; reconnect attempts exhausted"));
                    } else if rx_count == 0 {
                        info!("Still connected, no notifications yet — keep strap awake; waiting…");
                    }
                    let now = Instant::now();
                    let quiet = self
                        .last_notification_at
                        .map(|t| now.duration_since(t) >= Self::QUIET_HISTORY_NUDGE)
                        .unwrap_or(true);
                    // Keep nudging history even while a Studio API request (battery/alarm) is
                    // waiting on CMD_FROM_STRAP — otherwise a stuck or slow command response
                    // stops history_start and the Pulse stream goes quiet after "sync".
                    if quiet {
                        let _ = self.send_command(WhoopPacket::history_start()).await;
                        if tracker.state == SessionState::Live || tracker.state == SessionState::LiveWarmup {
                            self.emit_session_state(tracker, SessionState::OutOfSync, "quiet_link_nudge");
                        }
                    }

                    let wall_now = Utc::now().timestamp_millis();
                    if let Some(lag_ms) = self.whoop.latest_lag_ms(wall_now) {
                        self.emit_lag_probe(lag_ms, Self::LAG_THRESHOLD_MS);
                        let (next, streak) = Self::next_state_from_lag_probe(
                            tracker.state,
                            tracker.live_probe_streak,
                            lag_ms,
                            Self::LAG_THRESHOLD_MS,
                        );
                        tracker.live_probe_streak = streak;
                        match next {
                            SessionState::CatchingUp => {
                                self.emit_session_state(tracker, SessionState::OutOfSync, "lag_above_threshold");
                                self.emit_session_state(tracker, SessionState::CatchingUp, "catch_up_active");
                            }
                            SessionState::Live => {
                                self.emit_session_state(tracker, SessionState::Live, "live_confirmed");
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        info!("sync_history_with_studio_jobs ended after {rx_count} BLE notifications");
        Ok(())
    }

    fn try_complete_studio_pending(&mut self, raw: &[u8]) -> anyhow::Result<bool> {
        let Some(pending) = self.studio_pending.take() else {
            return Ok(false);
        };

        let wp = match WhoopPacket::from_data(raw.to_vec()) {
            Ok(p) => p,
            Err(_) => {
                self.studio_pending = Some(pending);
                return Ok(false);
            }
        };

        if wp.packet_type != PacketType::CommandResponse {
            self.studio_pending = Some(pending);
            return Ok(false);
        }

        match pending {
            StudioPending::Battery {
                reply,
                gatt_percent,
            } => {
                if wp.cmd != CommandNumber::GetBatteryLevel.as_u8() {
                    self.studio_pending = Some(StudioPending::Battery {
                        reply,
                        gatt_percent,
                    });
                    return Ok(false);
                }
                match WhoopData::from_packet(wp) {
                    Ok(WhoopData::BatteryLevel {
                        percent,
                        raw_tail_hex,
                    }) => {
                        let v = Self::resolve_battery(percent, gatt_percent, Some(raw_tail_hex));
                        let _ = reply.send(Ok(v));
                    }
                    Ok(o) => {
                        let _ = reply.send(Err(format!("unexpected response: {o:?}")));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e.to_string()));
                    }
                }
                Ok(true)
            }
            StudioPending::AlarmRead(tx) => {
                if wp.cmd != CommandNumber::GetAlarmTime.as_u8() {
                    self.studio_pending = Some(StudioPending::AlarmRead(tx));
                    return Ok(false);
                }
                match WhoopData::from_packet(wp) {
                    Ok(WhoopData::AlarmInfo { enabled, unix }) => {
                        let _ = tx.send(Ok(serde_json::json!({
                            "enabled": enabled,
                            "unix": unix,
                        })));
                    }
                    Ok(o) => {
                        let _ = tx.send(Err(format!("unexpected response: {o:?}")));
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string()));
                    }
                }
                Ok(true)
            }
        }
    }

    async fn handle_studio_job(&mut self, job: StudioDeviceJob) -> anyhow::Result<()> {
        match job {
            StudioDeviceJob::PollBattery { reply } => {
                if let Some(prev) = self.studio_pending.take() {
                    match prev {
                        StudioPending::Battery { reply, .. } => {
                            let _ = reply.send(Err(
                                "previous battery poll replaced (timeout or slow response)".into(),
                            ));
                        }
                        StudioPending::AlarmRead(tx) => {
                            self.studio_pending = Some(StudioPending::AlarmRead(tx));
                            let _ = reply.send(Err(
                                "device busy (alarm read in flight — retry battery)".into(),
                            ));
                            return Ok(());
                        }
                    }
                }
                let gatt_percent = self.read_ble_battery_level().await;
                self.studio_pending = Some(StudioPending::Battery {
                    reply,
                    gatt_percent,
                });
                self.send_command(WhoopPacket::get_battery_level()).await?;
            }
            StudioDeviceJob::GetAlarm { reply } => {
                if self.studio_pending.is_some() {
                    let _ = reply.send(Err("device busy (another request in flight)".into()));
                    return Ok(());
                }
                self.studio_pending = Some(StudioPending::AlarmRead(reply));
                self.send_command(WhoopPacket::get_alarm_time()).await?;
            }
            StudioDeviceJob::SetAlarm { unix, reply } => {
                self.send_command(WhoopPacket::alarm_time(unix)).await?;
                let _ = reply.send(Ok(serde_json::json!({ "ok": true, "unix": unix })));
            }
            StudioDeviceJob::ClearAlarm { reply } => {
                self.send_command(WhoopPacket::disable_alarm()).await?;
                let _ = reply.send(Ok(serde_json::json!({ "ok": true })));
            }
            StudioDeviceJob::ToggleImuCollection { reply } => {
                self.send_command(WhoopPacket::toggle_r7_data_collection())
                    .await?;
                let _ = reply.send(Ok(serde_json::json!({ "ok": true, "note": "R7 collection toggled — expect larger history packets when enabled" })));
            }
            StudioDeviceJob::ToggleImuMode {
                enable,
                historical,
                reply,
            } => {
                let pkt = if historical {
                    WhoopPacket::toggle_imu_mode_historical(enable)
                } else {
                    WhoopPacket::toggle_imu_mode(enable)
                };
                self.send_command(pkt).await?;
                let _ = reply.send(Ok(serde_json::json!({
                    "ok": true,
                    "enable": enable,
                    "historical": historical,
                })));
            }
            StudioDeviceJob::Buzzer { reply } => {
                self.send_command(WhoopPacket::buzzer()).await?;
                let _ = reply.send(Ok(
                    serde_json::json!({ "ok": true, "note": "buzzer triggered" }),
                ));
            }
            StudioDeviceJob::StopBuzzer { reply } => {
                self.send_command(WhoopPacket::stop_buzzer()).await?;
                let _ = reply.send(Ok(
                    serde_json::json!({ "ok": true, "note": "buzzer stopped" }),
                ));
            }
        }
        Ok(())
    }

    async fn read_ble_battery_level(&self) -> Option<u8> {
        // Standard BLE Battery Level characteristic: 0x2A19
        let battery_char = Uuid::from_u128(0x00002a19_0000_1000_8000_00805f9b34fb);
        let chars = self.peripheral.characteristics();
        let ch = chars.into_iter().find(|c| c.uuid == battery_char)?;
        let bytes = self.peripheral.read(&ch).await.ok()?;
        let pct = *bytes.first()?;
        (pct <= 100).then_some(pct)
    }

    fn resolve_battery(
        cmd_percent: Option<u8>,
        gatt_percent: Option<u8>,
        raw_tail_hex: Option<String>,
    ) -> serde_json::Value {
        let (percent, source, confidence) = match (cmd_percent, gatt_percent) {
            (Some(cmd), Some(gatt)) => {
                let diff = cmd.abs_diff(gatt);
                if diff >= 10 {
                    (Some(gatt), "ble_gatt_preferred", "high")
                } else {
                    (Some(gatt), "ble_gatt+cmd26_agree", "high")
                }
            }
            (None, Some(gatt)) => (Some(gatt), "ble_gatt_only", "medium"),
            (Some(cmd), None) => (Some(cmd), "cmd26_only", "low"),
            (None, None) => (None, "none", "none"),
        };

        json!({
            "percent": percent,
            "source": source,
            "confidence": confidence,
            "cmd26_percent": cmd_percent,
            "ble_gatt_percent": gatt_percent,
            "raw_tail_hex": raw_tail_hex,
        })
    }

    async fn on_sleep(&mut self) -> anyhow::Result<bool> {
        let is_connected = self.peripheral.is_connected().await?;
        Ok(!is_connected)
    }

    pub async fn get_version(&mut self) -> anyhow::Result<()> {
        self.subscribe(CMD_FROM_STRAP).await?;

        let mut notifications = self.peripheral.notifications().await?;
        self.send_command(WhoopPacket::version()).await?;

        let timeout_duration = Duration::from_secs(5);
        match timeout(timeout_duration, notifications.next()).await {
            Ok(Some(notification)) => {
                let packet = WhoopPacket::from_data(notification.value)?;
                let data = WhoopData::from_packet(packet)?;
                if let WhoopData::VersionInfo { harvard, boylston } = data {
                    info!("version harvard {} boylston {}", harvard, boylston);
                }
                Ok(())
            }
            Ok(None) => Err(anyhow!("stream ended unexpectedly")),
            Err(_) => Err(anyhow!("timed out waiting for version notification")),
        }
    }

    pub async fn get_alarm(&mut self) -> anyhow::Result<WhoopData> {
        self.subscribe(CMD_FROM_STRAP).await?;

        let mut notifications = self.peripheral.notifications().await?;
        self.send_command(WhoopPacket::get_alarm_time()).await?;

        let timeout_duration = Duration::from_secs(30);
        match timeout(timeout_duration, notifications.next()).await {
            Ok(Some(notification)) => {
                let packet = WhoopPacket::from_data(notification.value)?;
                let data = WhoopData::from_packet(packet)?;
                Ok(data)
            }
            Ok(None) => Err(anyhow!("stream ended unexpectedly")),
            Err(_) => Err(anyhow!("timed out waiting for alarm notification")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionState, WhoopDevice};

    #[test]
    fn battery_resolver_prefers_ble_when_disagreeing() {
        let v = WhoopDevice::resolve_battery(Some(3), Some(77), Some("034d".into()));
        assert_eq!(v["percent"].as_u64(), Some(77));
        assert_eq!(v["source"].as_str(), Some("ble_gatt_preferred"));
        assert_eq!(v["confidence"].as_str(), Some("high"));
    }

    #[test]
    fn battery_resolver_falls_back_to_cmd26() {
        let v = WhoopDevice::resolve_battery(Some(66), None, Some("42".into()));
        assert_eq!(v["percent"].as_u64(), Some(66));
        assert_eq!(v["source"].as_str(), Some("cmd26_only"));
        assert_eq!(v["confidence"].as_str(), Some("low"));
    }

    #[test]
    fn lag_probe_transition_confirms_live_after_two_good_probes() {
        let (st1, streak1) =
            WhoopDevice::next_state_from_lag_probe(SessionState::CatchingUp, 0, 9_000, 15_000);
        assert_eq!(st1, SessionState::Live);
        assert_eq!(streak1, 1);
        let (st2, streak2) = WhoopDevice::next_state_from_lag_probe(st1, streak1, 4_000, 15_000);
        assert_eq!(st2, SessionState::Live);
        assert_eq!(streak2, 2);
    }

    #[test]
    fn lag_probe_transition_reverts_to_catch_up_when_lag_is_high() {
        let (state, streak) =
            WhoopDevice::next_state_from_lag_probe(SessionState::Live, 2, 16_000, 15_000);
        assert_eq!(state, SessionState::CatchingUp);
        assert_eq!(streak, 0);
    }
}
