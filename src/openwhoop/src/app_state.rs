use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};

use openwhoop::{StudioDeviceJob, db::DatabaseHandler};

#[derive(Clone)]
pub struct AppState {
    pub db: Option<DatabaseHandler>,
    pub ws_tx: broadcast::Sender<String>,
    pub last_hr_json: Arc<std::sync::Mutex<Option<String>>>,
    pub job_tx: mpsc::Sender<StudioDeviceJob>,
}

impl AppState {
    #[allow(dead_code)]
    pub fn new(
        db: Option<DatabaseHandler>,
        ws_tx: broadcast::Sender<String>,
        last_hr_json: Arc<std::sync::Mutex<Option<String>>>,
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
        let last_hr_json = Arc::new(std::sync::Mutex::new(None));
        let (job_tx, _) = mpsc::channel(32);
        Self {
            db: Some(db),
            ws_tx,
            last_hr_json,
            job_tx,
        }
    }
}
