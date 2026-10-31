use sea_orm_migration::prelude::*;
#[derive(DeriveMigrationName)]
pub struct Migration;
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.create_table(
            Table::create()
                .table(Telemetry::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(Telemetry::Id)
                        .string()
                        .primary_key()
                        .not_null(),
                )
                .col(ColumnDef::new(Telemetry::Kind).string().not_null())
                .col(ColumnDef::new(Telemetry::RecordedAtMs).big_integer())
                .col(
                    ColumnDef::new(Telemetry::ReceivedAtMs)
                        .big_integer()
                        .not_null(),
                )
                .col(ColumnDef::new(Telemetry::Payload).text().not_null())
                .col(
                    ColumnDef::new(Telemetry::Synced)
                        .boolean()
                        .not_null()
                        .default(false),
                )
                .to_owned(),
        )
        .await?;
        m.create_index(
            Index::create()
                .name("telemetry_pending")
                .table(Telemetry::Table)
                .col(Telemetry::Synced)
                .col(Telemetry::ReceivedAtMs)
                .to_owned(),
        )
        .await?;
        m.create_index(
            Index::create()
                .name("telemetry_readings_time")
                .table(Telemetry::Table)
                .col(Telemetry::Kind)
                .col(Telemetry::RecordedAtMs)
                .to_owned(),
        )
        .await
    }
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.drop_table(Table::drop().table(Telemetry::Table).to_owned())
            .await
    }
}
#[derive(Iden)]
enum Telemetry {
    #[iden = "telemetry_outbox"]
    Table,
    Id,
    Kind,
    RecordedAtMs,
    ReceivedAtMs,
    Payload,
    Synced,
}
