use std::{str::FromStr, time::{SystemTime, UNIX_EPOCH}};

use bytemuck::{Pod, Zeroable};
use drillx::Solution;
use eore_api::{
    consts::{BUS_ADDRESSES, CONFIG_ADDRESS, MINT_ADDRESS, PROOF, TOKEN_DECIMALS},
    state::{proof_pda, Proof},
    ID,
};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{account::ReadableAccount, instruction::Instruction, pubkey::Pubkey};
use steel::{AccountDeserialize, event};

pub const BITZ_TOKEN_DECIMALS: u8 = TOKEN_DECIMALS;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct MineEventWithBoosts {
    pub balance: u64,
    pub difficulty: u64,
    pub last_hash_at: i64,
    pub timing: i64,
    pub reward: u64,
    pub boost_1: u64,
    pub boost_2: u64,
    pub boost_3: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct MineEventWithGlobalBoosts {
    pub balance: u64,
    pub difficulty: u64,
    pub last_hash_at: i64,
    pub timing: i64,
    pub net_reward: u64,
    pub net_base_reward: u64,
    pub net_miner_boost_reward: u64,
    pub net_staker_boost_reward: u64,
}

event!(MineEventWithGlobalBoosts);

pub async fn get_proof(client: &RpcClient, address: Pubkey) -> Result<Proof, anyhow::Error> {
    let proof_address = proof_pda(address).0;
    let data = client.get_account_data(&proof_address).await?;
    Ok(*Proof::try_from_bytes(&data)?)
}

pub fn proof_pubkey(authority: Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[PROOF, authority.as_ref()], &ID).0
}

pub fn get_bitz_mint() -> Pubkey {
    MINT_ADDRESS
}

pub fn get_claim_ix(signer: Pubkey, beneficiary: Pubkey, claim_amount: u64) -> Instruction {
    eore_api::sdk::claim(signer, beneficiary, claim_amount)
}

pub fn get_cutoff(proof: Proof, buffer_time: u64) -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Failed to get time")
        .as_secs() as i64;
    proof
        .last_hash_at
        .saturating_add(60)
        .saturating_sub(buffer_time as i64)
        .saturating_sub(now)
}

pub async fn get_proof_and_config_with_busses(
    client: &RpcClient,
    authority: Pubkey,
) -> (
    Result<Proof, ()>,
    Result<eore_api::state::Config, ()>,
    Result<Vec<Result<eore_api::state::Bus, ()>>, ()>,
) {
    let account_pubkeys = vec![
        proof_pubkey(authority),
        CONFIG_ADDRESS,
        BUS_ADDRESSES[0],
        BUS_ADDRESSES[1],
        BUS_ADDRESSES[2],
        BUS_ADDRESSES[3],
        BUS_ADDRESSES[4],
        BUS_ADDRESSES[5],
        BUS_ADDRESSES[6],
        BUS_ADDRESSES[7],
    ];

    let datas = client.get_multiple_accounts(&account_pubkeys).await;
    if let Ok(datas) = datas {
        let proof = if let Some(data) = &datas[0] {
            Ok(*bytemuck::try_from_bytes::<Proof>(&data.data()[8..])
                .expect("Failed to parse treasury account"))
        } else {
            Err(())
        };

        let treasury_config = if let Some(data) = &datas[1] {
            Ok(
                *bytemuck::try_from_bytes::<eore_api::state::Config>(&data.data()[8..])
                    .expect("Failed to parse config account"),
            )
        } else {
            Err(())
        };
        let bus_1 = if let Some(data) = &datas[2] {
            Ok(
                *bytemuck::try_from_bytes::<eore_api::state::Bus>(&data.data()[8..])
                    .expect("Failed to parse bus1 account"),
            )
        } else {
            Err(())
        };
        let bus_2 = if let Some(data) = &datas[3] {
            Ok(
                *bytemuck::try_from_bytes::<eore_api::state::Bus>(&data.data()[8..])
                    .expect("Failed to parse bus2 account"),
            )
        } else {
            Err(())
        };
        let bus_3 = if let Some(data) = &datas[4] {
            Ok(
                *bytemuck::try_from_bytes::<eore_api::state::Bus>(&data.data()[8..])
                    .expect("Failed to parse bus3 account"),
            )
        } else {
            Err(())
        };
        let bus_4 = if let Some(data) = &datas[5] {
            Ok(
                *bytemuck::try_from_bytes::<eore_api::state::Bus>(&data.data()[8..])
                    .expect("Failed to parse bus4 account"),
            )
        } else {
            Err(())
        };
        let bus_5 = if let Some(data) = &datas[6] {
            Ok(
                *bytemuck::try_from_bytes::<eore_api::state::Bus>(&data.data()[8..])
                    .expect("Failed to parse bus5 account"),
            )
        } else {
            Err(())
        };
        let bus_6 = if let Some(data) = &datas[7] {
            Ok(
                *bytemuck::try_from_bytes::<eore_api::state::Bus>(&data.data()[8..])
                    .expect("Failed to parse bus6 account"),
            )
        } else {
            Err(())
        };
        let bus_7 = if let Some(data) = &datas[8] {
            Ok(
                *bytemuck::try_from_bytes::<eore_api::state::Bus>(&data.data()[8..])
                    .expect("Failed to parse bus7 account"),
            )
        } else {
            Err(())
        };
        let bus_8 = if let Some(data) = &datas[9] {
            Ok(
                *bytemuck::try_from_bytes::<eore_api::state::Bus>(&data.data()[8..])
                    .expect("Failed to parse bus8 account"),
            )
        } else {
            Err(())
        };

        (
            proof,
            treasury_config,
            Ok(vec![bus_1, bus_2, bus_3, bus_4, bus_5, bus_6, bus_7, bus_8]),
        )
    } else {
        (Err(()), Err(()), Err(()))
    }
}


pub fn get_auth_ix(signer: Pubkey) -> Instruction {
    let proof = proof_pubkey(signer);

    eore_api::prelude::auth(proof)
}

pub fn get_reset_ix(signer: Pubkey) -> Instruction {
    eore_api::prelude::reset(signer)
}

pub fn get_mine_with_global_boost_ix(signer: Pubkey, solution: Solution, bus: usize) -> Instruction {
    let collect_ix = eore_api::sdk::mine(
        signer,
        signer,
        BUS_ADDRESSES[bus],
        solution,
        config_pda().0,
    );
    collect_ix
}

pub const CONFIG: &[u8] = b"config";
pub fn boost_program_id() -> Pubkey {
    Pubkey::from_str("eBoFjsXMceooxywb8MeCqRCkQ2JEEsd5AUELXbaQfh8").expect("Invalid pubkey")
}
pub fn config_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[CONFIG], &boost_program_id())
}