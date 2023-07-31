//! `SeaORM` Entity. Generated by sea-orm-codegen 0.11.3

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "metadata_to_creator")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub metadata_id: i32,
    #[sea_orm(primary_key, auto_increment = false)]
    pub creator_id: i32,
    pub role: String,
    pub index: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::creator::Entity",
        from = "Column::CreatorId",
        to = "super::creator::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    Creator,
    #[sea_orm(
        belongs_to = "super::metadata::Entity",
        from = "Column::MetadataId",
        to = "super::metadata::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    Metadata,
}

impl Related<super::creator::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Creator.def()
    }
}

impl Related<super::metadata::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Metadata.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
