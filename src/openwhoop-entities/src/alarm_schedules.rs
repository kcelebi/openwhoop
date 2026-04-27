//! `SeaORM` Entity, @generated style.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "alarm_schedules")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub label: String,
    pub kind: String,
    pub cron_expr: Option<String>,
    pub one_time_unix: Option<i64>,
    pub next_unix: Option<i64>,
    pub last_rang_unix: Option<i64>,
    pub last_sent_unix: Option<i64>,
    pub enabled: bool,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
