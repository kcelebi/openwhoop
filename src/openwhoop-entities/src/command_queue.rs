//! `SeaORM` Entity for command queue.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "command_queue")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub device_id: String,
    pub command_type: String,
    pub payload: Option<Json>,
    pub status: String,
    pub created_at: DateTime,
    pub sent_at: Option<DateTime>,
    pub error: Option<String>,
    pub retry_count: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
