use anyhow::{bail, Result};
use apalis::prelude::MemoryStorage;
use background::ApplicationJob;
use common_utils::ryot_log;
use database_models::{
    prelude::{Exercise, UserToEntity, Workout},
    user_to_entity, workout,
};
use database_utils::deploy_job_to_re_evaluate_user_workouts;
use fitness_models::{
    ExerciseBestSetRecord, ProcessedExercise, UserToExerciseBestSetExtraInformation,
    UserToExerciseExtraInformation, UserToExerciseHistoryExtraInformation, UserWorkoutInput,
    WorkoutInformation, WorkoutOrExerciseTotals, WorkoutSetPersonalBest, WorkoutSetRecord,
    WorkoutSetTotals, WorkoutSummary, WorkoutSummaryExercise,
};
use nanoid::nanoid;
use rust_decimal_macros::dec;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, ModelTrait,
    QueryFilter,
};

use super::LOT_MAPPINGS;

fn get_best_set_index(records: &[WorkoutSetRecord]) -> Option<usize> {
    records
        .iter()
        .enumerate()
        .max_by_key(|(_, record)| {
            record.statistic.duration.unwrap_or(dec!(0))
                + record.statistic.distance.unwrap_or(dec!(0))
                + record.statistic.reps.unwrap_or(dec!(0))
                + record.statistic.weight.unwrap_or(dec!(0))
        })
        .map(|(index, _)| index)
}

fn get_index_of_highest_pb(
    records: &[WorkoutSetRecord],
    pb_type: &WorkoutSetPersonalBest,
) -> Option<usize> {
    let record = records.iter().reduce(|record1, record2| {
        let pb1 = record1.get_personal_best(pb_type);
        let pb2 = record2.get_personal_best(pb_type);
        match (pb1, pb2) {
            (Some(pb1), Some(pb2)) => {
                if pb1 > pb2 {
                    record1
                } else {
                    record2
                }
            }
            _ => record1,
        }
    });
    record.and_then(|r| records.iter().position(|l| l == r))
}

/// Create or update a workout in the database and also update user and exercise associations.
pub async fn create_or_update_workout(
    input: UserWorkoutInput,
    user_id: &String,
    db: &DatabaseConnection,
    perform_application_job: &MemoryStorage<ApplicationJob>,
) -> Result<String> {
    let end_time = input.end_time;
    let mut input = input;
    let (new_workout_id, to_update_workout) = match &input.update_workout_id {
        Some(id) => (
            id.to_owned(),
            // DEV: Unwrap to make sure we error out early if the workout to edit does not exist
            Some(Workout::find_by_id(id).one(db).await?.unwrap()),
        ),
        None => (
            input
                .create_workout_id
                .unwrap_or_else(|| format!("wor_{}", nanoid!(12))),
            None,
        ),
    };
    ryot_log!(debug, "Creating new workout with id = {}", new_workout_id);
    let mut exercises = vec![];
    let mut workout_totals = vec![];
    if input.exercises.is_empty() {
        bail!("This workout has no associated exercises")
    }
    let mut first_set_of_exercise_confirmed_at = input
        .exercises
        .first()
        .unwrap()
        .sets
        .first()
        .unwrap()
        .confirmed_at;
    for (exercise_idx, ex) in input.exercises.iter_mut().enumerate() {
        if ex.sets.is_empty() {
            bail!("This exercise has no associated sets")
        }
        let db_ex = match Exercise::find_by_id(ex.exercise_id.clone()).one(db).await? {
            None => {
                ryot_log!(error, "Exercise with id = {} not found", ex.exercise_id);
                continue;
            }
            Some(e) => e,
        };
        let mut sets = vec![];
        let mut total = WorkoutOrExerciseTotals::default();
        let association = UserToEntity::find()
            .filter(user_to_entity::Column::UserId.eq(user_id))
            .filter(user_to_entity::Column::ExerciseId.eq(ex.exercise_id.clone()))
            .one(db)
            .await
            .ok()
            .flatten();
        let history_item = UserToExerciseHistoryExtraInformation {
            best_set: None,
            idx: exercise_idx,
            workout_id: new_workout_id.clone(),
            workout_end_on: end_time,
        };
        let association = match association {
            None => {
                let user_to_ex = user_to_entity::ActiveModel {
                    user_id: ActiveValue::Set(user_id.clone()),
                    exercise_id: ActiveValue::Set(Some(ex.exercise_id.clone())),
                    exercise_extra_information: ActiveValue::Set(Some(
                        UserToExerciseExtraInformation {
                            history: vec![history_item],
                            lifetime_stats: WorkoutOrExerciseTotals::default(),
                            personal_bests: vec![],
                        },
                    )),
                    created_on: ActiveValue::Set(
                        first_set_of_exercise_confirmed_at.unwrap_or(end_time),
                    ),
                    exercise_num_times_interacted: ActiveValue::Set(Some(1)),
                    ..Default::default()
                };
                user_to_ex.insert(db).await.unwrap()
            }
            Some(e) => {
                let last_updated_on = e.last_updated_on;
                let mut extra_info = e.exercise_extra_information.clone().unwrap_or_default();
                extra_info.history.insert(0, history_item);
                let mut to_update: user_to_entity::ActiveModel = e.into();
                to_update.exercise_num_times_interacted =
                    ActiveValue::Set(Some(extra_info.history.len().try_into().unwrap()));
                to_update.exercise_extra_information = ActiveValue::Set(Some(extra_info));
                to_update.last_updated_on =
                    ActiveValue::Set(first_set_of_exercise_confirmed_at.unwrap_or(last_updated_on));
                to_update.update(db).await?
            }
        };
        if let Some(d) = ex.rest_time {
            total.rest_time += d * (ex.sets.len() - 1) as u16;
        }
        ex.sets
            .sort_unstable_by_key(|s| s.confirmed_at.unwrap_or_default());
        for set in ex.sets.iter_mut() {
            let mut actual_rest_time = None;
            if exercise_idx != 0
                && set.confirmed_at.is_some()
                && first_set_of_exercise_confirmed_at.is_some()
            {
                actual_rest_time = Some(
                    (set.confirmed_at.unwrap() - first_set_of_exercise_confirmed_at.unwrap())
                        .num_seconds(),
                );
            }
            first_set_of_exercise_confirmed_at = set.confirmed_at;
            set.remove_invalids(&db_ex.lot);
            if let Some(r) = set.statistic.reps {
                total.reps += r;
                if let Some(w) = set.statistic.weight {
                    total.weight += w * r;
                }
            }
            if let Some(d) = set.statistic.duration {
                total.duration += d;
            }
            if let Some(d) = set.statistic.distance {
                total.distance += d;
            }
            let mut totals = WorkoutSetTotals::default();
            if let (Some(we), Some(re)) = (&set.statistic.weight, &set.statistic.reps) {
                totals.weight = Some(we * re);
            }
            let mut value = WorkoutSetRecord {
                lot: set.lot,
                actual_rest_time,
                totals: Some(totals),
                note: set.note.clone(),
                personal_bests: Some(vec![]),
                confirmed_at: set.confirmed_at,
                statistic: set.statistic.clone(),
            };
            value.statistic.one_rm = value.calculate_one_rm();
            value.statistic.pace = value.calculate_pace();
            value.statistic.volume = value.calculate_volume();
            sets.push(value);
        }
        let mut personal_bests = association
            .exercise_extra_information
            .clone()
            .unwrap_or_default()
            .personal_bests;
        let types_of_prs = LOT_MAPPINGS
            .iter()
            .find(|lm| lm.0 == db_ex.lot)
            .map(|lm| lm.1)
            .unwrap();
        for best_type in types_of_prs.iter() {
            let set_idx = get_index_of_highest_pb(&sets, best_type).unwrap();
            let possible_record = personal_bests
                .iter()
                .find(|pb| pb.lot == *best_type)
                .and_then(|record| record.sets.first());
            let set = sets.get_mut(set_idx).unwrap();
            if let Some(r) = possible_record {
                if let Some(workout) = Workout::find_by_id(r.workout_id.clone()).one(db).await? {
                    let workout_set =
                        workout.information.exercises[r.exercise_idx].sets[r.set_idx].clone();
                    if set.get_personal_best(best_type) > workout_set.get_personal_best(best_type) {
                        if let Some(ref mut set_personal_bests) = set.personal_bests {
                            set_personal_bests.push(*best_type);
                        }
                        total.personal_bests_achieved += 1;
                    }
                }
            } else {
                if let Some(ref mut set_personal_bests) = set.personal_bests {
                    set_personal_bests.push(*best_type);
                }
                total.personal_bests_achieved += 1;
            }
        }
        workout_totals.push(total.clone());
        for (set_idx, set) in sets.iter().enumerate() {
            if let Some(set_personal_bests) = &set.personal_bests {
                for best in set_personal_bests.iter() {
                    let to_insert_record = ExerciseBestSetRecord {
                        workout_id: new_workout_id.clone(),
                        exercise_idx,
                        set_idx,
                    };
                    if let Some(record) = personal_bests.iter_mut().find(|pb| pb.lot == *best) {
                        let mut data = record.sets.clone();
                        data.insert(0, to_insert_record);
                        record.sets = data;
                    } else {
                        personal_bests.push(UserToExerciseBestSetExtraInformation {
                            lot: *best,
                            sets: vec![to_insert_record],
                        });
                    }
                }
            }
        }
        let best_set = get_best_set_index(&sets).and_then(|i| sets.get(i).cloned());
        let mut association_extra_information = association
            .exercise_extra_information
            .clone()
            .unwrap_or_default();
        association_extra_information.history[0].best_set = best_set.clone();
        let mut association: user_to_entity::ActiveModel = association.into();
        association_extra_information.lifetime_stats += total.clone();
        association_extra_information.personal_bests = personal_bests;
        association.exercise_extra_information =
            ActiveValue::Set(Some(association_extra_information));
        association.update(db).await?;
        exercises.push((
            best_set,
            db_ex.lot,
            ProcessedExercise {
                sets,
                lot: db_ex.lot,
                name: db_ex.id,
                total: Some(total),
                notes: ex.notes.clone(),
                rest_time: ex.rest_time,
                assets: ex.assets.clone(),
                superset_with: ex.superset_with.clone(),
            },
        ));
    }
    let summary_total = workout_totals.into_iter().sum();
    let model = workout::Model {
        end_time,
        name: input.name,
        user_id: user_id.clone(),
        id: new_workout_id.clone(),
        start_time: input.start_time,
        repeated_from: input.repeated_from,
        summary: WorkoutSummary {
            total: Some(summary_total),
            exercises: exercises
                .clone()
                .into_iter()
                .map(|(best_set, lot, e)| WorkoutSummaryExercise {
                    best_set,
                    lot: Some(lot),
                    id: e.name.clone(),
                    num_sets: e.sets.len(),
                })
                .collect(),
        },
        information: WorkoutInformation {
            comment: input.comment,
            assets: input.assets,
            exercises: exercises.into_iter().map(|(_, _, ex)| ex).collect(),
        },
        template_id: input.template_id,
        duration: 0,
    };
    let mut insert: workout::ActiveModel = model.into();
    insert.duration = ActiveValue::NotSet;
    if let Some(old_workout) = to_update_workout.clone() {
        insert.end_time = ActiveValue::Set(old_workout.end_time);
        insert.start_time = ActiveValue::Set(old_workout.start_time);
        insert.repeated_from = ActiveValue::Set(old_workout.repeated_from.clone());
        old_workout.delete(db).await?;
    }
    let data = insert.insert(db).await?;
    if to_update_workout.is_some() {
        deploy_job_to_re_evaluate_user_workouts(perform_application_job, user_id).await;
    }
    Ok(data.id)
}

// DEV: For exercises, reduce count, remove from history if present. We will not
// re-calculate exercise associations totals or change personal bests.
pub async fn delete_existing_workout(
    input: workout::Model,
    db: &DatabaseConnection,
    user_id: String,
) -> Result<()> {
    for (idx, ex) in input.information.exercises.iter().enumerate() {
        let association = match UserToEntity::find()
            .filter(user_to_entity::Column::UserId.eq(&user_id))
            .filter(user_to_entity::Column::ExerciseId.eq(ex.name.clone()))
            .one(db)
            .await?
        {
            None => continue,
            Some(assoc) => assoc,
        };
        let mut ei = association
            .exercise_extra_information
            .clone()
            .unwrap_or_default();
        if let Some(ex_idx) = ei
            .history
            .iter()
            .position(|e| e.workout_id == input.id && e.idx == idx)
        {
            ei.history.remove(ex_idx);
        }
        let mut association: user_to_entity::ActiveModel = association.into();
        association.exercise_extra_information = ActiveValue::Set(Some(ei));
        association.update(db).await?;
    }
    input.delete(db).await?;
    Ok(())
}
