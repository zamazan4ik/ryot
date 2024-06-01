//! `SeaORM` Entity. Generated by sea-orm-codegen 0.12.2

use async_trait::async_trait;
use chrono::NaiveDate;
use nanoid::nanoid;
use sea_orm::{entity::prelude::*, ActiveValue};
use serde::{Deserialize, Serialize};

use crate::models::media::{SeenPodcastExtraInformation, SeenShowExtraInformation};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "calendar_event")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub date: NaiveDate,
    pub metadata_id: Option<String>,
    pub metadata_show_extra_information: Option<SeenShowExtraInformation>,
    pub metadata_podcast_extra_information: Option<SeenPodcastExtraInformation>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::metadata::Entity",
        from = "Column::MetadataId",
        to = "super::metadata::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    Metadata,
}

impl Related<super::metadata::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Metadata.def()
    }
}

#[async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(mut self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        if insert {
            self.id = ActiveValue::Set(format!("cal_{}", nanoid!(12)));
        }
        Ok(self)
    }
}
