use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Alarm::Table)
                    .add_column(ColumnDef::new(Alarm::CloudId).uuid())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("alarm_schedules_cloud_id_unique")
                    .table(Alarm::Table)
                    .col(Alarm::CloudId)
                    .unique()
                    .to_owned(),
            )
            .await
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("alarm_schedules_cloud_id_unique")
                    .table(Alarm::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Alarm::Table)
                    .drop_column(Alarm::CloudId)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum Alarm {
    #[iden = "alarm_schedules"]
    Table,
    CloudId,
}
