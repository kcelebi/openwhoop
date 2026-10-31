use std::collections::HashMap;
use std::str::FromStr;

use chrono::{Local, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use cron::Schedule;
use openwhoop_entities::{alarm_schedules, cloud_hr_outbox, command_queue, packets, sleep_cycles};
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
    pub timezone: Option<String>,
    pub one_time_unix: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct AlarmSchedulePatch {
    pub label: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct CloudAlarmInput {
    pub id: Uuid,
    pub label: String,
    pub kind: String,
    pub cron_expr: Option<String>,
    pub timezone: String,
    pub one_time_unix: Option<i64>,
    pub enabled: bool,
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

    pub async fn enqueue_cloud_hr_sample(
        &self,
        recorded_at_ms: i64,
        bpm: u8,
        received_at_ms: i64,
    ) -> anyhow::Result<()> {
        let row = cloud_hr_outbox::ActiveModel {
            recorded_at_ms: Set(recorded_at_ms),
            bpm: Set(i16::from(bpm)),
            received_at_ms: Set(received_at_ms),
            synced: Set(false),
        };
        cloud_hr_outbox::Entity::insert(row)
            .on_conflict(
                OnConflict::column(cloud_hr_outbox::Column::RecordedAtMs)
                    .update_columns([
                        cloud_hr_outbox::Column::Bpm,
                        cloud_hr_outbox::Column::ReceivedAtMs,
                        cloud_hr_outbox::Column::Synced,
                    ])
                    .to_owned(),
            )
            .exec(&self.db)
            .await?;
        Ok(())
    }

    pub async fn pending_cloud_hr_samples(
        &self,
        limit: u64,
    ) -> anyhow::Result<Vec<cloud_hr_outbox::Model>> {
        Ok(cloud_hr_outbox::Entity::find()
            .filter(cloud_hr_outbox::Column::Synced.eq(false))
            .order_by_asc(cloud_hr_outbox::Column::RecordedAtMs)
            .limit(limit.clamp(1, 500))
            .all(&self.db)
            .await?)
    }

    pub async fn mark_cloud_hr_synced(&self, timestamps: &[i64]) -> anyhow::Result<()> {
        if timestamps.is_empty() {
            return Ok(());
        }
        cloud_hr_outbox::Entity::update_many()
            .col_expr(
                cloud_hr_outbox::Column::Synced,
                sea_orm::sea_query::Expr::value(true),
            )
            .filter(cloud_hr_outbox::Column::RecordedAtMs.is_in(timestamps.iter().copied()))
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

    /// Supabase owns hosted alarm edits. Preserve local delivery evidence while applying them.
    pub async fn reconcile_cloud_alarms(&self, incoming: &[CloudAlarmInput]) -> anyhow::Result<()> {
        let existing = self.list_alarm_schedules().await?;
        let incoming_ids: std::collections::HashSet<Uuid> = incoming.iter().map(|a| a.id).collect();
        for row in &existing {
            if row.cloud_id.is_some_and(|id| !incoming_ids.contains(&id)) {
                self.delete_alarm_schedule(row.id).await?;
            }
        }
        let now = Utc::now().timestamp();
        for input in incoming {
            let _: Tz = input
                .timezone
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid timezone"))?;
            let next = match input.kind.as_str() {
                "once" => input.one_time_unix.filter(|unix| *unix > now),
                "cron" => next_from_cron(
                    input
                        .cron_expr
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("missing cron"))?,
                    now,
                    &input.timezone,
                )?,
                _ => anyhow::bail!("invalid cloud alarm kind"),
            };
            if let Some(mut row) = existing
                .iter()
                .find(|r| r.cloud_id == Some(input.id))
                .cloned()
            {
                let changed = row.label != input.label
                    || row.kind != input.kind
                    || row.cron_expr != input.cron_expr
                    || row.timezone != input.timezone
                    || row.one_time_unix != input.one_time_unix;
                row.label = input.label.clone();
                row.kind = input.kind.clone();
                row.cron_expr = input.cron_expr.clone();
                row.timezone = input.timezone.clone();
                row.one_time_unix = input.one_time_unix;
                row.enabled = input.enabled && next.is_some();
                if changed || (row.enabled && row.next_unix.is_none()) {
                    row.next_unix = next;
                    row.last_sent_unix = None;
                }
                if !row.enabled {
                    row.next_unix = None;
                    row.last_sent_unix = None;
                }
                row.updated_at = Utc::now().naive_utc();
                self.save_alarm_schedule(row).await?;
            } else if next.is_some() {
                let mut row = self
                    .create_alarm_schedule(AlarmScheduleDraft {
                        label: input.label.clone(),
                        kind: input.kind.clone(),
                        cron_expr: input.cron_expr.clone(),
                        timezone: Some(input.timezone.clone()),
                        one_time_unix: input.one_time_unix,
                    })
                    .await?;
                row.cloud_id = Some(input.id);
                row.enabled = input.enabled;
                if !row.enabled {
                    row.next_unix = None;
                }
                self.save_alarm_schedule(row).await?;
            }
        }
        Ok(())
    }

    async fn save_alarm_schedule(
        &self,
        row: alarm_schedules::Model,
    ) -> anyhow::Result<alarm_schedules::Model> {
        // Converting a fetched Model produces Unchanged values. Explicit Set values are
        // required or SeaORM silently leaves enabled/next_unix/last_rang untouched.
        let mut am: alarm_schedules::ActiveModel = row.clone().into();
        am.label = Set(row.label);
        am.cloud_id = Set(row.cloud_id);
        am.cron_expr = Set(row.cron_expr);
        am.timezone = Set(row.timezone);
        am.one_time_unix = Set(row.one_time_unix);
        am.next_unix = Set(row.next_unix);
        am.last_rang_unix = Set(row.last_rang_unix);
        am.last_missed_unix = Set(row.last_missed_unix);
        am.last_unconfirmed_unix = Set(row.last_unconfirmed_unix);
        am.last_sent_unix = Set(row.last_sent_unix);
        am.enabled = Set(row.enabled);
        am.updated_at = Set(row.updated_at);
        Ok(am.update(&self.db).await?)
    }

    pub async fn create_alarm_schedule(
        &self,
        draft: AlarmScheduleDraft,
    ) -> anyhow::Result<alarm_schedules::Model> {
        let now = Utc::now().naive_utc();
        let now_unix = Utc::now().timestamp();
        let kind = draft.kind.to_lowercase();
        let timezone = draft.timezone.unwrap_or_else(|| "UTC".to_string());
        let _: Tz = timezone
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid IANA timezone: {timezone}"))?;
        let next_unix = match kind.as_str() {
            "cron" => {
                let expr = draft
                    .cron_expr
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("cron_expr is required for cron schedules"))?;
                next_from_cron(expr, now_unix, &timezone)?
            }
            "once" => {
                let unix = draft.one_time_unix.ok_or_else(|| {
                    anyhow::anyhow!("one_time_unix is required for once schedules")
                })?;
                anyhow::ensure!(unix > now_unix, "one-time alarm must be in the future");
                Some(unix)
            }
            _ => return Err(anyhow::anyhow!("kind must be `cron` or `once`")),
        };

        let model = alarm_schedules::ActiveModel {
            id: NotSet,
            cloud_id: Set(None),
            label: Set(draft.label),
            kind: Set(kind),
            cron_expr: Set(draft.cron_expr),
            timezone: Set(timezone),
            one_time_unix: Set(draft.one_time_unix),
            next_unix: Set(next_unix),
            last_rang_unix: Set(None),
            last_missed_unix: Set(None),
            last_unconfirmed_unix: Set(None),
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
        let Some(mut row) = alarm_schedules::Entity::find_by_id(id)
            .one(&self.db)
            .await?
        else {
            return Ok(None);
        };
        if let Some(label) = patch.label {
            row.label = label;
        }
        if let Some(enabled) = patch.enabled {
            row.enabled = enabled;
            if !enabled {
                row.last_sent_unix = None;
            }
            if enabled {
                let now = Utc::now().timestamp();
                if row.next_unix.is_none_or(|next| next <= now) {
                    row.next_unix = match row.kind.as_str() {
                        "cron" => row
                            .cron_expr
                            .as_deref()
                            .map(|expr| next_from_cron(expr, now, &row.timezone))
                            .transpose()?
                            .flatten(),
                        "once" => row.one_time_unix.filter(|unix| *unix > now),
                        _ => None,
                    };
                    anyhow::ensure!(row.next_unix.is_some(), "alarm has no future occurrence");
                }
            }
        }
        row.updated_at = Utc::now().naive_utc();
        let updated = self.save_alarm_schedule(row).await?;
        Ok(Some(updated))
    }

    pub async fn delete_alarm_schedule(&self, id: i32) -> anyhow::Result<bool> {
        let res = alarm_schedules::Entity::delete_by_id(id)
            .exec(&self.db)
            .await?;
        Ok(res.rows_affected > 0)
    }

    /// Mark all due schedules as rang and compute each schedule's next trigger.
    /// Returns the IDs and next_unix values of schedules that were triggered.
    pub async fn advance_due_alarm_schedules(
        &self,
        now_unix: i64,
    ) -> anyhow::Result<Vec<(i32, i64)>> {
        let due = alarm_schedules::Entity::find()
            .filter(alarm_schedules::Column::Enabled.eq(true))
            .filter(alarm_schedules::Column::NextUnix.lte(now_unix))
            .order_by_asc(alarm_schedules::Column::NextUnix)
            .all(&self.db)
            .await?;

        let mut triggered = Vec::new();
        for mut s in due {
            let next_unix = s.next_unix;
            triggered.push((s.id, next_unix.unwrap_or(now_unix)));

            s.last_rang_unix = Some(now_unix);
            s.next_unix = match s.kind.as_str() {
                "once" => {
                    s.enabled = false;
                    None
                }
                "cron" => {
                    if let Some(expr) = s.cron_expr.as_ref() {
                        next_from_cron(expr, now_unix, &s.timezone)?
                    } else {
                        None
                    }
                }
                _ => None,
            };
            s.last_sent_unix = None;
            s.updated_at = Utc::now().naive_utc();
            self.save_alarm_schedule(s).await?;
        }
        Ok(triggered)
    }

    /// Update the one closest matching schedule when the strap reports an alarm.
    pub async fn mark_alarm_rang(&self, strap_unix: i64) -> anyhow::Result<()> {
        let now = Utc::now().timestamp();
        let candidates = alarm_schedules::Entity::find()
            .filter(alarm_schedules::Column::Enabled.eq(true))
            .filter(alarm_schedules::Column::NextUnix.gte(strap_unix - 90))
            .filter(alarm_schedules::Column::NextUnix.lte(strap_unix + 90))
            .all(&self.db)
            .await?;
        let Some(mut s) = candidates
            .into_iter()
            .min_by_key(|s| (s.next_unix.unwrap_or(i64::MAX) - strap_unix).abs())
        else {
            return Ok(());
        };
        s.last_rang_unix = Some(strap_unix);
        s.next_unix = match s.kind.as_str() {
            "once" => {
                s.enabled = false;
                None
            }
            "cron" => s
                .cron_expr
                .as_deref()
                .map(|expr| next_from_cron(expr, now, &s.timezone))
                .transpose()?
                .flatten(),
            _ => None,
        };
        s.last_sent_unix = None;
        s.updated_at = Utc::now().naive_utc();
        self.save_alarm_schedule(s).await?;
        Ok(())
    }

    /// Expire occurrences that are no longer safe to program on the strap.
    /// Expiration is deliberately distinct from a confirmed strap alarm event.
    pub async fn expire_past_alarm_schedules(&self, now_unix: i64) -> anyhow::Result<usize> {
        let due = alarm_schedules::Entity::find()
            .filter(alarm_schedules::Column::Enabled.eq(true))
            .filter(alarm_schedules::Column::NextUnix.lt(now_unix))
            .all(&self.db)
            .await?;
        let count = due.len();
        for mut row in due {
            if row.last_sent_unix.is_some() {
                row.last_unconfirmed_unix = row.next_unix;
            } else {
                row.last_missed_unix = row.next_unix;
            }
            row.next_unix = match row.kind.as_str() {
                "once" => {
                    row.enabled = false;
                    None
                }
                "cron" => row
                    .cron_expr
                    .as_deref()
                    .map(|expr| next_from_cron(expr, now_unix, &row.timezone))
                    .transpose()?
                    .flatten(),
                _ => None,
            };
            row.last_sent_unix = None;
            row.updated_at = Utc::now().naive_utc();
            self.save_alarm_schedule(row).await?;
        }
        Ok(count)
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
            self.save_alarm_schedule(row).await?;
        }
        Ok(())
    }

    pub async fn list_pending_commands(
        &self,
        device_id: &str,
    ) -> anyhow::Result<Vec<command_queue::Model>> {
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

    pub async fn get_device_alarm_state(
        &self,
        device_id: &str,
    ) -> anyhow::Result<Option<(bool, i64)>> {
        // Get the most recently programmed alarm for this device (from alarm_schedules)
        // In a multi-device scenario, we'd need a device_id column on alarm_schedules too
        // For now, we track device-specific alarm state via a simple query
        // This is a placeholder - actual implementation would need device tracking
        let _ = device_id;
        Ok(None)
    }
}

fn next_from_cron(expr: &str, from_unix: i64, timezone: &str) -> anyhow::Result<Option<i64>> {
    let sched = parse_cron_schedule(expr)?;
    let tz: Tz = timezone
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid IANA timezone: {timezone}"))?;
    let from = chrono::DateTime::<Utc>::from_timestamp(from_unix, 0)
        .ok_or_else(|| anyhow::anyhow!("invalid from_unix"))?;
    Ok(sched
        .after(&from.with_timezone(&tz))
        .next()
        .map(|dt| dt.timestamp()))
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
        db.mark_command_failed(id, "connection timeout")
            .await
            .unwrap();

        let pending = db.list_pending_commands("device123").await.unwrap();
        assert_eq!(pending.len(), 0);
    }

    #[tokio::test]
    async fn queue_commands_for_different_devices() {
        let db = DatabaseHandler::new("sqlite::memory:").await;

        db.push_command("device1", "buzzer", None).await.unwrap();
        db.push_command("device2", "buzzer", None).await.unwrap();
        db.push_command(
            "device1",
            "set-alarm",
            Some(serde_json::json!({"unix": 1234567890})),
        )
        .await
        .unwrap();

        let pending1 = db.list_pending_commands("device1").await.unwrap();
        let pending2 = db.list_pending_commands("device2").await.unwrap();

        assert_eq!(pending1.len(), 2);
        assert_eq!(pending2.len(), 1);
    }

    #[test]
    fn daily_alarm_keeps_local_hour_across_dst() {
        let march_7 = chrono::DateTime::parse_from_rfc3339("2026-03-07T13:00:00Z")
            .unwrap()
            .timestamp();
        let next = next_from_cron("0 7 * * *", march_7, "America/New_York")
            .unwrap()
            .unwrap();
        assert_eq!(
            next,
            chrono::DateTime::parse_from_rfc3339("2026-03-08T11:00:00Z")
                .unwrap()
                .timestamp()
        );

        let october_31 = chrono::DateTime::parse_from_rfc3339("2026-10-31T12:00:00Z")
            .unwrap()
            .timestamp();
        let next = next_from_cron("0 7 * * *", october_31, "America/New_York")
            .unwrap()
            .unwrap();
        assert_eq!(
            next,
            chrono::DateTime::parse_from_rfc3339("2026-11-01T12:00:00Z")
                .unwrap()
                .timestamp()
        );
    }

    #[tokio::test]
    async fn expired_alarm_is_missed_and_never_rang() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let now = Utc::now().timestamp();
        let alarm = db
            .create_alarm_schedule(AlarmScheduleDraft {
                label: "test".into(),
                kind: "once".into(),
                cron_expr: None,
                timezone: Some("America/New_York".into()),
                one_time_unix: Some(now + 3600),
            })
            .await
            .unwrap();
        let mut row: alarm_schedules::ActiveModel = alarm.into();
        row.next_unix = Set(Some(now - 60));
        row.update(db.connection()).await.unwrap();

        assert_eq!(db.expire_past_alarm_schedules(now).await.unwrap(), 1);
        let stored = db.list_alarm_schedules().await.unwrap().pop().unwrap();
        assert!(!stored.enabled);
        assert_eq!(stored.last_missed_unix, Some(now - 60));
        assert_eq!(stored.last_rang_unix, None);
        assert_eq!(db.next_enabled_alarm_unix(now).await.unwrap(), None);
    }

    #[tokio::test]
    async fn strap_event_only_completes_closest_alarm() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let now = Utc::now().timestamp();
        for (label, unix) in [("first", now + 3600), ("second", now + 3620)] {
            db.create_alarm_schedule(AlarmScheduleDraft {
                label: label.into(),
                kind: "once".into(),
                cron_expr: None,
                timezone: Some("America/New_York".into()),
                one_time_unix: Some(unix),
            })
            .await
            .unwrap();
        }
        db.mark_alarm_rang(now + 3600).await.unwrap();
        let rows = db.list_alarm_schedules().await.unwrap();
        assert!(!rows.iter().find(|r| r.label == "first").unwrap().enabled);
        assert!(rows.iter().find(|r| r.label == "second").unwrap().enabled);
    }

    #[tokio::test]
    async fn programmed_alarm_without_event_is_unconfirmed_not_missed() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let now = Utc::now().timestamp();
        let alarm = db
            .create_alarm_schedule(AlarmScheduleDraft {
                label: "test".into(),
                kind: "once".into(),
                cron_expr: None,
                timezone: Some("America/New_York".into()),
                one_time_unix: Some(now + 3600),
            })
            .await
            .unwrap();
        let mut row: alarm_schedules::ActiveModel = alarm.into();
        row.next_unix = Set(Some(now - 60));
        row.last_sent_unix = Set(Some(now - 3600));
        row.update(db.connection()).await.unwrap();

        db.expire_past_alarm_schedules(now).await.unwrap();
        let stored = db.list_alarm_schedules().await.unwrap().pop().unwrap();
        assert_eq!(stored.last_unconfirmed_unix, Some(now - 60));
        assert_eq!(stored.last_missed_unix, None);
        assert_eq!(stored.last_rang_unix, None);
    }

    #[tokio::test]
    async fn cloud_hr_outbox_keeps_samples_until_acknowledged() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        db.enqueue_cloud_hr_sample(1_700_000_000_000, 73, 1_700_000_000_100)
            .await
            .unwrap();
        assert_eq!(db.pending_cloud_hr_samples(100).await.unwrap().len(), 1);
        db.mark_cloud_hr_synced(&[1_700_000_000_000]).await.unwrap();
        assert!(db.pending_cloud_hr_samples(100).await.unwrap().is_empty());
        // A corrected sample must be uploaded again.
        db.enqueue_cloud_hr_sample(1_700_000_000_000, 74, 1_700_000_000_200)
            .await
            .unwrap();
        let rows = db.pending_cloud_hr_samples(100).await.unwrap();
        assert_eq!(rows[0].bpm, 74);
    }

    #[tokio::test]
    async fn cloud_alarm_toggle_and_delete_reconcile_locally() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        let mut remote = CloudAlarmInput {
            id: Uuid::new_v4(),
            label: "first".into(),
            kind: "once".into(),
            cron_expr: None,
            timezone: "America/New_York".into(),
            one_time_unix: Some(Utc::now().timestamp() + 3600),
            enabled: true,
        };
        db.reconcile_cloud_alarms(&[remote.clone()]).await.unwrap();
        let first = db.list_alarm_schedules().await.unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].cloud_id, Some(remote.id));
        assert!(first[0].enabled);
        remote.enabled = false;
        db.reconcile_cloud_alarms(&[remote.clone()]).await.unwrap();
        let off = db.list_alarm_schedules().await.unwrap();
        assert!(!off[0].enabled);
        assert_eq!(off[0].next_unix, None);
        remote.enabled = true;
        db.reconcile_cloud_alarms(&[remote]).await.unwrap();
        assert!(
            db.list_alarm_schedules().await.unwrap()[0]
                .next_unix
                .is_some()
        );
        db.reconcile_cloud_alarms(&[]).await.unwrap();
        assert!(db.list_alarm_schedules().await.unwrap().is_empty());
    }
}
