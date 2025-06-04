use chrono::NaiveDateTime;
use deadpool_diesel::mysql::{Manager, Pool};
use diesel::{sql_types::{Datetime, Text}, MysqlConnection, RunQueryDsl};

use crate::{app_database::AppDatabaseError, models};
use tracing::error;


pub struct AppRRDatabase {
    connection_pool: Pool,
}

impl AppRRDatabase {
    pub fn new(url: String) -> Self {
        let manager = Manager::new(url, deadpool_diesel::Runtime::Tokio1);

        let pool = Pool::builder(manager).build().unwrap();

        AppRRDatabase {
            connection_pool: pool,
        }
    }

    pub async fn get_miner_rewards(
        &self,
        miner_pubkey: String,
    ) -> Result<models::Reward, AppDatabaseError> {
        if let Ok(db_conn) = self.connection_pool.get().await {
            let res = db_conn.interact(move |conn: &mut MysqlConnection| {
                diesel::sql_query("SELECT * FROM miners m JOIN rewards r ON m.id = r.miner_id WHERE m.pubkey = ?")
                    .bind::<Text, _>(miner_pubkey)
                    .get_result::<models::Reward>(conn)
            }).await;

            match res {
                Ok(interaction) => match interaction {
                    Ok(query) => {
                        return Ok(query);
                    }
                    Err(e) => {
                        error!("get_miner_rewards app_rr_database: {:?}", e);
                        return Err(AppDatabaseError::QueryFailed);
                    }
                },
                Err(e) => {
                    error!("{:?}", e);
                    return Err(AppDatabaseError::InteractionFailed);
                }
            }
        } else {
            return Err(AppDatabaseError::FailedToGetConnectionFromPool);
        };
    }

    pub async fn get_earning_in_period_by_pubkey(
        &self,
        pubkey: String,
        start_date: NaiveDateTime,
        end_date: NaiveDateTime,
    ) -> Result<models::Earning, AppDatabaseError> {
        if let Ok(db_conn) = self.connection_pool.get().await {
            let res = db_conn
                .interact(move |conn: &mut MysqlConnection| {
                    diesel::sql_query(
                        "
                            SELECT
                                0 as id,
                                m.id as miner_id,
                                e.pool_id,
                                0 as challenge_id,
                                SUM(e.amount) as amount,
                                0 as difficulty,
                                MAX(e.created_at) as created_at,
                                MAX(e.updated_at) as updated_at
                            FROM earnings e
                            JOIN miners m ON e.miner_id = m.id
                            WHERE m.pubkey = ?
                                AND e.challenge_id != -1
                                AND e.created_at >= ? AND e.created_at <= ?
                            GROUP BY m.id, m.pubkey, e.pool_id
                        ß",
                    )
                    .bind::<Text, _>(pubkey)
                    .bind::<Datetime, _>(start_date)
                    .bind::<Datetime, _>(end_date)
                    .get_result::<models::Earning>(conn)
                })
                .await;

            match res {
                Ok(interaction) => match interaction {
                    Ok(query) => {
                        return Ok(query);
                    }
                    Err(e) => {
                        error!(target: "server_log", "{:?}", e);
                        return Err(AppDatabaseError::QueryFailed);
                    }
                },
                Err(e) => {
                    error!(target: "server_log", "{:?}", e);
                    return Err(AppDatabaseError::InteractionFailed);
                }
            }
        } else {
            return Err(AppDatabaseError::FailedToGetConnectionFromPool);
        };
    }

    pub async fn get_last_claim_by_pubkey(
        &self,
        pubkey: String,
    ) -> Result<models::LastClaim, AppDatabaseError> {
        if let Ok(db_conn) = self.connection_pool.get().await {
            let res = db_conn
                .interact(move |conn: &mut MysqlConnection| {
                    diesel::sql_query("SELECT c.created_at FROM claims c JOIN miners m ON c.miner_id = m.id WHERE m.pubkey = ? ORDER BY c.id DESC LIMIT 1")
                        .bind::<Text, _>(pubkey)
                        .get_result::<models::LastClaim>(conn)
                })
                .await;

            match res {
                Ok(interaction) => match interaction {
                    Ok(query) => {
                        return Ok(query);
                    }
                    Err(e) => {
                        error!(target: "server_log", "{:?}", e);
                        return Err(AppDatabaseError::QueryFailed);
                    }
                },
                Err(e) => {
                    error!(target: "server_log", "{:?}", e);
                    return Err(AppDatabaseError::InteractionFailed);
                }
            }
        } else {
            return Err(AppDatabaseError::FailedToGetConnectionFromPool);
        };
    }

}