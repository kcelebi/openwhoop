//! Reconcile multiple stored schedules with the strap's single native alarm.
//! The Mac owns this loop while it owns the BLE connection.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use chrono::Utc;
use openwhoop::{StudioDeviceJob, db::DatabaseHandler};
use serde_json::Value;
use tokio::{
    sync::{mpsc, oneshot},
    time::{MissedTickBehavior, interval, timeout},
};

const JOB_TIMEOUT: Duration = Duration::from_secs(15);
const RECHECK_INTERVAL: Duration = Duration::from_secs(5);
const READBACK_INTERVAL_SECS: i64 = 60;
const LATE_GRACE_SECS: i64 = 10;

async fn get_strap_alarm(jobs: &mpsc::Sender<StudioDeviceJob>) -> anyhow::Result<Option<i64>> {
    let (reply, result) = oneshot::channel();
    timeout(JOB_TIMEOUT, jobs.send(StudioDeviceJob::GetAlarm { reply })).await??;
    let value = timeout(JOB_TIMEOUT, result)
        .await??
        .map_err(anyhow::Error::msg)?;
    let enabled = value
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(enabled
        .then(|| value.get("unix").and_then(Value::as_i64))
        .flatten())
}

async fn set_strap_alarm(
    jobs: &mpsc::Sender<StudioDeviceJob>,
    unix: Option<i64>,
) -> anyhow::Result<()> {
    let (reply, result) = oneshot::channel();
    let job = match unix {
        Some(unix) => StudioDeviceJob::SetAlarm {
            unix: u32::try_from(unix)?,
            reply,
        },
        None => StudioDeviceJob::ClearAlarm { reply },
    };
    timeout(JOB_TIMEOUT, jobs.send(job)).await??;
    timeout(JOB_TIMEOUT, result)
        .await??
        .map_err(anyhow::Error::msg)?;
    Ok(())
}

pub async fn run(db: DatabaseHandler, jobs: mpsc::Sender<StudioDeviceJob>, exit: Arc<AtomicBool>) {
    let mut tick = interval(RECHECK_INTERVAL);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_verified: Option<Option<i64>> = None;
    let mut last_readback_unix = 0;
    while !exit.load(Ordering::SeqCst) {
        tick.tick().await;
        let now = Utc::now().timestamp();
        let result: anyhow::Result<()> = async {
            db.expire_past_alarm_schedules(now - LATE_GRACE_SECS)
                .await?;
            let desired = db.next_enabled_alarm_unix(now - LATE_GRACE_SECS).await?;
            if last_verified == Some(desired) && now - last_readback_unix < READBACK_INTERVAL_SECS {
                return Ok(());
            }

            let current = get_strap_alarm(&jobs).await?;
            // A dashboard edit may have happened while the BLE read was pending.
            let desired = db
                .next_enabled_alarm_unix(Utc::now().timestamp() - LATE_GRACE_SECS)
                .await?;
            if current != desired {
                // Never send an alarm timestamp that is already due or too close to verify.
                if desired.is_some_and(|unix| unix <= Utc::now().timestamp() + 5) {
                    anyhow::bail!("next alarm is too close to program safely");
                }
                set_strap_alarm(&jobs, desired).await?;
                let readback = get_strap_alarm(&jobs).await?;
                anyhow::ensure!(
                    readback == desired,
                    "strap alarm readback did not match desired schedule"
                );
            }
            if let Some(unix) = desired {
                db.mark_alarm_programmed(unix).await?;
            }
            last_verified = Some(desired);
            last_readback_unix = Utc::now().timestamp();
            Ok(())
        }
        .await;
        if let Err(error) = result {
            last_verified = None;
            log::warn!("alarm reconciliation pending: {error}");
        }
    }
}
