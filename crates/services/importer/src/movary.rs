use async_graphql::Result;
use chrono::NaiveDate;
use common_models::DefaultCollection;
use common_utils::convert_naive_to_utc;
use csv::Reader;
use dependent_models::ImportResult;
use enums::{ImportSource, MediaLot, MediaSource};
use media_models::{
    DeployMovaryImportInput, ImportOrExportItemRating, ImportOrExportItemReview,
    ImportOrExportMediaItemSeen,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

use super::{ImportFailStep, ImportFailedItem, ImportOrExportMediaItem};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Common {
    title: String,
    tmdb_id: i32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rating {
    #[serde(flatten)]
    common: Common,
    user_rating: Decimal,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct History {
    #[serde(flatten)]
    common: Common,
    watched_at: NaiveDate,
    comment: Option<String>,
}

pub async fn import(input: DeployMovaryImportInput) -> Result<ImportResult> {
    let lot = MediaLot::Movie;
    let source = MediaSource::Tmdb;
    let mut media = vec![];
    let mut failed_items = vec![];
    let mut ratings_reader = Reader::from_path(input.ratings).unwrap();
    for (idx, result) in ratings_reader.deserialize().enumerate() {
        let record: Rating = match result {
            Ok(r) => r,
            Err(e) => {
                failed_items.push(ImportFailedItem {
                    lot: Some(lot),
                    step: ImportFailStep::InputTransformation,
                    identifier: idx.to_string(),
                    error: Some(format!("Ratings file: {:#?}", e)),
                });
                continue;
            }
        };
        media.push(ImportOrExportMediaItem {
            source_id: record.common.title.clone(),
            lot,
            source,
            identifier: record.common.tmdb_id.to_string(),
            reviews: vec![ImportOrExportItemRating {
                // DEV: Rates items out of 10
                rating: Some(record.user_rating.saturating_mul(dec!(10))),
                ..Default::default()
            }],
            ..Default::default()
        })
    }
    let mut watchlist_reader = Reader::from_path(input.watchlist).unwrap();
    for (idx, result) in watchlist_reader.deserialize().enumerate() {
        let record: Common = match result {
            Ok(r) => r,
            Err(e) => {
                failed_items.push(ImportFailedItem {
                    lot: Some(lot),
                    step: ImportFailStep::InputTransformation,
                    identifier: idx.to_string(),
                    error: Some(format!("Watchlist file: {:#?}", e)),
                });
                continue;
            }
        };
        media.push(ImportOrExportMediaItem {
            source_id: record.title.clone(),
            lot,
            source,
            identifier: record.tmdb_id.to_string(),
            collections: vec![DefaultCollection::Watchlist.to_string()],
            ..Default::default()
        })
    }
    let mut history_reader = Reader::from_path(input.history).unwrap();
    for (idx, result) in history_reader.deserialize().enumerate() {
        let record: History = match result {
            Ok(r) => r,
            Err(e) => {
                failed_items.push(ImportFailedItem {
                    lot: Some(lot),
                    step: ImportFailStep::InputTransformation,
                    identifier: idx.to_string(),
                    error: Some(format!("History file: {:#?}", e)),
                });
                continue;
            }
        };
        let watched_at = Some(convert_naive_to_utc(record.watched_at));
        let seen_item = ImportOrExportMediaItemSeen {
            started_on: None,
            ended_on: watched_at.map(|d| d.date_naive()),
            provider_watched_on: Some(ImportSource::Movary.to_string()),
            ..Default::default()
        };
        let review = record.comment.map(|c| ImportOrExportItemReview {
            spoiler: Some(false),
            text: Some(c),
            date: watched_at,
            visibility: None,
        });
        if let Some(media) = media
            .iter_mut()
            .find(|m| m.source_id == record.common.title)
        {
            if review.is_some() {
                if let Some(rating) = media.reviews.last_mut() {
                    rating.review = review;
                } else {
                    media.reviews.push(ImportOrExportItemRating {
                        review,
                        ..Default::default()
                    })
                }
            }
            media.seen_history.push(seen_item);
        } else {
            let mut reviews = vec![];
            if review.is_some() {
                reviews.push(ImportOrExportItemRating {
                    review,
                    ..Default::default()
                })
            }
            media.push(ImportOrExportMediaItem {
                source_id: record.common.title.clone(),
                lot,
                source,
                identifier: record.common.tmdb_id.to_string(),
                seen_history: vec![seen_item],
                reviews,
                ..Default::default()
            })
        }
    }
    Ok(ImportResult {
        media,
        failed_items,
        ..Default::default()
    })
}
