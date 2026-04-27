//! HTTP JSON API for the local Studio dashboard (insights + device control during `live-server`).

use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, patch, post},
};
use chrono::{Duration as ChDuration, Local, TimeZone};
use openwhoop_entities::heart_rate;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::time::timeout;

use openwhoop::{
    OpenWhoop, StudioDeviceJob,
    algo::{ExerciseMetrics, SleepConsistencyAnalyzer},
    db::{AlarmScheduleDraft, AlarmSchedulePatch},
    types::activities::{ActivityType, SearchActivityPeriods},
};

use crate::app_state::AppState;

pub fn api_routes() -> Router<AppState> {
    Router::new()
        .route("/api/insights/sleep", get(insights_sleep))
        .route("/api/insights/exercise", get(insights_exercise))
        .route("/api/insights/vitals", get(insights_vitals))
        .route(
            "/api/insights/heart-rate-series",
            get(insights_heart_rate_series),
        )
        .route("/api/compute/stress", post(compute_stress))
        .route("/api/compute/spo2", post(compute_spo2))
        .route("/api/compute/skin-temp", post(compute_skin_temp))
        .route("/api/compute/detect-events", post(compute_detect_events))
        .route("/api/device/battery", post(device_battery))
        .route(
            "/api/device/alarm",
            get(device_alarm_get).post(device_alarm_set),
        )
        .route("/api/device/alarm/clear", post(device_alarm_clear))
        .route(
            "/api/device/buzzer",
            post(device_buzzer).delete(device_buzzer_stop),
        )
        .route("/api/alarms", get(alarms_list).post(alarms_create))
        .route(
            "/api/alarms/{id}",
            patch(alarms_patch).delete(alarms_delete),
        )
        .route("/api/device/imu/r7-toggle", post(device_imu_r7))
        .route("/api/device/imu/mode", post(device_imu_mode))
}

async fn insights_sleep(State(app): State<AppState>) -> Result<Json<Value>, (StatusCode, String)> {
    let db = app.db.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available - run with DATABASE_URL or connect to AWS".into(),
    ))?;
    let sleep_records = db
        .get_sleep_cycles(None)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if sleep_records.is_empty() {
        return Ok(Json(
            json!({ "empty": true, "message": "No sleep data in your history yet." }),
        ));
    }

    let mut last_week: Vec<_> = sleep_records.iter().rev().take(7).copied().collect();
    last_week.reverse();

    let all = SleepConsistencyAnalyzer::new(sleep_records.clone())
        .calculate_consistency_metrics()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let week = SleepConsistencyAnalyzer::new(last_week)
        .calculate_consistency_metrics()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({
        "sleep_cycles_total": sleep_records.len(),
        "consistency_all_time": format!("{}", all),
        "consistency_last_7_cycles": format!("{}", week),
    })))
}

async fn insights_exercise(
    State(app): State<AppState>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let db = app.db.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available - run with DATABASE_URL or connect to AWS".into(),
    ))?;
    let exercises = db
        .search_activities(SearchActivityPeriods::default().with_activity(ActivityType::Activity))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if exercises.is_empty() {
        return Ok(Json(
            json!({ "empty": true, "message": "No workouts or activities recorded yet." }),
        ));
    }

    let last_week: Vec<_> = exercises.iter().rev().take(7).copied().rev().collect();
    let all = ExerciseMetrics::new(exercises)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let week = ExerciseMetrics::new(last_week)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({
        "activities_total": all.count,
        "metrics_all_time": format!("{}", all),
        "metrics_last_7": format!("{}", week),
    })))
}

async fn insights_vitals(State(app): State<AppState>) -> Result<Json<Value>, (StatusCode, String)> {
    // Try in-memory first (thin proxy mode), then database if available
    if let Some(last_hr) = app.last_hr_json.lock().unwrap().as_ref() {
        let parsed: Value = serde_json::from_str(last_hr).map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Invalid JSON in cache".into(),
            )
        })?;
        return Ok(Json(parsed));
    }
    // Fall back to database
    let db = app.db.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available and no live HR data yet".into(),
    ))?;
    let rows = heart_rate::Entity::find()
        .order_by_desc(heart_rate::Column::Time)
        .limit(40)
        .all(db.connection())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let samples: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "time": r.time.to_string(),
                "bpm": r.bpm,
                "stress": r.stress,
                "spo2": r.spo2,
                "skin_temp": r.skin_temp,
                "has_imu": r.imu_data.is_some(),
                "has_sensor_block": r.sensor_data.is_some(),
            })
        })
        .collect();

    Ok(Json(json!({ "latest": samples })))
}

#[derive(Deserialize)]
struct HeartSeriesQuery {
    /// One of `1h`, `24h`, `7d`.
    range: String,
}

fn heart_series_hours(range: &str) -> Option<i64> {
    match range {
        "1h" | "hour" => Some(1),
        "24h" | "1d" | "day" => Some(24),
        "7d" | "week" => Some(24 * 7),
        _ => None,
    }
}

async fn insights_heart_rate_series(
    Query(q): Query<HeartSeriesQuery>,
    State(app): State<AppState>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let db = app.db.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available - run with DATABASE_URL or connect to AWS".into(),
    ))?;
    let hours = heart_series_hours(&q.range).ok_or((
        StatusCode::BAD_REQUEST,
        "Invalid range. Use 1h, 24h, or 7d.".to_string(),
    ))?;
    let from = Local::now().naive_local() - ChDuration::hours(hours);
    let rows = heart_rate::Entity::find()
        .filter(heart_rate::Column::Time.gte(from))
        .order_by_asc(heart_rate::Column::Time)
        .limit(50_000)
        .all(db.connection())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // `heart_rate.time` is stored as local wall clock (see `timestamp_to_local` on ingest).
    // Do not use `naive.and_utc()` — that treats civil time as UTC and shifts the axis.
    let points: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let t_ms = Local
                .from_local_datetime(&r.time)
                .single()
                .or_else(|| Local.from_local_datetime(&r.time).latest())
                .map(|dt| dt.timestamp_millis())
                .unwrap_or(0);
            json!({
                "t_ms": t_ms,
                "bpm": r.bpm,
            })
        })
        .collect();

    Ok(Json(json!({
        "range": q.range,
        "count": points.len(),
        "points": points,
    })))
}

async fn compute_stress(State(app): State<AppState>) -> Result<Json<Value>, (StatusCode, String)> {
    let db = app.db.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available - run with DATABASE_URL or connect to AWS".into(),
    ))?;
    let w = OpenWhoop::new(db.clone());
    w.calculate_stress()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(
        json!({ "ok": true, "message": "stress pass complete (see /api/insights/vitals)" }),
    ))
}

async fn compute_spo2(State(app): State<AppState>) -> Result<Json<Value>, (StatusCode, String)> {
    let db = app.db.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available - run with DATABASE_URL or connect to AWS".into(),
    ))?;
    let w = OpenWhoop::new(db.clone());
    w.calculate_spo2()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({ "ok": true, "message": "SpO2 pass complete" })))
}

async fn compute_skin_temp(
    State(app): State<AppState>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let db = app.db.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available - run with DATABASE_URL or connect to AWS".into(),
    ))?;
    let w = OpenWhoop::new(db.clone());
    w.calculate_skin_temp()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(
        json!({ "ok": true, "message": "skin temperature pass complete" }),
    ))
}

async fn compute_detect_events(
    State(app): State<AppState>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let db = app.db.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available - run with DATABASE_URL or connect to AWS".into(),
    ))?;
    let w = OpenWhoop::new(db.clone());
    w.detect_sleeps()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    w.detect_events()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(
        json!({ "ok": true, "message": "detect-sleeps + detect-events finished" }),
    ))
}

async fn device_battery(State(app): State<AppState>) -> Result<Json<Value>, (StatusCode, String)> {
    let (tx, rx) = oneshot::channel();
    app.job_tx
        .send(StudioDeviceJob::PollBattery { reply: tx })
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "BLE loop not running".into(),
            )
        })?;
    let v = timeout(Duration::from_secs(12), rx)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "timeout waiting for strap".into(),
            )
        })?
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "reply dropped".into()))?
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(v))
}

async fn device_alarm_get(
    State(app): State<AppState>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let (tx, rx) = oneshot::channel();
    app.job_tx
        .send(StudioDeviceJob::GetAlarm { reply: tx })
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "BLE loop not running".into(),
            )
        })?;
    let v = timeout(Duration::from_secs(12), rx)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "timeout waiting for strap".into(),
            )
        })?
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "reply dropped".into()))?
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(v))
}

#[derive(Deserialize)]
struct AlarmSetBody {
    /// Unix timestamp in seconds (UTC).
    unix: u32,
}

async fn device_alarm_set(
    State(app): State<AppState>,
    Json(body): Json<AlarmSetBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let (tx, rx) = oneshot::channel();
    app.job_tx
        .send(StudioDeviceJob::SetAlarm {
            unix: body.unix,
            reply: tx,
        })
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "BLE loop not running".into(),
            )
        })?;
    let v = rx
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "reply dropped".into()))?
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(v))
}

async fn device_alarm_clear(
    State(app): State<AppState>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let (tx, rx) = oneshot::channel();
    app.job_tx
        .send(StudioDeviceJob::ClearAlarm { reply: tx })
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "BLE loop not running".into(),
            )
        })?;
    let v = rx
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "reply dropped".into()))?
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(v))
}

async fn device_buzzer(State(app): State<AppState>) -> Result<Json<Value>, (StatusCode, String)> {
    let (tx, rx) = oneshot::channel();
    app.job_tx
        .send(StudioDeviceJob::Buzzer { reply: tx })
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "BLE loop not running".into(),
            )
        })?;
    let v = timeout(Duration::from_secs(12), rx)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "timeout waiting for strap".into(),
            )
        })?
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "reply dropped".into()))?
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(v))
}

async fn device_buzzer_stop(
    State(app): State<AppState>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let (tx, rx) = oneshot::channel();
    app.job_tx
        .send(StudioDeviceJob::StopBuzzer { reply: tx })
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "BLE loop not running".into(),
            )
        })?;
    let v = timeout(Duration::from_secs(12), rx)
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                "timeout waiting for strap".into(),
            )
        })?
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "reply dropped".into()))?
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(v))
}

async fn alarms_list(State(app): State<AppState>) -> Result<Json<Value>, (StatusCode, String)> {
    let db = app.db.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available - run with DATABASE_URL or connect to AWS".into(),
    ))?;
    let items = db
        .list_alarm_schedules()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let data: Vec<Value> = items
        .into_iter()
        .map(|a| {
            json!({
                "id": a.id,
                "label": a.label,
                "kind": a.kind,
                "cron_expr": a.cron_expr,
                "one_time_unix": a.one_time_unix,
                "next_unix": a.next_unix,
                "last_rang_unix": a.last_rang_unix,
                "last_sent_unix": a.last_sent_unix,
                "enabled": a.enabled,
            })
        })
        .collect();
    Ok(Json(json!({ "items": data })))
}

#[derive(Deserialize)]
struct AlarmCreateBody {
    label: String,
    /// `cron` or `once`
    kind: String,
    cron_expr: Option<String>,
    one_time_unix: Option<i64>,
}

async fn alarms_create(
    State(app): State<AppState>,
    Json(body): Json<AlarmCreateBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let db = app.db.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available - run with DATABASE_URL or connect to AWS".into(),
    ))?;
    let created = db
        .create_alarm_schedule(AlarmScheduleDraft {
            label: body.label,
            kind: body.kind,
            cron_expr: body.cron_expr,
            one_time_unix: body.one_time_unix,
        })
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(json!({
        "ok": true,
        "id": created.id,
        "next_unix": created.next_unix
    })))
}

#[derive(Deserialize)]
struct AlarmPatchBody {
    label: Option<String>,
    enabled: Option<bool>,
}

async fn alarms_patch(
    Path(id): Path<i32>,
    State(app): State<AppState>,
    Json(body): Json<AlarmPatchBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let db = app.db.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available - run with DATABASE_URL or connect to AWS".into(),
    ))?;
    let updated = db
        .patch_alarm_schedule(
            id,
            AlarmSchedulePatch {
                label: body.label,
                enabled: body.enabled,
            },
        )
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    match updated {
        Some(a) => Ok(Json(json!({
            "ok": true,
            "id": a.id,
            "enabled": a.enabled,
            "next_unix": a.next_unix,
        }))),
        None => Err((StatusCode::NOT_FOUND, "schedule not found".into())),
    }
}

async fn alarms_delete(
    Path(id): Path<i32>,
    State(app): State<AppState>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let db = app.db.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "Database not available - run with DATABASE_URL or connect to AWS".into(),
    ))?;
    let ok = db
        .delete_alarm_schedule(id)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    if !ok {
        return Err((StatusCode::NOT_FOUND, "schedule not found".into()));
    }
    Ok(Json(json!({ "ok": true })))
}

async fn device_imu_r7(State(app): State<AppState>) -> Result<Json<Value>, (StatusCode, String)> {
    let (tx, rx) = oneshot::channel();
    app.job_tx
        .send(StudioDeviceJob::ToggleImuCollection { reply: tx })
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "BLE loop not running".into(),
            )
        })?;
    let v = rx
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "reply dropped".into()))?
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(v))
}

#[derive(Deserialize)]
struct ImuModeBody {
    enable: bool,
    #[serde(default)]
    historical: bool,
}

async fn device_imu_mode(
    State(app): State<AppState>,
    Json(body): Json<ImuModeBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let (tx, rx) = oneshot::channel();
    app.job_tx
        .send(StudioDeviceJob::ToggleImuMode {
            enable: body.enable,
            historical: body.historical,
            reply: tx,
        })
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "BLE loop not running".into(),
            )
        })?;
    let v = rx
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "reply dropped".into()))?
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(v))
}
