use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Outbox::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Outbox::RecordedAtMs)
                            .big_integer()
                            .primary_key()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Outbox::Bpm).small_integer().not_null())
                    .col(
                        ColumnDef::new(Outbox::ReceivedAtMs)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Outbox::Synced)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Outbox::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum Outbox {
    #[iden = "cloud_hr_outbox"]
    Table,
    RecordedAtMs,
    Bpm,
    ReceivedAtMs,
    Synced,
}
