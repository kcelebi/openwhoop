//! Messages between the live BLE loop and the Studio HTTP API (local dashboard).

use serde_json::Value;
use tokio::sync::oneshot;

#[derive(Debug)]
pub enum StudioDeviceJob {
    PollBattery {
        reply: oneshot::Sender<Result<Value, String>>,
    },
    GetAlarm {
        reply: oneshot::Sender<Result<Value, String>>,
    },
    /// Set strap alarm to Unix seconds (UTC).
    SetAlarm {
        unix: u32,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    ClearAlarm {
        reply: oneshot::Sender<Result<Value, String>>,
    },
    /// Same as CLI `enable-imu` — toggles R7 high-rate collection (large packets when on).
    ToggleImuCollection {
        reply: oneshot::Sender<Result<Value, String>>,
    },
    ToggleImuMode {
        enable: bool,
        historical: bool,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    /// Trigger instant buzzer/haptic feedback on the device
    Buzzer {
        reply: oneshot::Sender<Result<Value, String>>,
    },
    /// Stop any active buzzer/haptic feedback
    StopBuzzer {
        reply: oneshot::Sender<Result<Value, String>>,
    },
}
