use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AlarmSchedules::Table)
                    .add_column(
                        ColumnDef::new(AlarmSchedules::Timezone)
                            .string()
                            .not_null()
                            .default("UTC"),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(AlarmSchedules::Table)
                    .add_column(
                        ColumnDef::new(AlarmSchedules::LastMissedUnix)
                            .big_integer()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(AlarmSchedules::Table)
                    .add_column(
                        ColumnDef::new(AlarmSchedules::LastUnconfirmedUnix)
                            .big_integer()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AlarmSchedules::Table)
                    .drop_column(AlarmSchedules::LastUnconfirmedUnix)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(AlarmSchedules::Table)
                    .drop_column(AlarmSchedules::LastMissedUnix)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(AlarmSchedules::Table)
                    .drop_column(AlarmSchedules::Timezone)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum AlarmSchedules {
    Table,
    Timezone,
    LastMissedUnix,
    LastUnconfirmedUnix,
}
