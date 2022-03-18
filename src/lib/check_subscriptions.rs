use actix_web::cookie::time::OffsetDateTime;
use serde_json::json;
use sqlx::{Pool, Postgres};
use uuid::Uuid;

use crate::{
    bigquery::client::{BQClient, BQError, ResultSet},
    models::{
        aic::{AICArchiveModel, AICModel},
        subscriptions::{Subscription, SubscriptionModel},
    },
};

// Throw an error if required fields are not available
fn make_subscription_from_bq_row(rs: &ResultSet) -> Result<Subscription, BQError> {
    let sub = Subscription {
        id: Uuid::new_v4(),
        flow_id: rs.require_string_by_name("flow_id")?,
        subscription_id: rs.require_string_by_name("subscription_id")?,
        report_timestamp: rs.require_offsetdatetime_by_name("report_timestamp")?,
        subscription_created: rs.require_offsetdatetime_by_name("subscription_created")?,
        fxa_uid: rs.require_string_by_name("fxa_uid")?,
        quantity: rs.require_i32_by_name("quantity")?,
        plan_id: rs.require_string_by_name("plan_id")?,
        plan_currency: rs.require_string_by_name("plan_currency")?,
        plan_amount: rs.require_i32_by_name("plan_amount")?,
        country: rs.get_string_by_name("country")?,
        aic_id: None,
        aic_expires: None,
        cj_event_value: None,
        status: None,
        status_history: None,
    };
    Ok(sub)
}

pub async fn fetch_and_process_new_subscriptions(bq: BQClient, db_pool: &Pool<Postgres>) {
    let subscriptions = SubscriptionModel { db_pool };
    let aics = AICModel { db_pool };
    let aics_archive = AICArchiveModel { db_pool };
    // Get all results from bigquery table that stores new subscription reports
    let query = "SELECT * FROM `cjms_bigquery.cj_attribution_v1`;";
    let mut rs = bq.get_bq_results(query).await;
    while rs.next_row() {
        // If can't deserialize e.g. required fields are not available log and move on.
        let mut sub = match make_subscription_from_bq_row(&rs) {
            Ok(sub) => sub,
            Err(e) => {
                // TODO - Log information and get a metric
                println!(
                    "Failed to make subscription for bq result row. {:?}. Continuing...",
                    e
                );
                continue;
            }
        };
        let get_aic_for_sub = aics.fetch_one_by_flow_id(&sub.flow_id).await;
        match get_aic_for_sub {
            Ok(aic) => {
                // - append the aic_id and cj_event_value (if found in aic or aic_archive table)
                sub.aic_id = Some(aic.id);
                sub.cj_event_value = Some(aic.cj_event_value.clone());
                sub.aic_expires = Some(aic.expires);

                // TODO - Handle this case on report
                // - mark status do_not_report (if subscription_starttime is after aic expires)
                // Add details to status_history blob
                match aics_archive.create_from_aic(&aic).await {
                    Ok(to_delete) => {
                        // TODO - Discuss the use of initiating a panic here.
                        // I think this is a time when it's a good idea because something very unexpected would be happening
                        // here and it's not clear how to recover.
                        aics.delete(&to_delete.id)
                            .await
                            .expect("Failed to delete aic after creating in aic_archive.");
                    }
                    Err(e) => {
                        println!("Failed to create aic_archive entry: {:?}. Continuing...", e);
                        continue;
                    }
                };
            }
            // - TODO - append the aic_id and cj_event_value (if found in aic_archive table)
            Err(e) => {
                // TODO - maybe lets make the subscription row anyway..
                println!(
                    "Errorr getting aic for subscription: {:?}. Continuing....",
                    e
                );
                continue;
            }
        }
        sub.status = Some("not_reported".to_string());
        sub.status_history = Some(json!([{
            "status": "not_reported",
            "t": OffsetDateTime::now_utc().to_string()
        }]));
        let _created = match subscriptions.create_from_sub(&sub).await {
            Ok(sub) => sub,
            Err(e) => match e {
                sqlx::Error::Database(e) => {
                    // The code for duplicate key constraints e.g. duplicate flow id issues
                    if e.code() == Some(std::borrow::Cow::Borrowed("23505")) {
                        // ToDo add some specific logging / metrics around duplicate key issues.
                        // This could help us see that we have an ETL issue.
                        println!("Duplicate Key Violation");
                    }
                    println!(
                        "DatabaseError error while creating subscription {:?}. Continuing...",
                        e
                    );
                    continue;
                }
                _ => {
                    println!(
                        "Unexpected error while creating subscription {:?}. Continuing...",
                        e
                    );
                    continue;
                }
            },
        };
    }
}
