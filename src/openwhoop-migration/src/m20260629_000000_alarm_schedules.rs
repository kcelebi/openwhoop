use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AlarmSchedules::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AlarmSchedules::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AlarmSchedules::Label).string().not_null())
                    .col(
                        ColumnDef::new(AlarmSchedules::Kind)
                            .string_len(16)
                            .not_null(),
                    )
                    .col(ColumnDef::new(AlarmSchedules::CronExpr).string().null())
                    .col(
                        ColumnDef::new(AlarmSchedules::OneTimeUnix)
                            .big_integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(AlarmSchedules::NextUnix)
                            .big_integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(AlarmSchedules::LastRangUnix)
                            .big_integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(AlarmSchedules::LastSentUnix)
                            .big_integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(AlarmSchedules::Enabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(AlarmSchedules::CreatedAt)
                            .date_time()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AlarmSchedules::UpdatedAt)
                            .date_time()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AlarmSchedules::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum AlarmSchedules {
    Table,
    Id,
    Label,
    Kind,
    CronExpr,
    OneTimeUnix,
    NextUnix,
    LastRangUnix,
    LastSentUnix,
    Enabled,
    CreatedAt,
    UpdatedAt,
}
