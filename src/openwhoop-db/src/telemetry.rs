use crate::DatabaseHandler;
use sea_orm::{ConnectionTrait, Statement};
use serde_json::Value;

pub struct TelemetryRow {
    pub id: String,
    pub kind: String,
    pub recorded_at_ms: Option<i64>,
    pub received_at_ms: i64,
    pub payload: Value,
    pub payload_text: String,
}
impl DatabaseHandler {
    pub async fn enqueue_telemetry(
        &self,
        id: String,
        kind: &str,
        recorded_at_ms: Option<i64>,
        payload: Value,
    ) -> anyhow::Result<()> {
        self.connection().execute(Statement::from_sql_and_values(self.connection().get_database_backend(),
            "INSERT INTO telemetry_outbox(id,kind,recorded_at_ms,received_at_ms,payload,synced) VALUES($1,$2,$3,$4,$5,false) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload,synced=false WHERE telemetry_outbox.payload != excluded.payload",
            [id.into(),kind.into(),recorded_at_ms.into(),chrono::Utc::now().timestamp_millis().into(),payload.to_string().into()])).await?;
        Ok(())
    }
    pub async fn pending_telemetry(&self, limit: u64) -> anyhow::Result<Vec<TelemetryRow>> {
        let rows=self.connection().query_all(Statement::from_sql_and_values(self.connection().get_database_backend(),
            "SELECT id,kind,recorded_at_ms,received_at_ms,payload FROM telemetry_outbox WHERE synced=false ORDER BY received_at_ms LIMIT $1",[(limit as i64).into()])).await?;
        rows.into_iter()
            .map(|r| {
                Ok(TelemetryRow {
                    id: r.try_get("", "id")?,
                    kind: r.try_get("", "kind")?,
                    recorded_at_ms: r.try_get("", "recorded_at_ms")?,
                    received_at_ms: r.try_get("", "received_at_ms")?,
                    payload: serde_json::from_str(&r.try_get::<String>("", "payload")?)?,
                    payload_text: r.try_get("", "payload")?,
                })
            })
            .collect()
    }
    pub async fn acknowledge_telemetry(&self, rows: &[TelemetryRow]) -> anyhow::Result<()> {
        for row in rows {
            // A richer decoded reading may have replaced this row during upload.
            self.connection()
                .execute(Statement::from_sql_and_values(
                    self.connection().get_database_backend(),
                    "UPDATE telemetry_outbox SET synced=true WHERE id=$1 AND payload=$2",
                    [row.id.clone().into(), row.payload_text.clone().into()],
                ))
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn telemetry_keeps_raw_bytes_and_does_not_ack_newer_enrichment() {
        let db = DatabaseHandler::new("sqlite::memory:").await;
        db.enqueue_telemetry(
            "raw".into(),
            "raw_ble",
            None,
            json!({"bytes":[0,255,13,10],"characteristic":"unknown"}),
        )
        .await
        .unwrap();
        db.enqueue_telemetry("reading:1".into(), "reading", Some(1), json!({"bpm":60}))
            .await
            .unwrap();
        let previous = db.pending_telemetry(100).await.unwrap();
        assert_eq!(
            previous.iter().find(|r| r.id == "raw").unwrap().payload["bytes"],
            json!([0, 255, 13, 10])
        );
        db.enqueue_telemetry(
            "reading:1".into(),
            "reading",
            Some(1),
            json!({"bpm":60,"rr_ms":[1000],"sensor_data":{"skin_temp_raw":123}}),
        )
        .await
        .unwrap();
        db.acknowledge_telemetry(&previous).await.unwrap();
        let pending = db.pending_telemetry(100).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].payload["rr_ms"], json!([1000]));
        db.acknowledge_telemetry(&pending).await.unwrap();
        assert!(db.pending_telemetry(100).await.unwrap().is_empty());
        // Idempotent reprocessing must not upload identical normalized rows again.
        db.enqueue_telemetry(
            "reading:1".into(),
            "reading",
            Some(1),
            pending[0].payload.clone(),
        )
        .await
        .unwrap();
        assert!(db.pending_telemetry(100).await.unwrap().is_empty());
    }
}
