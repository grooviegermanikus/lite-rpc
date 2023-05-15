use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;
use futures::FutureExt;
use itertools::Itertools;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client_api::response::RpcVoteAccountInfo;
use solana_sdk::pubkey::Pubkey;

#[derive(Debug)]
pub struct StakingInfo {
    stake_per_vote_account: HashMap<Pubkey, u64>,
    // redundant
    total_stake: u64,
}


// TODO remove
pub async fn load_votestuff() -> anyhow::Result<StakingInfo> {
    // let url = "https://api.devnet.solana.com".to_string();
    let url = "http://5.62.126.197:8899/".to_string(); // mangobox testnet
    // let url = "http://147.28.169.13:8899/".to_string(); //?? small multinode machine

    let rpc_client = RpcClient::new_with_timeout(url.clone(), Duration::from_secs(90));


    // current epoc
    let res_vote_accounts = rpc_client.get_vote_accounts().await?;

    let stake_by_voter: HashMap<Pubkey, u64> =
        res_vote_accounts.current.iter()
        .filter(|&acc| acc.epoch_vote_account)
        .map(|acc| (Pubkey::from_str(acc.node_pubkey.as_str()).unwrap(), acc.activated_stake))
        .into_grouping_map()
        .sum();

    let total_stake = stake_by_voter.values().sum();
    Ok(StakingInfo {
        stake_per_vote_account: stake_by_voter,
        total_stake: total_stake
    })
}
