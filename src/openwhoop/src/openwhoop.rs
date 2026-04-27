use std::sync::{Arc, Mutex};

use btleplug::api::ValueNotification;
use chrono::{DateTime, Local, TimeDelta, Utc};
use openwhoop_codec::{
    Activity, HistoryReading, WhoopData, WhoopPacket,
    constants::{CMD_FROM_STRAP, DATA_FROM_STRAP, EVENTS_FROM_STRAP, MEMFAULT, MetadataType},
};
use openwhoop_db::{DatabaseHandler, SearchHistory};
use openwhoop_entities::packets;
use tokio::sync::broadcast;

use crate::{
    algo::{
        ActivityPeriod, MAX_SLEEP_PAUSE, SkinTempCalculator, SleepCycle, SpO2Calculator,
        StressCalculator, helpers::format_hm::FormatHM,
    },
    types::activities,
};

pub struct OpenWhoop {
    pub database: DatabaseHandler,
    pub packet: Option<WhoopPacket>,
    pub last_history_packet: Option<HistoryReading>,
    pub history_packets: Vec<HistoryReading>,
    /// When set, JSON lines are pushed for the local live dashboard WebSocket.
    pub live_tx: Option<broadcast::Sender<String>>,
    /// Latest heart-rate JSON for new WebSocket subscribers (replay on connect).
    pub live_hr_snapshot: Option<Arc<Mutex<Option<String>>>>,
    /// After `HistoryComplete`, device loop should send another `history_start` so new samples keep flowing.
    history_refresh_requested: bool,
    /// Wall-clock lag probe source: latest history sample unix timestamp (ms).
    last_sample_unix_ms: Option<u64>,
    /// Wall-clock lag probe source: when we received that sample (ms).
    last_sample_received_ms: Option<i64>,
}

impl OpenWhoop {
    pub fn new(database: DatabaseHandler) -> Self {
        Self {
            database,
            packet: None,
            last_history_packet: None,
            history_packets: Vec::new(),
            live_tx: None,
            live_hr_snapshot: None,
            history_refresh_requested: false,
            last_sample_unix_ms: None,
            last_sample_received_ms: None,
        }
    }

    pub fn take_history_refresh_requested(&mut self) -> bool {
        std::mem::take(&mut self.history_refresh_requested)
    }

    pub fn with_live_broadcast(mut self, tx: broadcast::Sender<String>) -> Self {
        self.live_tx = Some(tx);
        self
    }

    pub fn with_live_hr_snapshot(mut self, snap: Arc<Mutex<Option<String>>>) -> Self {
        self.live_hr_snapshot = Some(snap);
        self
    }

    pub fn emit_live_json(&self, payload: serde_json::Value) {
        if let Some(tx) = self.live_tx.as_ref() {
            let _ = tx.send(payload.to_string());
        }
    }

    pub fn latest_lag_ms(&self, now_ms: i64) -> Option<i64> {
        self.last_sample_unix_ms
            .map(|unix| now_ms - i64::try_from(unix).unwrap_or(now_ms))
            .map(|v| v.max(0))
    }

    pub fn last_notification_age_ms(&self, now_ms: i64) -> Option<i64> {
        self.last_sample_received_ms.map(|rx| (now_ms - rx).max(0))
    }

    pub async fn store_packet(
        &self,
        notification: ValueNotification,
    ) -> anyhow::Result<packets::Model> {
        let packet = self
            .database
            .create_packet(notification.uuid, notification.value)
            .await?;

        Ok(packet)
    }

    pub async fn handle_packet(
        &mut self,
        packet: packets::Model,
    ) -> anyhow::Result<Option<WhoopPacket>> {
        let data = match packet.uuid {
            DATA_FROM_STRAP => {
                let framed = if let Some(mut whoop_packet) = self.packet.take() {
                    // TODO: maybe not needed but it would be nice to handle packet length here
                    // so if next packet contains end of one and start of another it is handled

                    whoop_packet.data.extend_from_slice(&packet.bytes);

                    if whoop_packet.data.len() + 3 >= whoop_packet.size {
                        whoop_packet
                    } else {
                        self.packet = Some(whoop_packet);
                        return Ok(None);
                    }
                } else {
                    let packet = WhoopPacket::from_data(packet.bytes)?;
                    if packet.partial {
                        self.packet = Some(packet);
                        return Ok(None);
                    }
                    packet
                };

                let ptype = framed.packet_type;
                let pcmd = framed.cmd;
                let plen = framed.data.len();
                let head: Vec<u8> = framed.data.iter().take(8).cloned().collect();
                match WhoopData::from_packet(framed) {
                    Ok(data) => data,
                    Err(_) => {
                        debug!(
                            "DATA_FROM_STRAP: WhoopData parse skipped (type={:?} cmd=0x{:02x} payload={}B head={:02x?})",
                            ptype, pcmd, plen, head
                        );
                        return Ok(None);
                    }
                }
            }
            CMD_FROM_STRAP => {
                let framed = WhoopPacket::from_data(packet.bytes)?;
                let ptype = framed.packet_type;
                let pcmd = framed.cmd;
                let plen = framed.data.len();
                let head: Vec<u8> = framed.data.iter().take(8).cloned().collect();
                match WhoopData::from_packet(framed) {
                    Ok(data) => data,
                    Err(_) => {
                        debug!(
                            "CMD_FROM_STRAP: WhoopData parse skipped (type={:?} cmd=0x{:02x} payload={}B head={:02x?})",
                            ptype, pcmd, plen, head
                        );
                        return Ok(None);
                    }
                }
            }
            EVENTS_FROM_STRAP => {
                let framed = match WhoopPacket::from_data(packet.bytes.clone()) {
                    Ok(p) => p,
                    Err(_) => {
                        trace!(
                            "EVENTS_FROM_STRAP ({} bytes, not a framed Whoop packet)",
                            packet.bytes.len()
                        );
                        return Ok(None);
                    }
                };
                match WhoopData::from_packet(framed) {
                    Ok(data) => data,
                    Err(_) => {
                        trace!(
                            "EVENTS_FROM_STRAP ({} bytes, WhoopData parse skipped)",
                            packet.bytes.len()
                        );
                        return Ok(None);
                    }
                }
            }
            MEMFAULT => {
                trace!("MEMFAULT ({} bytes)", packet.bytes.len());
                return Ok(None);
            }
            _ => {
                warn!(
                    "Unhandled GATT characteristic {} ({} bytes)",
                    packet.uuid,
                    packet.bytes.len()
                );
                return Ok(None);
            }
        };

        self.handle_data(data).await
    }

    async fn handle_data(&mut self, data: WhoopData) -> anyhow::Result<Option<WhoopPacket>> {
        match data {
            WhoopData::HistoryReading(hr) if hr.is_valid() => {
                let ptime = DateTime::from_timestamp_millis(i64::try_from(hr.unix)?)
                    .unwrap()
                    .with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M:%S");

                // Dashboard WebSocket: emit every valid sample so Pulse can plot catch-up in real time
                // and the right-hand readout updates as data arrives.
                if let Some(tx) = self.live_tx.as_ref() {
                    let received_at_ms = Utc::now().timestamp_millis();
                    let payload = serde_json::json!({
                        "type": "heart_rate",
                        "unix_ms": hr.unix,
                        "bpm": hr.bpm,
                        "time_local": format!("{ptime}"),
                        "rr_count": hr.rr.len(),
                        "imu_count": hr.imu_data.len(),
                        "skin_contact": hr.sensor_data.as_ref().map(|s| s.skin_contact),
                        "signal_quality": hr.sensor_data.as_ref().map(|s| s.signal_quality),
                        "received_at_ms": received_at_ms,
                    });
                    let line = payload.to_string();
                    if let Some(snap) = self.live_hr_snapshot.as_ref() {
                        if let Ok(mut g) = snap.lock() {
                            *g = Some(line.clone());
                        }
                    }
                    let _ = tx.send(line);
                }
                self.last_sample_unix_ms = Some(hr.unix);
                self.last_sample_received_ms = Some(Utc::now().timestamp_millis());

                if let Some(last_packet) = self.last_history_packet.as_mut() {
                    if last_packet.unix == hr.unix && last_packet.bpm == hr.bpm {
                        return Ok(None);
                    }
                    last_packet.unix = hr.unix;
                    last_packet.bpm = hr.bpm;
                } else {
                    self.last_history_packet = Some(hr.clone());
                }

                if hr.imu_data.is_empty() {
                    info!("HistoryReading time: {}", ptime);
                } else {
                    info!("HistoryReading time: {} (IMU)", ptime);
                }

                self.history_packets.push(hr);
            }
            WhoopData::HistoryReading(hr) => {
                debug!(
                    "HistoryReading skipped (validation failed) unix_ms={} bpm={}",
                    hr.unix, hr.bpm
                );
            }
            WhoopData::HistoryMetadata { data, cmd, .. } => match cmd {
                MetadataType::HistoryComplete => {
                    info!("HistoryMetadata: HistoryComplete");
                    self.history_refresh_requested = true;
                    if let Some(tx) = self.live_tx.as_ref() {
                        let _ =
                            tx.send(serde_json::json!({ "type": "history_complete" }).to_string());
                    }
                }
                MetadataType::HistoryStart => {
                    info!("HistoryMetadata: HistoryStart (batch)");
                    if let Some(tx) = self.live_tx.as_ref() {
                        let _ = tx
                            .send(serde_json::json!({ "type": "history_batch_start" }).to_string());
                    }
                }
                MetadataType::HistoryEnd => {
                    let batch = std::mem::take(&mut self.history_packets);
                    let n = batch.len();
                    self.database.create_readings(batch).await?;
                    info!(
                        "HistoryMetadata: HistoryEnd — committed {} readings to database",
                        n
                    );

                    if let Some(tx) = self.live_tx.as_ref() {
                        let _ = tx.send(
                            serde_json::json!({
                                "type": "history_batch_end",
                                "readings_committed": n,
                            })
                            .to_string(),
                        );
                    }

                    let packet = WhoopPacket::history_end(data);
                    return Ok(Some(packet));
                }
            },
            WhoopData::ConsoleLog { log, .. } => {
                trace!(target: "ConsoleLog", "{}", log);
            }
            WhoopData::AlarmInfo { enabled, unix } => {
                if let Some(tx) = self.live_tx.as_ref() {
                    let _ = tx.send(
                        serde_json::json!({
                            "type": "alarm_state",
                            "enabled": enabled,
                            "unix": unix,
                            "received_at_ms": Utc::now().timestamp_millis(),
                        })
                        .to_string(),
                    );
                }
            }
            WhoopData::VersionInfo { harvard, boylston } => {
                info!("version harvard {} boylston {}", harvard, boylston);
                if let Some(tx) = self.live_tx.as_ref() {
                    let _ = tx.send(
                        serde_json::json!({
                            "type": "version",
                            "harvard": harvard,
                            "boylston": boylston,
                        })
                        .to_string(),
                    );
                }
            }
            WhoopData::BatteryLevel {
                percent,
                raw_tail_hex,
            } => {
                if let Some(tx) = self.live_tx.as_ref() {
                    let _ = tx.send(
                        serde_json::json!({
                            "type": "battery_candidate",
                            "source": "cmd26",
                            "percent": percent,
                            "raw_tail_hex": raw_tail_hex,
                            "received_at_ms": Utc::now().timestamp_millis(),
                        })
                        .to_string(),
                    );
                }
            }
            WhoopData::RunAlarm { unix } => {
                let _ = self.database.mark_alarm_rang(i64::from(unix)).await;
                if let Some(tx) = self.live_tx.as_ref() {
                    let _ = tx.send(
                        serde_json::json!({
                            "type": "strap_alarm_fired",
                            "unix": unix,
                            "received_at_ms": Utc::now().timestamp_millis(),
                        })
                        .to_string(),
                    );
                }
            }
            WhoopData::Event { unix, event } => {
                if let Some(tx) = self.live_tx.as_ref() {
                    let _ = tx.send(
                        serde_json::json!({
                            "type": "device_event",
                            "unix": unix,
                            "command": format!("{:?}", event),
                            "command_code": event.as_u8(),
                            "received_at_ms": Utc::now().timestamp_millis(),
                        })
                        .to_string(),
                    );
                }
            }
            WhoopData::UnknownEvent { unix, event } => {
                if let Some(tx) = self.live_tx.as_ref() {
                    let _ = tx.send(
                        serde_json::json!({
                            "type": "strap_unknown_event",
                            "unix": unix,
                            "event_code": event,
                            "received_at_ms": Utc::now().timestamp_millis(),
                        })
                        .to_string(),
                    );
                }
            }
        }

        Ok(None)
    }

    pub async fn get_latest_sleep(&self) -> anyhow::Result<Option<SleepCycle>> {
        Ok(self.database.get_latest_sleep().await?.map(map_sleep_cycle))
    }

    pub async fn detect_events(&self) -> anyhow::Result<()> {
        let latest_activity = self.database.get_latest_activity().await?;
        let start_from = latest_activity.map(|a| a.from);

        let sleeps = self
            .database
            .get_sleep_cycles(start_from)
            .await?
            .windows(2)
            .map(|sleep| (sleep[0].id, sleep[0].end, sleep[1].start))
            .collect::<Vec<_>>();

        for (cycle_id, start, end) in sleeps {
            let options = SearchHistory {
                from: Some(start),
                to: Some(end),
                ..Default::default()
            };

            let history = self.database.search_history(options).await?;
            let events = ActivityPeriod::detect_from_gravity(&history);

            for event in events {
                let activity = match event.activity {
                    Activity::Active => activities::ActivityType::Activity,
                    Activity::Sleep => activities::ActivityType::Nap,
                    _ => continue,
                };

                let activity = activities::ActivityPeriod {
                    period_id: cycle_id,
                    from: event.start,
                    to: event.end,
                    activity,
                };

                let duration = activity.to - activity.from;
                info!(
                    "Detected activity period from: {} to: {}, duration: {}",
                    activity.from,
                    activity.to,
                    duration.format_hm()
                );
                self.database.create_activity(activity).await?;
            }
        }

        Ok(())
    }

    /// TODO: add handling for data splits
    pub async fn detect_sleeps(&self) -> anyhow::Result<()> {
        'a: loop {
            let last_sleep = self.get_latest_sleep().await?;

            let options = SearchHistory {
                from: last_sleep.map(|s| s.end),
                limit: Some(86400 * 2),
                ..Default::default()
            };

            let mut history = self.database.search_history(options).await?;
            let mut periods = ActivityPeriod::detect_from_gravity(&history);

            while let Some(mut sleep) = ActivityPeriod::find_sleep(&mut periods) {
                if let Some(last_sleep) = last_sleep {
                    let diff = sleep.start - last_sleep.end;

                    if diff < MAX_SLEEP_PAUSE {
                        history = self
                            .database
                            .search_history(SearchHistory {
                                from: Some(last_sleep.start),
                                to: Some(sleep.end),
                                ..Default::default()
                            })
                            .await?;

                        sleep.start = last_sleep.start;
                        sleep.duration = sleep.end - sleep.start;
                    } else {
                        let this_sleep_id = sleep.end.date();
                        let last_sleep_id = last_sleep.end.date();

                        if this_sleep_id == last_sleep_id {
                            if sleep.duration < last_sleep.duration() {
                                let nap = activities::ActivityPeriod {
                                    period_id: last_sleep.id,
                                    from: sleep.start,
                                    to: sleep.end,
                                    activity: activities::ActivityType::Nap,
                                };
                                self.database.create_activity(nap).await?;
                                continue;
                            } else {
                                let nap = activities::ActivityPeriod {
                                    period_id: last_sleep.id - TimeDelta::days(1),
                                    from: last_sleep.start,
                                    to: last_sleep.end,
                                    activity: activities::ActivityType::Nap,
                                };
                                self.database.create_activity(nap).await?;
                            }
                        }
                    }
                }

                let sleep_cycle = SleepCycle::from_event(sleep, &history)?;

                info!(
                    "Detected sleep from {} to {}, duration: {}",
                    sleep.start,
                    sleep.end,
                    sleep.duration.format_hm()
                );
                self.database.create_sleep(sleep_cycle).await?;
                continue 'a;
            }

            break;
        }

        Ok(())
    }

    pub async fn calculate_spo2(&self) -> anyhow::Result<()> {
        loop {
            let last = self.database.last_spo2_time().await?;
            let options = SearchHistory {
                from: last.map(|t| {
                    t - TimeDelta::seconds(i64::try_from(SpO2Calculator::WINDOW_SIZE).unwrap_or(0))
                }),
                to: None,
                limit: Some(86400),
            };

            let readings = self.database.search_sensor_readings(options).await?;
            if readings.is_empty() || readings.len() <= SpO2Calculator::WINDOW_SIZE {
                break;
            }

            let scores = readings
                .windows(SpO2Calculator::WINDOW_SIZE)
                .filter_map(SpO2Calculator::calculate);

            for score in scores {
                self.database.update_spo2_on_reading(score).await?;
            }
        }

        Ok(())
    }

    pub async fn calculate_skin_temp(&self) -> anyhow::Result<()> {
        loop {
            let readings = self
                .database
                .search_temp_readings(SearchHistory {
                    limit: Some(86400),
                    ..Default::default()
                })
                .await?;

            if readings.is_empty() {
                break;
            }

            for reading in &readings {
                if let Some(score) =
                    SkinTempCalculator::convert(reading.time, reading.skin_temp_raw)
                {
                    self.database.update_skin_temp_on_reading(score).await?;
                }
            }
        }

        Ok(())
    }

    pub async fn calculate_stress(&self) -> anyhow::Result<()> {
        loop {
            let last_stress = self.database.last_stress_time().await?;
            let options = SearchHistory {
                from: last_stress.map(|t| {
                    t - TimeDelta::seconds(
                        i64::try_from(StressCalculator::MIN_READING_PERIOD).unwrap_or(0),
                    )
                }),
                to: None,
                limit: Some(86400),
            };

            let history = self.database.search_history(options).await?;
            if history.is_empty() || history.len() <= StressCalculator::MIN_READING_PERIOD {
                break;
            }

            let stress_scores = history
                .windows(StressCalculator::MIN_READING_PERIOD)
                .filter_map(StressCalculator::calculate_stress);

            for stress in stress_scores {
                self.database.update_stress_on_reading(stress).await?;
            }
        }

        Ok(())
    }
}

fn map_sleep_cycle(sleep: openwhoop_entities::sleep_cycles::Model) -> SleepCycle {
    SleepCycle {
        id: sleep.end.date(),
        start: sleep.start,
        end: sleep.end,
        min_bpm: sleep.min_bpm.try_into().unwrap(),
        max_bpm: sleep.max_bpm.try_into().unwrap(),
        avg_bpm: sleep.avg_bpm.try_into().unwrap(),
        min_hrv: sleep.min_hrv.try_into().unwrap(),
        max_hrv: sleep.max_hrv.try_into().unwrap(),
        avg_hrv: sleep.avg_hrv.try_into().unwrap(),
        score: sleep
            .score
            .unwrap_or_else(|| SleepCycle::sleep_score(sleep.start, sleep.end)),
    }
}
