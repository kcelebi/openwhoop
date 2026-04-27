use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, mpsc};

use openwhoop::{StudioDeviceJob, db::DatabaseHandler};

/// Shared state for the Studio HTTP + WebSocket server (`live-server`).
/// The database is optional - when connecting to a remote DB (e.g., Supabase/Postgres on AWS),
/// alarm CRUD operations work. When no DB is provided, alarm endpoints return 503.
#[derive(Clone)]
pub struct AppState {
    pub db: Option<DatabaseHandler>,
    pub ws_tx: broadcast::Sender<String>,
    pub last_hr_json: Arc<Mutex<Option<String>>>,
    pub job_tx: mpsc::Sender<StudioDeviceJob>,
}

impl AppState {
    pub fn new(
        db: Option<DatabaseHandler>,
        ws_tx: broadcast::Sender<String>,
        last_hr_json: Arc<Mutex<Option<String>>>,
        job_tx: mpsc::Sender<StudioDeviceJob>,
    ) -> Self {
        Self {
            db,
            ws_tx,
            last_hr_json,
            job_tx,
        }
    }

    /// Convenience constructor when no database is needed (thin proxy mode).
    pub fn with_db(db: DatabaseHandler) -> Self {
        let (ws_tx, _) = broadcast::channel(512);
        let last_hr_json = Arc::new(Mutex::new(None));
        let (job_tx, _) = mpsc::channel(32);
        Self {
            db: Some(db),
            ws_tx,
            last_hr_json,
            job_tx,
        }
    }
}
