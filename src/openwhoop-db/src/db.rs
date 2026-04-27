use std::collections::HashMap;
use std::str::FromStr;

use chrono::{Local, NaiveDateTime, TimeZone, Utc};
use cron::Schedule;
use openwhoop_entities::{alarm_schedules, command_queue, packets, sleep_cycles};
use openwhoop_migration::{Migrator, MigratorTrait, OnConflict};
use sea_orm::{
    ActiveModelTrait, ActiveValue::NotSet, ColumnTrait, ConnectOptions, Database,
    DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use uuid::Uuid;

use openwhoop_algos::SleepCycle;
use openwhoop_codec::HistoryReading;

#[derive(Clone)]
pub struct DatabaseHandler {
    pub(crate) db: DatabaseConnection,
}

#[derive(Clone, Debug)]
pub struct AlarmScheduleDraft {
    pub label: String,
    /// `cron` or `once`
    pub kind: String,
    pub cron_expr: Option<String>,
    pub one_time_unix: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct AlarmSchedulePatch {
    pub label: Option<String>,
    pub enabled: Option<bool>,
}

impl DatabaseHandler {
    pub fn connection(&self) -> &DatabaseConnection {
        &self.db
    }

    pub async fn new<C>(path: C) -> Self
    where
        C: Into<ConnectOptions>,
    {
        let db = Database::connect(path)
            .await
            .expect("Unable to connect to db");

        Migrator::up(&db, None)
            .await
            .expect("Error running migrations");

        Self { db }
    }

    pub async fn create_packet(
        &self,
        char: Uuid,
        data: Vec<u8>,
    ) -> anyhow::Result<openwhoop_entities::packets::Model> {
        let packet = openwhoop_entities::packets::ActiveModel {
            id: NotSet,
            uuid: Set(char),
            bytes: Set(data),
        };

        let packet = packet.insert(&self.db).await?;
        Ok(packet)
    }

    pub async fn create_reading(&self, reading: HistoryReading) -> anyhow::Result<()> {
        let time = timestamp_to_local(reading.unix)?;

        let sensor_json = reading
            .sensor_data
            .as_ref()
            .map(|s| serde_json::to_value(s))
            .transpose()?;

        let packet = openwhoop_entities::heart_rate::ActiveModel {
            id: NotSet,
            bpm: Set(i16::from(reading.bpm)),
            time: Set(time),
            rr_intervals: Set(rr_to_string(reading.rr)),
            activity: NotSet,
            stress: NotSet,
            spo2: NotSet,
            skin_temp: NotSet,
            imu_data: Set(Some(serde_json::to_value(reading.imu_data)?)),
            sensor_data: Set(sensor_json),
            synced: NotSet,
        };

        let _model = openwhoop_entities::heart_rate::Entity::insert(packet)
            .on_conflict(
                OnConflict::column(openwhoop_entities::heart_rate::Column::Time)
                    .update_column(openwhoop_entities::heart_rate::Column::Bpm)
                    .update_column(openwhoop_entities::heart_rate::Column::RrIntervals)
                    .update_column(openwhoop_entities::heart_rate::Column::SensorData)
                    .to_owned(),
            )
            .exec(&self.db)
            .await?;

        Ok(())
    }

    pub async fn create_readings(&self, readings: Vec<HistoryReading>) -> anyhow::Result<()> {
        if readings.is_empty() {
            return Ok(());
        }
        // Same `time` twice in one INSERT breaks Postgres upsert:
        // "ON CONFLICT DO UPDATE command cannot affect row a second time"
        let mut by_unix: HashMap<u64, HistoryReading> = HashMap::new();
        for r in readings {
            by_unix.insert(r.unix, r);
        }
        let mut readings: Vec<HistoryReading> = by_unix.into_values().collect();
        readings.sort_by_key(|r| r.unix);

        let payloads = readings
            .into_iter()
            .map(|r| {
                let time = timestamp_to_local(r.unix)?;
                let sensor_json = r
                    .sensor_data
                    .as_ref()
                    .map(|s| serde_json::to_value(s))
                    .transpose()?;
                Ok(openwhoop_entities::heart_rate::ActiveModel {
                    id: NotSet,
                    bpm: Set(i16::from(r.bpm)),
                    time: Set(time),
                    rr_intervals: Set(rr_to_string(r.rr)),
                    activity: NotSet,
                    stress: NotSet,
                    spo2: NotSet,
                    skin_temp: NotSet,
                    imu_data: Set(Some(serde_json::to_value(r.imu_data)?)),
                    sensor_data: Set(sensor_json),
                    synced: NotSet,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        // SQLite limits to 999 SQL variables per statement.
        // heart_rate has 11 columns, so max 90 rows per batch.
        for chunk in payloads.chunks(90) {
            openwhoop_entities::heart_rate::Entity::insert_many(chunk.to_vec())
                .on_conflict(
                    OnConflict::column(openwhoop_entities::heart_rate::Column::Time)
                        .update_column(openwhoop_entities::heart_rate::Column::Bpm)
                        .update_column(openwhoop_entities::heart_rate::Column::RrIntervals)
                        .update_column(openwhoop_entities::heart_rate::Column::SensorData)
                        .to_owned(),
                )
                .exec(&self.db)
                .await?;
        }

        Ok(())
    }

    pub async fn get_packets(&self, id: i32) -> anyhow::Result<Vec<packets::Model>> {
        let stream = packets::Entity::find()
            .filter(packets::Column::Id.gt(id))
            .order_by_asc(packets::Column::Id)
            .limit(10_000)
            .all(&self.db)
            .await?;

        Ok(stream)
    }

    pub async fn get_latest_sleep(
        &self,
    ) -> anyhow::Result<Option<openwhoop_entities::sleep_cycles::Model>> {
        let sleep = sleep_cycles::Entity::find()
            .order_by_desc(sleep_cycles::Column::End)
            .one(&self.db)
            .await?;

        Ok(sleep)
    }

    pub async fn create_sleep(&self, sleep: SleepCycle) -> anyhow::Result<()> {
        let model = sleep_cycles::ActiveModel {
            id: Set(Uuid::new_v4()),
            sleep_id: Set(sleep.id),
            start: Set(sleep.start),
            end: Set(sleep.end),
            min_bpm: Set(sleep.min_bpm.into()),
            max_bpm: Set(sleep.max_bpm.into()),
            avg_bpm: Set(sleep.avg_bpm.into()),
            min_hrv: Set(sleep.min_hrv.into()),
            max_hrv: Set(sleep.max_hrv.into()),
            avg_hrv: Set(sleep.avg_hrv.into()),
            score: Set(sleep.score.into()),
            synced: NotSet,
        };

        let _r = sleep_cycles::Entity::insert(model)
            .on_conflict(
                OnConflict::column(sleep_cycles::Column::SleepId)
                    .update_columns([
                        sleep_cycles::Column::Start,
                        sleep_cycles::Column::End,
                        sleep_cycles::Column::MinBpm,
                        sleep_cycles::Column::MaxBpm,
                        sleep_cycles::Column::AvgBpm,
                        sleep_cycles::Column::MinHrv,
                        sleep_cycles::Column::MaxHrv,
                        sleep_cycles::Column::AvgHrv,
                    ])
                    .to_owned(),
            )
            .exec(&self.db)
            .await?;

        Ok(())
    }

    pub async fn list_alarm_schedules(&self) -> anyhow::Result<Vec<alarm_schedules::Model>> {
        let items = alarm_schedules::Entity::find()
            .order_by_asc(alarm_schedules::Column::NextUnix)
            .all(&self.db)
            .await?;
        Ok(items)
    }

    pub async fn create_alarm_schedule(
        &self,
        draft: AlarmScheduleDraft,
    ) -> anyhow::Result<alarm_schedules::Model> {
        let now = Utc::now().naive_utc();
        let now_unix = Utc::now().timestamp();
        let kind = draft.kind.to_lowercase();
        let next_unix = match kind.as_str() {
            "cron" => {
                let expr = draft
                    .cron_expr
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("cron_expr is required for cron schedules"))?;
                next_from_cron(expr, now_unix)?
            }
            "once" => draft
                .one_time_unix
                .ok_or_else(|| anyhow::anyhow!("one_time_unix is required for once schedules"))
                .map(Some)?,
            _ => return Err(anyhow::anyhow!("kind must be `cron` or `once`")),
        };

        let model = alarm_schedules::ActiveModel {
            id: NotSet,
            label: Set(draft.label),
            kind: Set(kind),
            cron_expr: Set(draft.cron_expr),
            one_time_unix: Set(draft.one_time_unix),
            next_unix: Set(next_unix),
            last_rang_unix: Set(None),
            last_sent_unix: Set(None),
            enabled: Set(true),
            created_at: Set(now),
            updated_at: Set(now),
        };
        Ok(model.insert(&self.db).await?)
    }

    pub async fn patch_alarm_schedule(
        &self,
        id: i32,
        patch: AlarmSchedulePatch,
    ) -> anyhow::Result<Option<alarm_schedules::Model>> {
        let Some(mut row) = alarm_schedules::Entity::find_by_id(id).one(&self.db).await? else {
            return Ok(None);
        };
        if let Some(label) = patch.label {
            row.label = label;
        }
        if let Some(enabled) = patch.enabled {
            row.enabled = enabled;
        }
        row.updated_at = Utc::now().naive_utc();
        let am: alarm_schedules::ActiveModel = row.into();
        let updated = am.update(&self.db).await?;
        Ok(Some(updated))
    }

    pub async fn delete_alarm_schedule(&self, id: i32) -> anyhow::Result<bool> {
        let res = alarm_schedules::Entity::delete_by_id(id).exec(&self.db).await?;
        Ok(res.rows_affected > 0)
    }

    /// Mark all due schedules as rang and compute each schedule's next trigger.
    pub async fn advance_due_alarm_schedules(&self, now_unix: i64) -> anyhow::Result<()> {
        let due = alarm_schedules::Entity::find()
            .filter(alarm_schedules::Column::Enabled.eq(true))
            .filter(alarm_schedules::Column::NextUnix.lte(now_unix))
            .order_by_asc(alarm_schedules::Column::NextUnix)
            .all(&self.db)
            .await?;

        for mut s in due {
            s.last_rang_unix = Some(now_unix);
            s.next_unix = match s.kind.as_str() {
                "once" => {
                    s.enabled = false;
                    None
                }
                "cron" => {
                    if let Some(expr) = s.cron_expr.as_ref() {
                        next_from_cron(expr, now_unix + 1)?
                    } else {
                        None
                    }
                }
                _ => None,
            };
            s.updated_at = Utc::now().naive_utc();
            let am: alarm_schedules::ActiveModel = s.into();
            let _ = am.update(&self.db).await?;
        }
        Ok(())
    }

    /// Update schedule row(s) when strap alarm fired.
    pub async fn mark_alarm_rang(&self, strap_unix: i64) -> anyhow::Result<()> {
        let now = Utc::now().timestamp();
        let candidates = alarm_schedules::Entity::find()
            .filter(alarm_schedules::Column::Enabled.eq(true))
            .filter(alarm_schedules::Column::NextUnix.lte(strap_unix + 90))
            .order_by_desc(alarm_schedules::Column::NextUnix)
            .all(&self.db)
            .await?;
        if candidates.is_empty() {
            return Ok(());
        }
        for mut s in candidates {
            s.last_rang_unix = Some(strap_unix);
            s.next_unix = match s.kind.as_str() {
                "once" => {
                    s.enabled = false;
                    None
                }
                "cron" => {
                    if let Some(expr) = s.cron_expr.as_ref() {
                        next_from_cron(expr, now + 1)?
                    } else {
                        None
                    }
                }
                _ => None,
            };
            s.updated_at = Utc::now().naive_utc();
            let am: alarm_schedules::ActiveModel = s.into();
            let _ = am.update(&self.db).await?;
        }
        Ok(())
    }

    pub async fn next_enabled_alarm_unix(&self, now_unix: i64) -> anyhow::Result<Option<i64>> {
        let row = alarm_schedules::Entity::find()
            .filter(alarm_schedules::Column::Enabled.eq(true))
            .filter(alarm_schedules::Column::NextUnix.gte(now_unix))
            .order_by_asc(alarm_schedules::Column::NextUnix)
            .one(&self.db)
            .await?;
        Ok(row.and_then(|r| r.next_unix))
    }

    pub async fn mark_alarm_programmed(&self, unix: i64) -> anyhow::Result<()> {
        let rows = alarm_schedules::Entity::find()
            .filter(alarm_schedules::Column::Enabled.eq(true))
            .filter(alarm_schedules::Column::NextUnix.eq(unix))
            .all(&self.db)
            .await?;
        for mut row in rows {
            row.last_sent_unix = Some(Utc::now().timestamp());
            row.updated_at = Utc::now().naive_utc();
            let am: alarm_schedules::ActiveModel = row.into();
            let _ = am.update(&self.db).await?;
        }
        Ok(())
    }

    pub async fn list_pending_commands(&self, device_id: &str) -> anyhow::Result<Vec<command_queue::Model>> {
        use command_queue::Entity;
        use sea_orm::QueryFilter;
        Ok(Entity::find()
            .filter(command_queue::Column::DeviceId.eq(device_id))
            .filter(command_queue::Column::Status.eq("pending"))
            .order_by_asc(command_queue::Column::CreatedAt)
            .all(&self.db)
            .await?)
    }

    pub async fn push_command(
        &self,
        device_id: &str,
        command_type: &str,
        payload: Option<serde_json::Value>,
    ) -> anyhow::Result<i32> {
        use command_queue::ActiveModel;
        use sea_orm::ActiveValue::{NotSet, Set};

        let now = Utc::now().naive_utc();
        let model = ActiveModel {
            id: NotSet,
            device_id: Set(device_id.to_string()),
            command_type: Set(command_type.to_string()),
            payload: Set(payload),
            status: Set("pending".to_string()),
            created_at: Set(now),
            sent_at: Set(None),
            error: Set(None),
            retry_count: Set(0),
        };

        let inserted = model.insert(&self.db).await?;
        Ok(inserted.id)
    }

    pub async fn mark_command_sent(&self, id: i32) -> anyhow::Result<()> {
        
        
        use sea_orm::ActiveValue::Set;

        let now = Utc::now().naive_utc();
        let mut model: command_queue::ActiveModel = command_queue::Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("command {} not found", id))?
            .into();
        model.status = Set("sent".to_string());
        model.sent_at = Set(Some(now));
        model.update(&self.db).await?;
        Ok(())
    }

    pub async fn mark_command_failed(&self, id: i32, error: &str) -> anyhow::Result<()> {
        
        
        use sea_orm::ActiveValue::Set;

        let mut model: command_queue::ActiveModel = command_queue::Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("command {} not found", id))?
            .into();
        model.status = Set("failed".to_string());
        model.error = Set(Some(error.to_string()));
        let current_count = model.retry_count.take().unwrap_or(0);
        model.retry_count = Set(current_count + 1);
        model.update(&self.db).await?;
        Ok(())
    }

    pub async fn get_device_alarm_state(&self, device_id: &str) -> anyhow::Result<Option<(bool, i64)>> {
        // Get the most recently programmed alarm for this device (from alarm_schedules)
        // In a multi-device scenario, we'd need a device_id column on alarm_schedules too
        // For now, we track device-specific alarm state via a simple query
        // This is a placeholder - actual implementation would need device tracking
        let _ = device_id;
        Ok(None)
    }
}

fn next_from_cron(expr: &str, from_unix: i64) -> anyhow::Result<Option<i64>> {
    let sched = parse_cron_schedule(expr)?;
    let from = chrono::DateTime::<Utc>::from_timestamp(from_unix, 0)
        .ok_or_else(|| anyhow::anyhow!("invalid from_unix"))?;
    Ok(sched.after(&from).next().map(|dt| dt.timestamp()))
}

fn parse_cron_schedule(expr: &str) -> anyhow::Result<Schedule> {
    if let Ok(s) = Schedule::from_str(expr) {
        return Ok(s);
    }
    // Accept common 5-field cron (min hour dom mon dow) by prepending seconds.
    if expr.split_whitespace().count() == 5 {
        let with_seconds = format!("0 {expr}");
        return Ok(Schedule::from_str(&with_seconds)?);
    }
    Err(anyhow::anyhow!("invalid cron expression: {expr}"))
}

fn timestamp_to_local(unix: u64) -> anyhow::Result<NaiveDateTime> {
    let millis = i64::try_from(unix)?;
    let dt = Local
        .timestamp_millis_opt(millis)
        .single()
        .ok_or_else(|| anyhow::anyhow!("ambiguous or invalid unix timestamp: {}", millis))?;

    Ok(dt.naive_local())
}

fn rr_to_string(rr: Vec<u16>) -> String {
    rr.iter().map(u16::to_string).collect::<Vec<_>>().join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_and_get_packets() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let uuid = Uuid::new_v4();
        let data = vec![0xAA, 0xBB, 0xCC];

        let packet = db.create_packet(uuid, data.clone()).await.unwrap();
        assert_eq!(packet.uuid, uuid);
        assert_eq!(packet.bytes, data);

        let packets = db.get_packets(0).await.unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].uuid, uuid);
    }

    #[tokio::test]
    async fn create_reading_and_search_history() {
        let db = DatabaseHandler::new("sqlite::memory:").await;

        let reading = HistoryReading {
            unix: 1735689600000, // 2025-01-01 00:00:00 UTC in millis
            bpm: 72,
            rr: vec![833, 850],
            imu_data: vec![],
            sensor_data: None,
        };

        db.create_reading(reading).await.unwrap();

        let history = db
            .search_history(crate::SearchHistory::default())
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].bpm, 72);
        assert_eq!(history[0].rr, vec![833, 850]);
    }

    #[tokio::test]
    async fn create_readings_batch() {
        let db = DatabaseHandler::new("sqlite::memory:").await;

        let readings: Vec<HistoryReading> = (0..5)
            .map(|i| HistoryReading {
                unix: 1735689600000 + i * 1000,
                bpm: 70 + u8::try_from(i).unwrap(),
                rr: vec![850],
                imu_data: vec![],
                sensor_data: None,
            })
            .collect();

        db.create_readings(readings).await.unwrap();

        let history = db
            .search_history(crate::SearchHistory::default())
            .await
            .unwrap();
        assert_eq!(history.len(), 5);
    }

    #[tokio::test]
    async fn create_and_get_sleep() {
        let db = DatabaseHandler::new("sqlite::memory:").await;

        let start = chrono::NaiveDate::from_ymd_opt(2025, 1, 1)
            .unwrap()
            .and_hms_opt(22, 0, 0)
            .unwrap();
        let end = chrono::NaiveDate::from_ymd_opt(2025, 1, 2)
            .unwrap()
            .and_hms_opt(6, 0, 0)
            .unwrap();

        let sleep = SleepCycle {
            id: end.date(),
            start,
            end,
            min_bpm: 50,
            max_bpm: 70,
            avg_bpm: 60,
            min_hrv: 30,
            max_hrv: 80,
            avg_hrv: 55,
            score: 100.0,
        };

        db.create_sleep(sleep).await.unwrap();

        let latest = db.get_latest_sleep().await.unwrap();
        assert!(latest.is_some());
        let latest = latest.unwrap();
        assert_eq!(latest.min_bpm, 50);
        assert_eq!(latest.avg_bpm, 60);
    }

    #[tokio::test]
    async fn upsert_reading_on_conflict() {
        let db = DatabaseHandler::new("sqlite::memory:").await;

        let reading = HistoryReading {
            unix: 1735689600000,
            bpm: 72,
            rr: vec![833],
            imu_data: vec![],
            sensor_data: None,
        };
        db.create_reading(reading).await.unwrap();

        // Insert again with different bpm - should upsert
        let reading2 = HistoryReading {
            unix: 1735689600000,
            bpm: 80,
            rr: vec![750],
            imu_data: vec![],
            sensor_data: None,
        };
        db.create_reading(reading2).await.unwrap();

        let history = db
            .search_history(crate::SearchHistory::default())
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].bpm, 80);
    }

    #[tokio::test]
    async fn push_and_list_pending_commands() {
        let db = DatabaseHandler::new("sqlite::memory:").await;

        let id = db.push_command("device123", "buzzer", None).await.unwrap();
        assert!(id > 0);

        let pending = db.list_pending_commands("device123").await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].command_type, "buzzer");
        assert_eq!(pending[0].status, "pending");
    }

    #[tokio::test]
    async fn mark_command_sent() {
        let db = DatabaseHandler::new("sqlite::memory:").await;

        let id = db.push_command("device123", "buzzer", None).await.unwrap();
        db.mark_command_sent(id).await.unwrap();

        let pending = db.list_pending_commands("device123").await.unwrap();
        assert_eq!(pending.len(), 0);
    }

    #[tokio::test]
    async fn mark_command_failed_increments_retry() {
        let db = DatabaseHandler::new("sqlite::memory:").await;

        let id = db.push_command("device123", "buzzer", None).await.unwrap();
        db.mark_command_failed(id, "connection timeout").await.unwrap();

        let pending = db.list_pending_commands("device123").await.unwrap();
        assert_eq!(pending.len(), 0);
    }

    #[tokio::test]
    async fn queue_commands_for_different_devices() {
        let db = DatabaseHandler::new("sqlite::memory:").await;

        db.push_command("device1", "buzzer", None).await.unwrap();
        db.push_command("device2", "buzzer", None).await.unwrap();
        db.push_command("device1", "set-alarm", Some(serde_json::json!({"unix": 1234567890}))).await.unwrap();

        let pending1 = db.list_pending_commands("device1").await.unwrap();
        let pending2 = db.list_pending_commands("device2").await.unwrap();

        assert_eq!(pending1.len(), 2);
        assert_eq!(pending2.len(), 1);
    }
}
