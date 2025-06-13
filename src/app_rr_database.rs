use chrono::NaiveDateTime;
use deadpool_diesel::mysql::{Manager, Pool};
use diesel::{sql_types::{BigInt, Datetime, Integer, Text}, MysqlConnection, RunQueryDsl};

use crate::{app_database::AppDatabaseError, models, MIN_DIFF, MIN_HASHPOWER};
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

    pub async fn get_high_difficulty(
        &self,
        pubkey: String,
    ) -> Result<models::HighDifficulty, AppDatabaseError> {
        if let Ok(db_conn) = self.connection_pool.get().await {
            let mut sql_query = "
                    SELECT MAX(e.difficulty) as high_difficulty
                    FROM earnings e 
                    WHERE DATE(e.created_at) = DATE(NOW())
                    ORDER BY e.difficulty DESC
                    LIMIT 1;
                ";
            if !pubkey.is_empty() {
                sql_query = "
                    SELECT MAX(e.difficulty) as high_difficulty
                    FROM earnings e 
                    JOIN miners m
                    ON m.id = e.miner_id 
                    WHERE DATE(e.created_at) = DATE(NOW())
                    AND m.pubkey = ?
                    ORDER BY e.difficulty DESC
                    LIMIT 1;
                ";
            }
            let res = db_conn
                .interact(move |conn: &mut MysqlConnection| {
                    diesel::sql_query(sql_query)
                    .bind::<Text, _>(pubkey)
                    .get_result::<models::HighDifficulty>(conn)
                })
                .await;
    
            match res {
                Ok(Ok(query)) => Ok(query),
                Ok(Err(e)) => {
                    error!(target: "server_log", "DB error: {:?}", e);
                    Err(AppDatabaseError::QueryFailed)
                }
                Err(e) => {
                    error!(target: "server_log", "Interaction error: {:?}", e);
                    Err(AppDatabaseError::InteractionFailed)
                }
            }
        } else {
            Err(AppDatabaseError::FailedToGetConnectionFromPool)
        }
    }

    pub async fn get_hashpower(
        &self,
        pubkey: String,
    ) -> Result<models::Hashpower, AppDatabaseError> {
        if let Ok(db_conn) = self.connection_pool.get().await {
            let mut sql_query = "
                    SELECT sum(80 * POW(2, e.difficulty - 12)) as hashpower
                    FROM earnings e
                    WHERE e.challenge_id  = (
                        SELECT c.id
                        FROM challenges c
                        ORDER BY c.id DESC
                        LIMIT 1 OFFSET 2
                    )
                ";
            if !pubkey.is_empty() {
                sql_query = "
                    SELECT sum(80 * POW(2, e.difficulty - 12)) as hashpower
                    FROM earnings e
                    JOIN miners m
                    ON m.id = e.miner_id
                    WHERE e.challenge_id  = (
                        SELECT c.id
                        FROM challenges c
                        ORDER BY c.id DESC
                        LIMIT 1 OFFSET 2
                    )
                    AND m.pubkey = ?
                ";
            }
            let res = db_conn
                .interact(move |conn: &mut MysqlConnection| {
                    diesel::sql_query(sql_query)
                    .bind::<Text, _>(pubkey)
                    .get_result::<models::Hashpower>(conn)
                })
                .await;
    
            match res {
                Ok(Ok(query)) => Ok(query),
                Ok(Err(e)) => {
                    error!(target: "server_log", "DB error: {:?}", e);
                    Err(AppDatabaseError::QueryFailed)
                }
                Err(e) => {
                    error!(target: "server_log", "Interaction error: {:?}", e);
                    Err(AppDatabaseError::InteractionFailed)
                }
            }
        } else {
            Err(AppDatabaseError::FailedToGetConnectionFromPool)
        }
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

    pub async fn get_earnings_with_challenge_and_submission_day(
        &self,
        pubkey: String
    ) -> Result<Vec<models::EarningWithChallengeWithSubmission>, AppDatabaseError> {
        if let Ok(db_conn) = self.connection_pool.get().await {
            let res = db_conn
                .interact(move |conn: &mut MysqlConnection| {
                    diesel::sql_query(
                        "
                WITH challenge_best_difficulty AS (
                    SELECT challenge_id, MAX(difficulty) as best_difficulty
                    FROM earnings
                    WHERE created_at >= NOW() - INTERVAL 1 DAY
                        AND challenge_id != -1
                    GROUP BY challenge_id
                )
                SELECT
                    e.miner_id,
                    e.challenge_id,
                    m.pubkey,
                    e.amount_coal as miner_amount_coal,
                    e.amount_ore as miner_amount_ore,
                    cbd.best_difficulty,
                    e.difficulty as miner_difficulty,
                    ? * POW(2, e.difficulty - ?) as miner_hashpower,
                    ? * POW(2, cbd.best_difficulty - ?) as best_challenge_hashpower,
                    e.created_at,
                    c.rewards_earned_coal as total_rewards_earned_coal,
                    c.rewards_earned_ore as total_rewards_earned_ore
                FROM
                    earnings e
                JOIN
                    miners m ON e.miner_id = m.id
                JOIN
                    challenges c ON e.challenge_id = c.id
                JOIN
                    challenge_best_difficulty cbd ON e.challenge_id = cbd.challenge_id
                WHERE
                    m.pubkey = ?
                    AND e.created_at >= NOW() - INTERVAL 1 DAY
                    AND e.challenge_id != -1
                ORDER BY
                    e.created_at DESC
                ",
                    )
                    .bind::<BigInt, _>(MIN_HASHPOWER as i64)
                    .bind::<Integer, _>(MIN_DIFF as i32)
                    .bind::<BigInt, _>(MIN_HASHPOWER as i64)
                    .bind::<Integer, _>(MIN_DIFF as i32)
                    .bind::<Text, _>(pubkey)
                    .get_results::<models::EarningWithChallengeWithSubmission>(conn)
                })
                .await;

            match res {
                Ok(interaction) => match interaction {
                    Ok(query) => {
                        return Ok(query);
                    }
                    Err(e) => {
                        error!("get_earnings_with_challenge_and_submission: {:?}", e);
                        return Err(AppDatabaseError::QueryFailed);
                    }
                },
                Err(e) => {
                    error!("get_earnings_with_challenge_and_submission {:?}", e);
                    return Err(AppDatabaseError::InteractionFailed);
                }
            }
        } else {
            return Err(AppDatabaseError::FailedToGetConnectionFromPool);
        }
    }

    pub async fn get_earnings_with_challenge_and_submission_hours(
        &self,
        pubkey: String
    ) -> Result<Vec<models::EarningWithChallengeWithSubmission>, AppDatabaseError> {
        if let Ok(db_conn) = self.connection_pool.get().await {
            let res = db_conn
                .interact(move |conn: &mut MysqlConnection| {
                    diesel::sql_query(
                        "
                WITH challenge_best_difficulty AS (
                    SELECT challenge_id, MAX(difficulty) as best_difficulty
                    FROM earnings
                    WHERE created_at >= NOW() - INTERVAL 3 HOUR
                        AND challenge_id != -1
                    GROUP BY challenge_id
                )
                SELECT
                    e.miner_id,
                    e.challenge_id,
                    m.pubkey,
                    e.amount_coal as miner_amount_coal,
                    e.amount_ore as miner_amount_ore,
                    cbd.best_difficulty,
                    e.difficulty as miner_difficulty,
                    ? * POW(2, e.difficulty - ?) as miner_hashpower,
                    ? * POW(2, cbd.best_difficulty - ?) as best_challenge_hashpower,
                    e.created_at,
                    c.rewards_earned_coal as total_rewards_earned_coal,
                    c.rewards_earned_ore as total_rewards_earned_ore
                FROM
                    earnings e
                JOIN
                    miners m ON e.miner_id = m.id
                JOIN
                    challenges c ON e.challenge_id = c.id
                JOIN
                    challenge_best_difficulty cbd ON e.challenge_id = cbd.challenge_id
                WHERE
                    m.pubkey = ?
                    AND e.created_at >= NOW() - INTERVAL 3 HOUR
                    AND e.challenge_id != -1
                ORDER BY
                    e.created_at DESC
                ",
                    )
                    .bind::<BigInt, _>(MIN_HASHPOWER as i64)
                    .bind::<Integer, _>(MIN_DIFF as i32)
                    .bind::<BigInt, _>(MIN_HASHPOWER as i64)
                    .bind::<Integer, _>(MIN_DIFF as i32)
                    .bind::<Text, _>(pubkey)
                    .get_results::<models::EarningWithChallengeWithSubmission>(conn)
                })
                .await;

            match res {
                Ok(interaction) => match interaction {
                    Ok(query) => {
                        return Ok(query);
                    }
                    Err(e) => {
                        error!("get_earnings_with_challenge_and_submission: {:?}", e);
                        return Err(AppDatabaseError::QueryFailed);
                    }
                },
                Err(e) => {
                    error!("get_earnings_with_challenge_and_submission {:?}", e);
                    return Err(AppDatabaseError::InteractionFailed);
                }
            }
        } else {
            return Err(AppDatabaseError::FailedToGetConnectionFromPool);
        }
    }

}