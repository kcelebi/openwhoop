#[macro_use]
extern crate log;

mod app_state;
mod live_server;
mod studio;

use std::{
    io,
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::anyhow;
#[cfg(target_os = "linux")]
use btleplug::api::BDAddr;
use btleplug::{
    api::{Central, Manager as _, Peripheral as _, ScanFilter},
    platform::{Adapter, Manager, Peripheral},
};
use chrono::{DateTime, Local, NaiveDateTime, NaiveTime, TimeDelta, Utc};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use dotenv::dotenv;
use openwhoop::api;
use openwhoop::{
    OpenWhoop, StudioDeviceJob, WhoopDevice,
    algo::{ExerciseMetrics, SleepConsistencyAnalyzer},
    db::DatabaseHandler,
    types::activities::{ActivityType, SearchActivityPeriods},
};
use openwhoop_codec::{WhoopPacket, constants::WHOOP_SERVICE};
use openwhoop_entities::packets;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{sleep, timeout};

use crate::app_state::AppState;

#[cfg(target_os = "linux")]
pub type DeviceId = BDAddr;

#[cfg(target_os = "macos")]
pub type DeviceId = String;

#[derive(Parser)]
pub struct OpenWhoopCli {
    #[arg(env, long)]
    pub debug_packets: bool,
    /// SQLite or Postgres URL. Not used by `scan`, `completions`, or `download-firmware`.
    #[arg(env = "DATABASE_URL", long = "database-url")]
    pub database_url: Option<String>,
    #[cfg(target_os = "linux")]
    #[arg(env, long)]
    pub ble_interface: Option<String>,
    #[clap(subcommand)]
    pub subcommand: OpenWhoopCommand,
}

#[derive(Subcommand)]
pub enum OpenWhoopCommand {
    ///
    /// Scan for Whoop devices
    ///
    Scan,
    ///
    /// Download history data from whoop devices
    ///
    DownloadHistory {
        #[arg(long, env)]
        whoop: DeviceId,
    },
    ///
    /// Hold a BLE sync session and stream parsed heart-rate / metadata as JSON over WebSocket.
    /// Optional DATABASE_URL for remote DB (e.g., Supabase). Without it, runs in thin proxy mode.
    /// Set OPENWHOOP_STUDIO_BIND=0.0.0.0 for network access (Tailscale).
    ///
    LiveServer {
        #[arg(long, env)]
        whoop: DeviceId,
        #[arg(long, default_value_t = 3848)]
        port: u16,
    },
    ///
    /// Reruns the packet processing on stored packets
    /// This is used after new more of packets get handled
    ///
    ReRun,
    ///
    /// Detects sleeps and exercises
    ///
    DetectEvents,
    ///
    /// Print sleep statistics for all time and last week
    ///
    SleepStats,
    ///
    /// Print activity statistics for all time and last week
    ///
    ExerciseStats,
    ///
    /// Calculate stress for historical data
    ///
    CalculateStress,
    ///
    /// Calculate SpO2 from raw sensor data
    ///
    CalculateSpo2,
    ///
    /// Calculate skin temperature from raw sensor data
    ///
    CalculateSkinTemp,
    ///
    /// Set alarm
    ///
    SetAlarm {
        #[arg(long, env)]
        whoop: DeviceId,
        alarm_time: AlarmTime,
    },
    ///
    /// Get current alarm setting from device
    ///
    GetAlarm {
        #[arg(long, env)]
        whoop: DeviceId,
    },
    ///
    /// Copy packets from one database into another
    ///
    Merge { from: String },
    Restart {
        #[arg(long, env)]
        whoop: DeviceId,
    },
    ///
    /// Erase all history data from the device
    ///
    Erase {
        #[arg(long, env)]
        whoop: DeviceId,
    },
    ///
    /// Get device firmware version info
    ///
    Version {
        #[arg(long, env)]
        whoop: DeviceId,
    },
    ///
    /// Generate Shell completions
    ///
    Completions { shell: Shell },
    ///
    /// Enable IMU data
    ///
    EnableImu {
        #[arg(long, env)]
        whoop: DeviceId,
    },
    ///
    /// Sync data between local and remote database
    ///
    Sync {
        #[arg(long, env)]
        remote: String,
    },
    ///
    /// Download firmware from WHOOP API
    ///
    DownloadFirmware {
        #[arg(long, env = "WHOOP_EMAIL")]
        email: String,
        #[arg(long, env = "WHOOP_PASSWORD")]
        password: String,
        #[arg(long, default_value = "HARVARD")]
        device_name: String,
        #[arg(long, default_value = "41.16.5.0")]
        maxim: String,
        #[arg(long, default_value = "17.2.2.0")]
        nordic: String,
        #[arg(long, default_value = "./firmware")]
        output_dir: String,
    },
    ///
    /// Run the alarm scheduler (runs on AWS with remote database)
    /// Checks cron schedules and queues commands when laptop is offline
    ///
    Scheduler {
        /// Interval in seconds between scheduler checks (default: 60)
        #[arg(long, default_value_t = 60)]
        interval_secs: u64,
        /// Device ID to manage alarms for
        #[arg(long, env)]
        device_id: String,
        /// URL of the live-server to send commands to
        #[arg(long, default_value = "http://127.0.0.1:3848")]
        studio_url: String,
    },
    ///
    /// Agent CLI: control Whoop via HTTP API (requires live-server running)
    ///
    Agent {
        #[clap(subcommand)]
        command: AgentCommand,
    },
    ///
    /// Queue commands for offline laptop reconnection
    ///
    Queue {
        #[clap(subcommand)]
        command: QueueCommand,
    },
}

#[derive(Subcommand, Clone)]
pub enum AgentCommand {
    /// Trigger instant buzzer/haptic feedback on the Whoop
    Buzzer,
    /// Get current battery level
    Battery,
    /// Get current alarm setting
    GetAlarm,
    /// Set alarm (accepts same formats as set-alarm)
    SetAlarm { alarm_time: AlarmTime },
    /// Clear any set alarm
    ClearAlarm,
    /// List scheduled alarms from database
    ListAlarms,
    /// Create a new scheduled alarm (cron expression or one-time)
    CreateAlarm {
        label: String,
        /// Alarm kind: "cron" or "one-time"
        kind: String,
        /// Cron expression (e.g., "0 7 * * *") or Unix timestamp for one-time
        schedule: String,
    },
    /// Delete a scheduled alarm by ID
    DeleteAlarm { id: i32 },
}

#[derive(Subcommand, Clone)]
pub enum QueueCommand {
    /// List pending commands in the queue
    List {
        /// Device ID to filter by (optional)
        #[arg(long)]
        device_id: Option<String>,
    },
    /// Push a command to the queue
    Push {
        /// Device ID
        #[arg(long)]
        device_id: String,
        /// Command type: buzzer, set-alarm, clear-alarm
        #[arg(long)]
        command: String,
        /// Optional payload as JSON (e.g., '{"unix": 1234567890}')
        #[arg(long)]
        payload: Option<String>,
    },
    /// Process and send pending commands to live-server
    Process {
        /// Device ID
        #[arg(long)]
        device_id: String,
        /// URL of the live-server (default: http://127.0.0.1:3848)
        #[arg(long)]
        studio_url: Option<String>,
    },
    /// Clear failed commands
    ClearFailed {
        /// Device ID
        #[arg(long)]
        device_id: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if let Err(error) = dotenv() {
        println!("{}", error);
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("sqlx::query", log::LevelFilter::Off)
        .filter_module("sea_orm_migration::migrator", log::LevelFilter::Off)
        .filter_module("bluez_async", log::LevelFilter::Off)
        .filter_module("sqlx::postgres::notice", log::LevelFilter::Off)
        .init();

    OpenWhoopCli::parse().run().await
}

async fn download_firmware(
    email: &str,
    password: &str,
    device_name: &str,
    maxim: &str,
    nordic: &str,
    output_dir: &str,
) -> anyhow::Result<()> {
    info!("authenticating...");
    let client = api::WhoopApiClient::sign_in(email, password).await?;

    let chip_names = match device_name {
        "HARVARD" => vec!["MAXIM", "NORDIC"],
        "PUFFIN" => vec!["MAXIM", "NORDIC", "RUGGLES", "PEARL"],
        other => anyhow::bail!("unknown device family: {other}"),
    };

    let target_versions: std::collections::HashMap<&str, &str> =
        [("MAXIM", maxim), ("NORDIC", nordic)].into_iter().collect();

    let current: Vec<api::ChipFirmware> = chip_names
        .iter()
        .map(|c| api::ChipFirmware {
            chip_name: c.to_string(),
            version: "1.0.0.0".into(),
        })
        .collect();

    let upgrade: Vec<api::ChipFirmware> = chip_names
        .iter()
        .map(|c| api::ChipFirmware {
            chip_name: c.to_string(),
            version: target_versions.get(c).unwrap_or(&"1.0.0.0").to_string(),
        })
        .collect();

    info!("device: {device_name}");
    for uv in &upgrade {
        info!("  target {}: {}", uv.chip_name, uv.version);
    }

    info!("downloading firmware...");
    let fw_b64 = client
        .download_firmware(device_name, current, upgrade)
        .await?;

    api::decode_and_extract(&fw_b64, std::path::Path::new(output_dir))?;
    Ok(())
}

async fn scan_command(
    adapter: &Adapter,
    device_id: Option<DeviceId>,
) -> anyhow::Result<Peripheral> {
    if let Some(id) = device_id.as_ref() {
        info!(
            "Scanning for Whoop matching {:?} — keep the strap on and nearby (this can take a minute if it is sleeping)",
            id
        );
    }

    adapter
        .start_scan(ScanFilter {
            services: vec![WHOOP_SERVICE],
        })
        .await?;

    loop {
        let peripherals = adapter.peripherals().await?;

        for peripheral in peripherals {
            let Some(properties) = peripheral.properties().await? else {
                continue;
            };

            if !properties.services.contains(&WHOOP_SERVICE) {
                continue;
            }

            let Some(device_id) = device_id.as_ref() else {
                println!("Address: {}", properties.address);
                println!("Name: {:?}", properties.local_name);
                println!("RSSI: {:?}", properties.rssi);
                println!();
                continue;
            };

            #[cfg(target_os = "linux")]
            if properties.address == *device_id {
                return Ok(peripheral);
            }

            #[cfg(target_os = "macos")]
            {
                let Some(name) = properties.local_name else {
                    continue;
                };
                let name = sanitize_name(&name);
                // `starts_with`: e.g. WHOOP=WHOOP 4C0639073
                // `contains` (suffix length >= 6): e.g. WHOOP=4C0639073 when name is WHOOP 4C0639073
                let id = device_id.as_str();
                if name.starts_with(id) || (id.len() >= 6 && name.contains(id)) {
                    return Ok(peripheral);
                }
            }
        }

        sleep(Duration::from_secs(1)).await;
    }
}

async fn scan_command_with_timeout(
    adapter: &Adapter,
    device_id: DeviceId,
    wait: Duration,
) -> anyhow::Result<Option<Peripheral>> {
    match timeout(wait, scan_command(adapter, Some(device_id))).await {
        Ok(Ok(p)) => Ok(Some(p)),
        Ok(Err(e)) => Err(e),
        Err(_) => {
            let _ = adapter.stop_scan().await;
            Ok(None)
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum AlarmTime {
    DateTime(NaiveDateTime),
    Time(NaiveTime),
    Minute,
    Minute5,
    Minute10,
    Minute15,
    Minute30,
    Hour,
}

impl FromStr for AlarmTime {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(t) = s.parse() {
            return Ok(Self::DateTime(t));
        }

        if let Ok(t) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
            return Ok(Self::DateTime(t));
        }

        if let Ok(t) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
            return Ok(Self::DateTime(t));
        }

        if let Ok(t) = s.parse() {
            return Ok(Self::Time(t));
        }

        match s {
            "minute" | "1min" | "min" => Ok(Self::Minute),
            "5minute" | "5min" => Ok(Self::Minute5),
            "10minute" | "10min" => Ok(Self::Minute10),
            "15minute" | "15min" => Ok(Self::Minute15),
            "30minute" | "30min" => Ok(Self::Minute30),
            "hour" | "h" => Ok(Self::Hour),
            _ => Err(anyhow!("Invalid alarm time")),
        }
    }
}

impl AlarmTime {
    pub fn unix(self) -> DateTime<Utc> {
        let mut now = Utc::now();
        let timezone_df = Local::now().offset().to_owned();

        match self {
            AlarmTime::DateTime(dt) => dt.and_utc() - timezone_df,
            AlarmTime::Time(t) => {
                let current_time = now.time();
                if current_time > t {
                    now += TimeDelta::days(1);
                }

                now.with_time(t).unwrap() - timezone_df
            }
            _ => {
                let offset = self.offset();
                now + offset
            }
        }
    }

    fn offset(self) -> TimeDelta {
        match self {
            AlarmTime::DateTime(_) => todo!(),
            AlarmTime::Time(_) => todo!(),
            AlarmTime::Minute => TimeDelta::minutes(1),
            AlarmTime::Minute5 => TimeDelta::minutes(5),
            AlarmTime::Minute10 => TimeDelta::minutes(10),
            AlarmTime::Minute15 => TimeDelta::minutes(15),
            AlarmTime::Minute30 => TimeDelta::minutes(30),
            AlarmTime::Hour => TimeDelta::hours(1),
        }
    }
}

#[cfg(target_os = "macos")]
pub fn sanitize_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

impl OpenWhoopCli {
    async fn run(self) -> anyhow::Result<()> {
        if let OpenWhoopCommand::DownloadFirmware {
            email,
            password,
            device_name,
            maxim,
            nordic,
            output_dir,
        } = &self.subcommand
        {
            return download_firmware(email, password, device_name, maxim, nordic, output_dir)
                .await;
        }

        match &self.subcommand {
            OpenWhoopCommand::Completions { shell } => {
                let mut command = OpenWhoopCli::command();
                let bin_name = command.get_name().to_string();
                generate(*shell, &mut command, bin_name, &mut io::stdout());
                return Ok(());
            }
            OpenWhoopCommand::Scan => {
                let adapter = self.create_ble_adapter().await?;
                scan_command(&adapter, None).await?;
                return Ok(());
            }
            OpenWhoopCommand::LiveServer { .. } => {
                // LiveServer runs separately - has its own match arm later
                return Ok(());
            }
            OpenWhoopCommand::Scheduler { .. } => {
                // Scheduler runs separately - has its own match arm later
                return Ok(());
            }
            OpenWhoopCommand::DownloadFirmware { .. } => {
                // No BLE/DB needed - downloads from WHOOP API
                return Ok(());
            }
            _ => {}
        }

        // For other commands, DATABASE_URL is required
        let database_url = self.database_url.as_deref().ok_or_else(|| {
            anyhow!("DATABASE_URL or --database-url is required for this command")
        })?;

        let adapter = self.create_ble_adapter().await?;
        let db_handler = DatabaseHandler::new(database_url.to_owned()).await;

        match self.subcommand {
            OpenWhoopCommand::Scan | OpenWhoopCommand::Completions { .. } => {
                unreachable!("handled before database init")
            }
            OpenWhoopCommand::LiveServer { .. } => {
                unreachable!("handled before database init")
            }
            OpenWhoopCommand::Scheduler { .. } => {
                unreachable!("handled before database init")
            }
            OpenWhoopCommand::DownloadFirmware { .. } => {
                unreachable!("handled before database init")
            }
            OpenWhoopCommand::DownloadHistory { whoop } => {
                let peripheral = scan_command(&adapter, Some(whoop)).await?;
                let mut whoop =
                    WhoopDevice::new(peripheral, adapter, db_handler, self.debug_packets);

                let should_exit = Arc::new(AtomicBool::new(false));

                let se = should_exit.clone();
                ctrlc::set_handler(move || {
                    println!("Received CTRL+C!");
                    se.store(true, Ordering::SeqCst);
                })?;

                whoop.connect().await?;
                whoop.initialize().await?;

                let result = whoop.sync_history(should_exit).await;

                info!("Exiting...");
                if let Err(e) = result {
                    error!("{}", e);
                }

                loop {
                    if let Ok(true) = whoop.is_connected().await {
                        whoop
                            .send_command(WhoopPacket::exit_high_freq_sync())
                            .await?;
                        break;
                    } else {
                        whoop.connect().await?;
                        sleep(Duration::from_secs(1)).await;
                    }
                }
            }
            OpenWhoopCommand::LiveServer { whoop, port } => {
                // LiveServer requires DATABASE_URL for database operations
                // If not provided, use local SQLite file in current directory
                let db_url = std::env::var("DATABASE_URL")
                    .unwrap_or_else(|_| "sqlite:openwhoop.db".to_string());
                let db_handler = DatabaseHandler::new(db_url).await;

                let (live_tx, _) = broadcast::channel::<String>(512);
                let live_hr_snapshot = Arc::new(Mutex::new(None::<String>));
                let should_exit = Arc::new(AtomicBool::new(false));
                let se = should_exit.clone();
                ctrlc::set_handler(move || {
                    println!("Received CTRL+C!");
                    se.store(true, Ordering::SeqCst);
                })?;

                let (job_tx, mut job_rx) = mpsc::channel::<StudioDeviceJob>(32);
                let app_state = AppState {
                    db: Some(db_handler.clone()),
                    ws_tx: live_tx.clone(),
                    last_hr_json: live_hr_snapshot.clone(),
                    job_tx,
                };
                let _server = tokio::spawn(async move {
                    if let Err(e) = live_server::run(app_state, port).await {
                        error!("live server: {}", e);
                    }
                });
                let mut session_seq: u64 = 0;
                let mut scan_cycle: u32 = 0;

                while !should_exit.load(Ordering::SeqCst) {
                    scan_cycle = scan_cycle.saturating_add(1);
                    session_seq = session_seq.saturating_add(1);
                    let _ = live_tx.send(
                        serde_json::json!({
                            "type": "session_state",
                            "state": "searching",
                            "reason": "scan_cycle_start",
                            "state_seq": session_seq,
                            "scan_cycle": scan_cycle,
                            "received_at_ms": Utc::now().timestamp_millis(),
                        })
                        .to_string(),
                    );

                    let Some(peripheral) =
                        scan_command_with_timeout(&adapter, whoop.clone(), Duration::from_secs(60))
                            .await?
                    else {
                        session_seq = session_seq.saturating_add(1);
                        let _ = live_tx.send(
                            serde_json::json!({
                                "type": "session_state",
                                "state": "offline",
                                "reason": "no_device_seen_in_scan_window",
                                "state_seq": session_seq,
                                "scan_cycle": scan_cycle,
                                "received_at_ms": Utc::now().timestamp_millis(),
                            })
                            .to_string(),
                        );
                        sleep(Duration::from_secs(60)).await;
                        continue;
                    };

                    let openwhoop = OpenWhoop::new(db_handler.clone())
                        .with_live_broadcast(live_tx.clone())
                        .with_live_hr_snapshot(live_hr_snapshot.clone());
                    let mut device = WhoopDevice::with_openwhoop(
                        peripheral,
                        adapter.clone(),
                        openwhoop,
                        self.debug_packets,
                    );
                    let mut tracker = openwhoop::SessionTracker::default();
                    tracker.state_seq = session_seq;
                    device.emit_external_session_state(
                        &mut tracker,
                        openwhoop::SessionState::Connecting,
                        "device_found",
                    );

                    if let Err(e) = device.connect().await {
                        error!("connect failed: {}", e);
                        device.emit_external_session_state(
                            &mut tracker,
                            openwhoop::SessionState::Reconnecting,
                            "connect_failed",
                        );
                        sleep(Duration::from_secs(5)).await;
                        session_seq = tracker.state_seq;
                        continue;
                    }
                    let _ = live_tx.send(
                        serde_json::json!({
                            "type": "status",
                            "message": "ble_connected"
                        })
                        .to_string(),
                    );
                    if let Err(e) = device.initialize().await {
                        error!("initialize failed: {}", e);
                        device.emit_external_session_state(
                            &mut tracker,
                            openwhoop::SessionState::Reconnecting,
                            "initialize_failed",
                        );
                        sleep(Duration::from_secs(5)).await;
                        session_seq = tracker.state_seq;
                        continue;
                    }
                    let _ = live_tx.send(
                        serde_json::json!({
                            "type": "status",
                            "message": "ble_subscribed"
                        })
                        .to_string(),
                    );

                    let result = device
                        .sync_history_with_studio_jobs(
                            should_exit.clone(),
                            &mut job_rx,
                            &mut tracker,
                            scan_cycle,
                        )
                        .await;
                    session_seq = tracker.state_seq;

                    if let Ok(true) = device.is_connected().await {
                        let _ = device
                            .send_command(WhoopPacket::exit_high_freq_sync())
                            .await;
                    }

                    if should_exit.load(Ordering::SeqCst) {
                        break;
                    }

                    if let Err(e) = result {
                        error!("live session ended: {}", e);
                        device.emit_external_session_state(
                            &mut tracker,
                            openwhoop::SessionState::Reconnecting,
                            "session_error",
                        );
                        session_seq = tracker.state_seq;
                        sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                }
            }
            OpenWhoopCommand::ReRun => {
                let mut whoop = OpenWhoop::new(db_handler.clone());
                let mut id = 0;
                loop {
                    let packets = db_handler.get_packets(id).await?;
                    if packets.is_empty() {
                        break;
                    }

                    for packet in packets {
                        id = packet.id;
                        whoop.handle_packet(packet).await?;
                    }

                    println!("{}", id);
                }
            }
            OpenWhoopCommand::DetectEvents => {
                let whoop = OpenWhoop::new(db_handler);
                whoop.detect_sleeps().await?;
                whoop.detect_events().await?;
            }
            OpenWhoopCommand::SleepStats => {
                let whoop = OpenWhoop::new(db_handler);
                let sleep_records = whoop.database.get_sleep_cycles(None).await?;

                if sleep_records.is_empty() {
                    println!("No sleep records found, exiting now");
                    return Ok(());
                }

                let mut last_week = sleep_records
                    .iter()
                    .rev()
                    .take(7)
                    .copied()
                    .collect::<Vec<_>>();

                last_week.reverse();
                let analyzer = SleepConsistencyAnalyzer::new(sleep_records);
                let metrics = analyzer.calculate_consistency_metrics()?;
                println!("All time: \n{}", metrics);
                let analyzer = SleepConsistencyAnalyzer::new(last_week);
                let metrics = analyzer.calculate_consistency_metrics()?;
                println!("\nWeek: \n{}", metrics);
            }
            OpenWhoopCommand::ExerciseStats => {
                let whoop = OpenWhoop::new(db_handler);
                let exercises = whoop
                    .database
                    .search_activities(
                        SearchActivityPeriods::default().with_activity(ActivityType::Activity),
                    )
                    .await?;

                if exercises.is_empty() {
                    println!("No activities found, exiting now");
                    return Ok(());
                };

                let last_week = exercises
                    .iter()
                    .rev()
                    .take(7)
                    .copied()
                    .rev()
                    .collect::<Vec<_>>();

                let metrics = ExerciseMetrics::new(exercises)?;
                let last_week = ExerciseMetrics::new(last_week)?;

                println!("All time: \n{}", metrics);
                println!("Last week: \n{}", last_week);
            }
            OpenWhoopCommand::CalculateStress => {
                let whoop = OpenWhoop::new(db_handler);
                whoop.calculate_stress().await?;
            }
            OpenWhoopCommand::CalculateSpo2 => {
                let whoop = OpenWhoop::new(db_handler);
                whoop.calculate_spo2().await?;
            }
            OpenWhoopCommand::CalculateSkinTemp => {
                let whoop = OpenWhoop::new(db_handler);
                whoop.calculate_skin_temp().await?;
            }
            OpenWhoopCommand::SetAlarm { whoop, alarm_time } => {
                let peripheral = scan_command(&adapter, Some(whoop)).await?;
                let mut whoop =
                    WhoopDevice::new(peripheral, adapter, db_handler, self.debug_packets);
                whoop.connect().await?;

                let time = alarm_time.unix();
                let now = Utc::now();

                if time < now {
                    error!(
                        "Time {} is in past, current time: {}",
                        time.format("%Y-%m-%d %H:%M:%S"),
                        now.format("%Y-%m-%d %H:%M:%S")
                    );
                    return Ok(());
                }

                let packet = WhoopPacket::alarm_time(u32::try_from(time.timestamp())?);
                whoop.send_command(packet).await?;
                let time = time.with_timezone(&Local);

                println!("Alarm time set for: {}", time.format("%Y-%m-%d %H:%M:%S"));
            }
            OpenWhoopCommand::GetAlarm { whoop } => {
                let peripheral = scan_command(&adapter, Some(whoop)).await?;
                let mut whoop = WhoopDevice::new(peripheral, adapter, db_handler, false);
                whoop.connect().await?;
                let data = whoop.get_alarm().await?;
                if let openwhoop_codec::WhoopData::AlarmInfo { enabled, unix } = data {
                    if enabled {
                        let alarm_time = DateTime::from_timestamp(i64::from(unix), 0)
                            .ok_or_else(|| anyhow!("Invalid alarm timestamp"))?
                            .with_timezone(&Local);
                        println!(
                            "Alarm is set for: {}",
                            alarm_time.format("%Y-%m-%d %H:%M:%S")
                        );
                    } else {
                        println!("No alarm is currently set");
                    }
                } else {
                    error!("Unexpected response from device: {:?}", data);
                }
            }
            OpenWhoopCommand::Merge { from } => {
                let from_db = DatabaseHandler::new(from).await;

                let mut id = 0;
                loop {
                    let packets = from_db.get_packets(id).await?;
                    if packets.is_empty() {
                        break;
                    }

                    for packets::Model {
                        uuid,
                        bytes,
                        id: c_id,
                    } in packets
                    {
                        id = c_id;
                        db_handler.create_packet(uuid, bytes).await?;
                    }

                    println!("{}", id);
                }
            }
            OpenWhoopCommand::Restart { whoop } => {
                let peripheral = scan_command(&adapter, Some(whoop)).await?;
                let mut whoop =
                    WhoopDevice::new(peripheral, adapter, db_handler, self.debug_packets);
                whoop.connect().await?;
                whoop.send_command(WhoopPacket::restart()).await?;
            }
            OpenWhoopCommand::Erase { whoop } => {
                let peripheral = scan_command(&adapter, Some(whoop)).await?;
                let mut whoop =
                    WhoopDevice::new(peripheral, adapter, db_handler, self.debug_packets);
                whoop.connect().await?;
                whoop.send_command(WhoopPacket::erase()).await?;
                info!("Erase command sent - device will trim all stored history data");
            }
            OpenWhoopCommand::Version { whoop } => {
                let peripheral = scan_command(&adapter, Some(whoop)).await?;
                let mut whoop = WhoopDevice::new(peripheral, adapter, db_handler, false);
                whoop.connect().await?;
                whoop.get_version().await?;
            }
            OpenWhoopCommand::EnableImu { whoop } => {
                let peripheral = scan_command(&adapter, Some(whoop)).await?;
                let mut whoop = WhoopDevice::new(peripheral, adapter, db_handler, false);
                whoop.connect().await?;
                whoop
                    .send_command(WhoopPacket::toggle_r7_data_collection())
                    .await?;
            }
            OpenWhoopCommand::Sync { remote } => {
                let remote_db = DatabaseHandler::new(remote.clone()).await;
                let sync = openwhoop::db::sync::DatabaseSync::new(
                    db_handler.connection(),
                    remote_db.connection(),
                );
                sync.run().await?;
            }
            OpenWhoopCommand::Scheduler {
                interval_secs,
                device_id,
                studio_url,
            } => {
                let db_url = self
                    .database_url
                    .as_deref()
                    .ok_or_else(|| anyhow!("DATABASE_URL is required for scheduler"))?;
                let db = DatabaseHandler::new(db_url).await;
                Self::run_scheduler(interval_secs, &device_id, &studio_url, db).await?;
                return Ok(());
            }
            OpenWhoopCommand::Agent { ref command } => {
                self.run_agent_command(command.clone()).await?;
            }
            OpenWhoopCommand::Queue { ref command } => {
                self.run_queue_command(command.clone()).await?;
            }
        }

        Ok(())
    }

    async fn run_agent_command(&self, command: AgentCommand) -> anyhow::Result<()> {
        let studio_url = std::env::var("OPENWHOOP_STUDIO_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:3848".to_string());
        let client = reqwest::Client::new();

        // Connection check - verify studio is reachable before sending commands
        match client.get(format!("{}/health", studio_url)).send().await {
            Ok(resp) if resp.status().is_success() => {
                info!("Connected to live-server at {}", studio_url);
            }
            Ok(resp) => {
                anyhow::bail!(
                    "live-server at {} returned HTTP {}",
                    studio_url,
                    resp.status()
                );
            }
            Err(e) => {
                anyhow::bail!(
                    "Cannot reach live-server at {}: {}. Is it running with OPENWHOOP_STUDIO_BIND=0.0.0.0?",
                    studio_url,
                    e
                );
            }
        };

        match command {
            AgentCommand::Buzzer => {
                let resp = client
                    .post(format!("{}/api/device/buzzer", studio_url))
                    .send()
                    .await?;
                let json: serde_json::Value = resp.json().await?;
                println!("{}", serde_json::to_string_pretty(&json)?);
            }
            AgentCommand::Battery => {
                let resp = client
                    .post(format!("{}/api/device/battery", studio_url))
                    .send()
                    .await?;
                let json: serde_json::Value = resp.json().await?;
                println!("{}", serde_json::to_string_pretty(&json)?);
            }
            AgentCommand::GetAlarm => {
                let resp = client
                    .get(format!("{}/api/device/alarm", studio_url))
                    .send()
                    .await?;
                let json: serde_json::Value = resp.json().await?;
                println!("{}", serde_json::to_string_pretty(&json)?);
            }
            AgentCommand::SetAlarm { alarm_time } => {
                let time = alarm_time.unix();
                let now = Utc::now();
                if time < now {
                    anyhow::bail!("Time {} is in past", time.format("%Y-%m-%d %H:%M:%S"));
                }
                let unix = u32::try_from(time.timestamp())?;
                let resp = client
                    .post(format!("{}/api/device/alarm", studio_url))
                    .json(&serde_json::json!({ "unix": unix }))
                    .send()
                    .await?;
                let json: serde_json::Value = resp.json().await?;
                println!("{}", serde_json::to_string_pretty(&json)?);
            }
            AgentCommand::ClearAlarm => {
                let resp = client
                    .post(format!("{}/api/device/alarm/clear", studio_url))
                    .send()
                    .await?;
                let json: serde_json::Value = resp.json().await?;
                println!("{}", serde_json::to_string_pretty(&json)?);
            }
            AgentCommand::ListAlarms => {
                let resp = client
                    .get(format!("{}/api/alarms", studio_url))
                    .send()
                    .await?;
                let json: serde_json::Value = resp.json().await?;
                println!("{}", serde_json::to_string_pretty(&json)?);
            }
            AgentCommand::CreateAlarm {
                label,
                kind,
                schedule,
            } => {
                let (cron_expr, one_time_unix) = match kind.as_str() {
                    "cron" => (Some(schedule.clone()), None),
                    "one-time" => {
                        let ts: i64 = schedule.parse()?;
                        (None, Some(ts))
                    }
                    _ => anyhow::bail!("kind must be 'cron' or 'one-time'"),
                };
                let resp = client
                    .post(format!("{}/api/alarms", studio_url))
                    .json(&serde_json::json!({
                        "label": label,
                        "kind": kind,
                        "cron_expr": cron_expr,
                        "one_time_unix": one_time_unix,
                    }))
                    .send()
                    .await?;
                let json: serde_json::Value = resp.json().await?;
                println!("{}", serde_json::to_string_pretty(&json)?);
            }
            AgentCommand::DeleteAlarm { id } => {
                let resp = client
                    .delete(format!("{}/api/alarms/{}", studio_url, id))
                    .send()
                    .await?;
                let json: serde_json::Value = resp.json().await?;
                println!("{}", serde_json::to_string_pretty(&json)?);
            }
        }
        Ok(())
    }

    async fn run_queue_command(&self, command: QueueCommand) -> anyhow::Result<()> {
        let database_url = self.database_url.as_deref().ok_or_else(|| {
            anyhow!("DATABASE_URL or --database-url is required for queue commands")
        })?;
        let db = DatabaseHandler::new(database_url.to_owned()).await;

        match command {
            QueueCommand::List { device_id } => match device_id {
                Some(id) => {
                    let pending = db.list_pending_commands(&id).await?;
                    println!("Pending commands for device '{}':", id);
                    for cmd in pending {
                        println!("  [{}] {} - {:?}", cmd.id, cmd.command_type, cmd.payload);
                    }
                }
                None => {
                    println!("Listing all pending commands requires device_id for now");
                }
            },
            QueueCommand::Push {
                device_id,
                command,
                payload,
            } => {
                let payload_json: Option<serde_json::Value> =
                    payload.as_ref().and_then(|p| serde_json::from_str(p).ok());
                let id = db.push_command(&device_id, &command, payload_json).await?;
                println!("Queued command {} (id={})", command, id);
            }
            QueueCommand::Process {
                device_id,
                studio_url,
            } => {
                let url = studio_url.unwrap_or_else(|| "http://127.0.0.1:3848".to_string());

                // Connection check before processing queue
                let client = reqwest::Client::new();
                match client.get(format!("{}/health", url)).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        info!("Connected to live-server at {}", url);
                    }
                    Ok(resp) => {
                        println!(
                            "Warning: live-server at {} returned HTTP {}",
                            url,
                            resp.status()
                        );
                    }
                    Err(e) => {
                        println!("Warning: Cannot reach live-server at {}: {}", url, e);
                        println!("Commands will remain queued until connection is restored.");
                    }
                }

                let pending = db.list_pending_commands(&device_id).await?;

                if pending.is_empty() {
                    println!("No pending commands for device '{}'", device_id);
                    return Ok(());
                }

                println!(
                    "Processing {} pending commands for device '{}'...",
                    pending.len(),
                    device_id
                );

                for cmd in pending {
                    let result = match cmd.command_type.as_str() {
                        "buzzer" => {
                            client
                                .post(format!("{}/api/device/buzzer", url))
                                .send()
                                .await
                        }
                        "set-alarm" => {
                            let payload = cmd.payload.as_ref();
                            let unix = payload.and_then(|p| p.get("unix")).and_then(|v| v.as_u64());
                            match unix {
                                Some(u) => {
                                    client
                                        .post(format!("{}/api/device/alarm", url))
                                        .json(&serde_json::json!({ "unix": u32::try_from(u)? }))
                                        .send()
                                        .await
                                }
                                None => {
                                    println!("Skipping set-alarm: missing unix in payload");
                                    continue;
                                }
                            }
                        }
                        "clear-alarm" => {
                            client
                                .post(format!("{}/api/device/alarm/clear", url))
                                .send()
                                .await
                        }
                        _ => {
                            println!("Unknown command type: {}", cmd.command_type);
                            continue;
                        }
                    };

                    match result {
                        Ok(resp) if resp.status().is_success() => {
                            db.mark_command_sent(cmd.id).await?;
                            println!("  [{}] {} - sent successfully", cmd.id, cmd.command_type);
                        }
                        Ok(resp) => {
                            let err = format!("HTTP {}", resp.status());
                            db.mark_command_failed(cmd.id, &err).await?;
                            println!("  [{}] {} - failed: {}", cmd.id, cmd.command_type, err);
                        }
                        Err(e) => {
                            db.mark_command_failed(cmd.id, &e.to_string()).await?;
                            println!("  [{}] {} - error: {}", cmd.id, cmd.command_type, e);
                        }
                    }
                }
            }
            QueueCommand::ClearFailed { device_id } => {
                println!(
                    "Clearing failed commands for device '{}' (not implemented yet)",
                    device_id
                );
            }
        }
        Ok(())
    }

    async fn run_scheduler(
        interval_secs: u64,
        device_id: &str,
        studio_url: &str,
        db_handler: DatabaseHandler,
    ) -> anyhow::Result<()> {
        info!(
            "Starting scheduler (interval: {}s, device: {}, studio: {})",
            interval_secs, device_id, studio_url
        );

        let client = reqwest::Client::new();

        loop {
            let now_unix = Utc::now().timestamp();

            // Check for due alarms in the database
            let due_alarms = db_handler.advance_due_alarm_schedules(now_unix).await?;

            for (schedule_id, next_unix) in due_alarms {
                // Queue the alarm command
                let payload = serde_json::json!({ "unix": next_unix, "schedule_id": schedule_id });
                let _ = db_handler
                    .push_command(device_id, "set-alarm", Some(payload))
                    .await;
                info!(
                    "Queued alarm for schedule {} at unix {}",
                    schedule_id, next_unix
                );
            }

            // Try to process pending queue (send to live-server)
            let pending = db_handler.list_pending_commands(device_id).await?;
            if !pending.is_empty() {
                for cmd in &pending {
                    let result = match cmd.command_type.as_str() {
                        "set-alarm" => {
                            if let Some(payload) = &cmd.payload {
                                if let Some(unix) = payload.get("unix").and_then(|v| v.as_u64()) {
                                    client
                                        .post(format!("{}/api/device/alarm", studio_url))
                                        .json(&serde_json::json!({ "unix": u32::try_from(unix)? }))
                                        .send()
                                        .await
                                } else {
                                    continue;
                                }
                            } else {
                                continue;
                            }
                        }
                        "buzzer" => {
                            client
                                .post(format!("{}/api/device/buzzer", studio_url))
                                .send()
                                .await
                        }
                        "clear-alarm" => {
                            client
                                .post(format!("{}/api/device/alarm/clear", studio_url))
                                .send()
                                .await
                        }
                        _ => continue,
                    };

                    match result {
                        Ok(resp) if resp.status().is_success() => {
                            db_handler.mark_command_sent(cmd.id).await?;
                            info!("Sent command {} to live-server", cmd.command_type);
                        }
                        Ok(resp) => {
                            let err = format!("HTTP {}", resp.status());
                            db_handler.mark_command_failed(cmd.id, &err).await?;
                            warn!("Failed to send command {}: {}", cmd.command_type, err);
                        }
                        Err(e) => {
                            db_handler
                                .mark_command_failed(cmd.id, &e.to_string())
                                .await?;
                            warn!("Error sending command {}: {}", cmd.command_type, e);
                        }
                    }
                }
            }

            tokio::time::sleep(Duration::from_secs(interval_secs)).await;
        }
    }

    async fn create_ble_adapter(&self) -> anyhow::Result<Adapter> {
        let manager = Manager::new().await?;

        #[cfg(target_os = "linux")]
        match self.ble_interface.as_ref() {
            Some(interface) => Self::adapter_from_name(&manager, interface).await,
            None => Self::default_adapter(&manager).await,
        }

        #[cfg(target_os = "macos")]
        Self::default_adapter(&manager).await
    }

    #[cfg(target_os = "linux")]
    async fn adapter_from_name(manager: &Manager, interface: &str) -> anyhow::Result<Adapter> {
        let adapters = manager.adapters().await?;
        let mut c_adapter = Err(anyhow!("Adapter: `{}` not found", interface));
        for adapter in adapters {
            let name = adapter.adapter_info().await?;
            if name.starts_with(interface) {
                c_adapter = Ok(adapter);
                break;
            }
        }

        c_adapter
    }

    async fn default_adapter(manager: &Manager) -> anyhow::Result<Adapter> {
        let adapters = manager.adapters().await?;
        adapters
            .into_iter()
            .next()
            .ok_or(anyhow!("No BLE adapters found"))
    }
}
