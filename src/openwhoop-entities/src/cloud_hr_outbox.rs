use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "cloud_hr_outbox")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub recorded_at_ms: i64,
    pub bpm: i16,
    pub received_at_ms: i64,
    pub synced: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
