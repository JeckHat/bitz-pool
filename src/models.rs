use chrono::NaiveDateTime;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Integer, Nullable, Text, Timestamp, TinyInt, Unsigned};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::challenges)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct Challenge {
    pub id: i32,
    pub pool_id: i32,
    pub submission_id: Option<i32>,
    pub challenge: Vec<u8>,
    pub rewards_earned: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, QueryableByName)]
pub struct ChallengeWithDifficulty {
    #[diesel(sql_type = Integer)]
    pub id: i32,
    #[diesel(sql_type = Nullable<Unsigned<BigInt>>)]
    pub rewards_earned: Option<u64>,
    #[diesel(sql_type = TinyInt)]
    pub difficulty: i8,
    #[diesel(sql_type = Unsigned<BigInt>)]
    pub challenge_hashpower: u64,
    #[diesel(sql_type = Timestamp)]
    pub updated_at: NaiveDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::challenges)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct InsertChallenge {
    pub pool_id: i32,
    pub challenge: Vec<u8>,
    pub rewards_earned: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::challenges)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct UpdateChallengeRewards {
    pub rewards_earned: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::claims)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct Claim {
    pub miner_id: i32,
    pub pool_id: i32,
    pub txn_id: i32,
    pub amount: u64,
}

#[derive(Debug, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::claims)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct LastClaim {
    pub created_at: NaiveDateTime,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::claims)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct InsertClaim {
    pub miner_id: i32,
    pub pool_id: i32,
    pub txn_id: i32,
    pub amount: u64,
}

#[derive(Debug, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::miners)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct Miner {
    pub id: i32,
    pub pubkey: String,
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::pools)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct Pool {
    pub id: i32,
    pub proof_pubkey: String,
    pub authority_pubkey: String,
    pub total_rewards: u64,
    pub claimed_rewards: u64,
}

#[derive(Debug, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::submissions)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct Submission {
    pub id: i64,
    pub miner_id: i32,
    pub challenge_id: i32,
    pub nonce: u64,
    pub difficulty: i8,
    pub created_at: NaiveDateTime,
}

#[derive(Debug, Clone, Deserialize, Serialize, QueryableByName)]
pub struct SubmissionWithPubkey {
    #[diesel(sql_type = BigInt)]
    pub id: i64,
    #[diesel(sql_type = Integer)]
    pub miner_id: i32,
    #[diesel(sql_type = Integer)]
    pub challenge_id: i32,
    #[diesel(sql_type = Unsigned<BigInt>)]
    pub nonce: u64,
    #[diesel(sql_type = TinyInt)]
    pub difficulty: i8,
    #[diesel(sql_type = Timestamp)]
    pub created_at: NaiveDateTime,
    #[diesel(sql_type = Text)]
    pub pubkey: String,
}

#[derive(
    Debug, Clone, Serialize, Deserialize, Insertable, Queryable, Selectable, QueryableByName,
)]
#[diesel(table_name = crate::schema::submissions)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct InsertSubmission {
    pub miner_id: i32,
    pub challenge_id: i32,
    pub nonce: u64,
    pub difficulty: i8,
}

#[derive(Debug, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::submissions)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct SubmissionWithId {
    pub id: i64,
}

#[derive(Debug, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::txns)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct Txn {
    pub id: i32,
    pub txn_type: String,
    pub signature: String,
    pub priority_fee: u32,
    pub created_at: NaiveDateTime,
}

#[derive(Debug, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::txns)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct TxnId {
    pub id: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::txns)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct InsertTxn {
    pub txn_type: String,
    pub signature: String,
    pub priority_fee: u32,
}

#[derive(Debug, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::rewards)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct InsertReward {
    pub miner_id: i32,
    pub pool_id: i32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::rewards)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct UpdateReward {
    pub miner_id: i32,
    pub balance: u64,
}

#[derive(Debug, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::rewards)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct Reward {
    pub miner_id: i32,
    pub balance: u64,
}

#[derive(Debug, Clone, Deserialize, Insertable, QueryableByName)]
#[diesel(table_name = crate::schema::earnings)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct InsertEarning {
    pub miner_id: i32,
    pub pool_id: i32,
    pub challenge_id: i32,
    pub amount: u64,
    pub difficulty: i8,
    pub worker_name: String,
}

#[derive(Debug, Serialize, Deserialize, Queryable, Selectable, QueryableByName)]
#[diesel(table_name = crate::schema::earnings)]
#[diesel(check_for_backend(diesel::mysql::Mysql))]
pub struct Earning {
    pub id: i64,
    pub miner_id: i32,
    pub pool_id: i32,
    pub challenge_id: i32,
    pub amount: u64,
    pub difficulty: i8,
    pub worker_name: String,
    #[diesel(sql_type = Timestamp)]
    pub created_at: NaiveDateTime,
    #[diesel(sql_type = Timestamp)]
    pub updated_at: NaiveDateTime,
}

#[derive(Debug, Deserialize, Serialize, QueryableByName)]
pub struct EarningWithChallengeWithSubmission {
    #[diesel(sql_type = Integer)]
    pub miner_id: i32,
    #[diesel(sql_type = Integer)]
    pub challenge_id: i32,
    #[diesel(sql_type = Text)]
    pub pubkey: String,
    #[diesel(sql_type = Unsigned<BigInt>)]
    pub miner_amount: u64,
    #[diesel(sql_type = TinyInt)]
    pub best_difficulty: i8,
    #[diesel(sql_type = TinyInt)]
    pub miner_difficulty: i8,
    #[diesel(sql_type = Unsigned<BigInt>)]
    pub best_challenge_hashpower: u64,
    #[diesel(sql_type = Unsigned<BigInt>)]
    pub miner_hashpower: u64,
    #[diesel(sql_type = Timestamp)]
    pub created_at: NaiveDateTime,
    #[diesel(sql_type = Nullable<Unsigned<BigInt>>)]
    pub total_rewards_earned: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, QueryableByName)]
pub struct HighDifficulty {
    #[diesel(sql_type = TinyInt)]
    pub high_difficulty: i8,
}

#[derive(Debug, Serialize, Deserialize, QueryableByName)]
pub struct Hashpower {
    #[diesel(sql_type = Unsigned<BigInt>)]
    pub hashpower: u64,
}