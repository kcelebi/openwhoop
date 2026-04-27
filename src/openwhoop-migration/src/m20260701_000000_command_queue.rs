use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(CommandQueue::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(CommandQueue::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(CommandQueue::DeviceId).string_len(50).not_null())
                    .col(ColumnDef::new(CommandQueue::CommandType).string_len(32).not_null())
                    .col(ColumnDef::new(CommandQueue::Payload).json().null())
                    .col(
                        ColumnDef::new(CommandQueue::Status)
                            .string_len(16)
                            .not_null()
                            .default("pending"),
                    )
                    .col(ColumnDef::new(CommandQueue::CreatedAt).date_time().not_null())
                    .col(ColumnDef::new(CommandQueue::SentAt).date_time().null())
                    .col(ColumnDef::new(CommandQueue::Error).string().null())
                    .col(ColumnDef::new(CommandQueue::RetryCount).integer().not_null().default(0))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(CommandQueue::Table).to_owned())
            .await
    }
}

#[derive(Iden)]
enum CommandQueue {
    Table,
    Id,
    DeviceId,
    CommandType,
    Payload,
    Status,
    CreatedAt,
    SentAt,
    Error,
    RetryCount,
}