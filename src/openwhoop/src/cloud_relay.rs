//! Outbound-only bridge from the Mac BLE relay to the owner's Supabase project.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use chrono::{DateTime, Utc};
use openwhoop::{
    StudioDeviceJob,
    db::{CloudAlarmInput, DatabaseHandler},
};
use reqwest::{Client, Method, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    sync::{mpsc, oneshot},
    time::{Instant, MissedTickBehavior, interval, interval_at, timeout},
};
use uuid::Uuid;

#[derive(Clone)]
pub struct CloudConfig {
    pub url: String,
    pub secret_key: String,
    pub owner_id: Uuid,
}

impl CloudConfig {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let url = std::env::var("OPENWHOOP_SUPABASE_URL").ok();
        let secret_key = std::env::var("OPENWHOOP_SUPABASE_SECRET_KEY").ok();
        let owner_id = std::env::var("OPENWHOOP_SUPABASE_OWNER_ID").ok();
        if url.is_none() && secret_key.is_none() && owner_id.is_none() {
            return Ok(None);
        }
        let url = url
            .ok_or_else(|| anyhow::anyhow!("OPENWHOOP_SUPABASE_URL is required for cloud relay"))?;
        anyhow::ensure!(
            url.starts_with("https://"),
            "OPENWHOOP_SUPABASE_URL must use HTTPS"
        );
        let secret_key = secret_key.ok_or_else(|| {
            anyhow::anyhow!("OPENWHOOP_SUPABASE_SECRET_KEY is required for cloud relay")
        })?;
        let owner_id = owner_id
            .ok_or_else(|| {
                anyhow::anyhow!("OPENWHOOP_SUPABASE_OWNER_ID is required for cloud relay")
            })?
            .parse()?;
        Ok(Some(Self {
            url: url.trim_end_matches('/').into(),
            secret_key,
            owner_id,
        }))
    }

    fn rest_url(&self, table: &str, query: &str) -> String {
        format!("{}/rest/v1/{table}?{query}", self.url)
    }

    fn request(&self, client: &Client, method: Method, url: String) -> reqwest::RequestBuilder {
        let request = client
            .request(method, url)
            .header("apikey", &self.secret_key);
        if self.secret_key.starts_with("eyJ") {
            request.bearer_auth(&self.secret_key)
        } else {
            request
        }
    }
}

#[derive(Deserialize)]
struct CloudCommand {
    id: Uuid,
    kind: String,
    expires_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct CloudAlarm {
    id: Uuid,
    label: String,
    kind: String,
    cron_expr: Option<String>,
    timezone: String,
    one_time_unix: Option<i64>,
    enabled: bool,
}

async fn sync_alarms(
    db: &DatabaseHandler,
    cfg: &CloudConfig,
    client: &Client,
) -> anyhow::Result<()> {
    let response = cfg
        .request(
            client,
            Method::GET,
            cfg.rest_url(
                "whoop_alarms",
                &format!(
                    "owner_id=eq.{}&select=id,label,kind,cron_expr,timezone,one_time_unix,enabled&limit=1000",
                    cfg.owner_id
                ),
            ),
        )
        .send()
        .await?;
    let cloud: Vec<CloudAlarm> = checked(response).await?.json().await?;
    anyhow::ensure!(
        cloud.len() < 1000,
        "cloud alarm limit reached; refusing destructive reconciliation"
    );
    let incoming: Vec<CloudAlarmInput> = cloud
        .into_iter()
        .map(|a| CloudAlarmInput {
            id: a.id,
            label: a.label,
            kind: a.kind,
            cron_expr: a.cron_expr,
            timezone: a.timezone,
            one_time_unix: a.one_time_unix,
            enabled: a.enabled,
        })
        .collect();
    db.reconcile_cloud_alarms(&incoming).await?;
    for row in db
        .list_alarm_schedules()
        .await?
        .into_iter()
        .filter(|row| row.cloud_id.is_some())
    {
        let mut state = json!({
            "next_unix": row.next_unix,
            "last_rang_unix": row.last_rang_unix,
            "last_missed_unix": row.last_missed_unix,
            "last_unconfirmed_unix": row.last_unconfirmed_unix,
            "last_programmed_unix": row.last_sent_unix,
        });
        if row.kind == "once"
            && row
                .one_time_unix
                .is_some_and(|unix| unix <= Utc::now().timestamp())
        {
            state["enabled"] = json!(false);
        }
        let response = cfg
            .request(
                client,
                Method::PATCH,
                cfg.rest_url(
                    "whoop_alarms",
                    &format!(
                        "id=eq.{}&owner_id=eq.{}",
                        row.cloud_id.unwrap(),
                        cfg.owner_id
                    ),
                ),
            )
            .json(&state)
            .send()
            .await?;
        checked(response).await?;
    }
    Ok(())
}

async fn checked(response: Response) -> anyhow::Result<Response> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "Supabase HTTP {status}: {}",
            body.chars().take(200).collect::<String>()
        );
    }
    Ok(response)
}

async fn upload_hr(db: &DatabaseHandler, cfg: &CloudConfig, client: &Client) -> anyhow::Result<()> {
    let rows = db.pending_cloud_hr_samples(200).await?;
    if rows.is_empty() {
        return Ok(());
    }
    let payload: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "owner_id": cfg.owner_id,
                "recorded_at_ms": r.recorded_at_ms,
                "bpm": r.bpm,
                "received_at_ms": r.received_at_ms,
                "source": "gatt_live",
            })
        })
        .collect();
    let response = cfg
        .request(
            client,
            Method::POST,
            cfg.rest_url("whoop_hr_samples", "on_conflict=owner_id%2Crecorded_at_ms"),
        )
        .header("Prefer", "resolution=merge-duplicates,return=minimal")
        .json(&payload)
        .send()
        .await?;
    checked(response).await?;
    db.mark_cloud_hr_synced(&rows.iter().map(|r| r.recorded_at_ms).collect::<Vec<_>>())
        .await?;
    Ok(())
}

async fn complete_command(
    cfg: &CloudConfig,
    client: &Client,
    id: Uuid,
    status: &str,
    error: Option<&str>,
) -> anyhow::Result<()> {
    let payload = json!({ "status": status, "completed_at": Utc::now(), "error": error });
    let response = cfg
        .request(
            client,
            Method::PATCH,
            cfg.rest_url(
                "whoop_commands",
                &format!("id=eq.{id}&owner_id=eq.{}", cfg.owner_id),
            ),
        )
        .json(&payload)
        .send()
        .await?;
    checked(response).await?;
    Ok(())
}

async fn process_commands(
    cfg: &CloudConfig,
    client: &Client,
    jobs: &mpsc::Sender<StudioDeviceJob>,
    force_alarm_recheck: &Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let response = cfg
        .request(
            client,
            Method::GET,
            cfg.rest_url(
                "whoop_commands",
                &format!(
                    "owner_id=eq.{}&status=eq.pending&order=created_at.asc&limit=20",
                    cfg.owner_id
                ),
            ),
        )
        .send()
        .await?;
    let commands: Vec<CloudCommand> = checked(response).await?.json().await?;
    for command in commands {
        if command.expires_at <= Utc::now() {
            complete_command(cfg, client, command.id, "expired", None).await?;
            continue;
        }
        if command.kind == "resync_alarm" {
            force_alarm_recheck.store(true, Ordering::SeqCst);
            complete_command(cfg, client, command.id, "done", None).await?;
            continue;
        }
        let (reply, result) = oneshot::channel();
        let job = match command.kind.as_str() {
            "buzzer" => StudioDeviceJob::Buzzer { reply },
            "stop_buzzer" => StudioDeviceJob::StopBuzzer { reply },
            _ => {
                complete_command(cfg, client, command.id, "failed", Some("unknown command"))
                    .await?;
                continue;
            }
        };
        let outcome = async {
            timeout(Duration::from_secs(3), jobs.send(job)).await??;
            timeout(Duration::from_secs(15), result)
                .await??
                .map_err(anyhow::Error::msg)?;
            if command.kind == "buzzer" {
                tokio::time::sleep(Duration::from_millis(900)).await;
                // Repeat stop after the first ACK; BLE ACK can precede physical motor shutdown.
                let mut stopped = false;
                for attempt in 0..3 {
                    let stop_result: anyhow::Result<()> = async {
                        let (reply, result) = oneshot::channel();
                        timeout(
                            Duration::from_secs(3),
                            jobs.send(StudioDeviceJob::StopBuzzer { reply }),
                        )
                        .await??;
                        timeout(Duration::from_secs(15), result)
                            .await??
                            .map_err(anyhow::Error::msg)?;
                        Ok(())
                    }
                    .await;
                    if let Err(error) = stop_result {
                        log::warn!("short buzz stop attempt failed: {error}");
                    } else {
                        stopped = true;
                    }
                    if attempt < 2 {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
                anyhow::ensure!(stopped, "could not stop short buzzer pulse");
            }
            Ok::<(), anyhow::Error>(())
        }
        .await;
        match outcome {
            Ok(()) => complete_command(cfg, client, command.id, "done", None).await?,
            Err(error) => {
                complete_command(cfg, client, command.id, "failed", Some(&error.to_string()))
                    .await?
            }
        }
    }
    Ok(())
}

async fn update_heartbeat(
    cfg: &CloudConfig,
    client: &Client,
    last_hr: &Arc<Mutex<Option<String>>>,
    strap_state: &Arc<Mutex<Option<Option<i64>>>>,
    battery_state: &Arc<Mutex<Option<(u8, DateTime<Utc>)>>>,
) -> anyhow::Result<()> {
    let sample = last_hr.lock().ok().and_then(|guard| guard.clone());
    let sample: Option<Value> = sample.as_deref().and_then(|s| serde_json::from_str(s).ok());
    let current_unix_ms = sample
        .as_ref()
        .and_then(|v| v.get("unix_ms"))
        .and_then(Value::as_i64);
    let fresh = current_unix_ms.is_some_and(|ms| Utc::now().timestamp_millis() - ms < 45_000);
    let strap = strap_state.lock().ok().and_then(|state| *state);
    let battery = battery_state.lock().ok().and_then(|state| state.clone());
    let payload = json!({
        "owner_id": cfg.owner_id,
        "last_seen_at": Utc::now(),
        "connected": fresh,
        "current_bpm": if fresh { sample.as_ref().and_then(|v| v.get("bpm")).and_then(Value::as_i64) } else { None },
        "current_unix_ms": current_unix_ms,
        "battery_percent": battery.as_ref().map(|(percent, _)| percent),
        "battery_updated_at": battery.as_ref().map(|(_, updated_at)| updated_at),
        "strap_alarm_enabled": strap.map(|unix| unix.is_some()),
        "strap_alarm_unix": strap.flatten(),
    });
    let response = cfg
        .request(
            client,
            Method::POST,
            cfg.rest_url("whoop_relay_status", "on_conflict=owner_id"),
        )
        .header("Prefer", "resolution=merge-duplicates,return=minimal")
        .json(&payload)
        .send()
        .await?;
    checked(response).await?;
    Ok(())
}

async fn poll_battery(
    jobs: mpsc::Sender<StudioDeviceJob>,
    state: Arc<Mutex<Option<(u8, DateTime<Utc>)>>>,
    exit: Arc<AtomicBool>,
) {
    // Keep battery requests out of the heartbeat loop and leave startup alarm
    // reconciliation time to finish before the first poll.
    let mut tick = interval_at(
        Instant::now() + Duration::from_secs(12),
        Duration::from_secs(60),
    );
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    while !exit.load(Ordering::SeqCst) {
        tick.tick().await;
        let result: anyhow::Result<u8> = async {
            let (reply, response) = oneshot::channel();
            timeout(
                Duration::from_secs(2),
                jobs.send(StudioDeviceJob::PollBattery { reply }),
            )
            .await??;
            let value = timeout(Duration::from_secs(8), response)
                .await??
                .map_err(anyhow::Error::msg)?;
            let percent = value
                .get("percent")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow::anyhow!("battery percentage unavailable"))?;
            anyhow::ensure!(percent <= 100, "invalid battery percentage");
            Ok(percent as u8)
        }
        .await;
        if let Ok(percent) = result {
            if let Ok(mut current) = state.lock() {
                *current = Some((percent, Utc::now()));
            }
        }
    }
}

pub async fn run(
    cfg: CloudConfig,
    db: DatabaseHandler,
    jobs: mpsc::Sender<StudioDeviceJob>,
    last_hr: Arc<Mutex<Option<String>>>,
    exit: Arc<AtomicBool>,
    force_alarm_recheck: Arc<AtomicBool>,
    strap_state: Arc<Mutex<Option<Option<i64>>>>,
) {
    let client = match Client::builder().timeout(Duration::from_secs(10)).build() {
        Ok(client) => client,
        Err(error) => {
            log::error!("cloud relay HTTP client: {error}");
            return;
        }
    };
    let mut tick = interval(Duration::from_secs(2));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let battery_state = Arc::new(Mutex::new(None));
    let battery_task = tokio::spawn(poll_battery(
        jobs.clone(),
        battery_state.clone(),
        exit.clone(),
    ));
    let mut last_heartbeat = 0;
    while !exit.load(Ordering::SeqCst) {
        tick.tick().await;
        if let Err(error) = upload_hr(&db, &cfg, &client).await {
            log::warn!("cloud HR upload pending: {error}");
        }
        if let Err(error) = sync_alarms(&db, &cfg, &client).await {
            log::warn!("cloud alarm sync pending: {error}");
        }
        if let Err(error) = process_commands(&cfg, &client, &jobs, &force_alarm_recheck).await {
            log::warn!("cloud command poll pending: {error}");
        }
        let now = Utc::now().timestamp();
        if now - last_heartbeat >= 10 {
            if let Err(error) =
                update_heartbeat(&cfg, &client, &last_hr, &strap_state, &battery_state).await
            {
                log::warn!("cloud heartbeat pending: {error}");
            } else {
                last_heartbeat = now;
            }
        }
    }
    battery_task.abort();
}
