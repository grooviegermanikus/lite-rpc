use std::time::Duration;
use anyhow::Context;
use itertools::Itertools;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::{http_client, rpc_params};
use serde_json::json;
use solana_account_decoder::UiAccountEncoding;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client_api::config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_rpc_client_api::request::RpcRequest;
use solana_rpc_client_api::response::{OptionalContext, RpcKeyedAccount};
use solana_sdk::commitment_config::CommitmentConfig;
use serde::{Deserialize, Serialize};

#[tokio::main]
async fn main() -> anyhow::Result<()> {

    // let url = "https://api.devnet.solana.com".to_string();
    let url = "http://5.62.126.197:8899/".to_string(); // mangobox testnet
    // let url = "http://147.28.169.13:8899/".to_string(); //?? small multinode machine

    let rpc_client = RpcClient::new_with_timeout(url.clone(), Duration::from_secs(90));


    let pubkey = solana_vote_program::id();
    let config = RpcProgramAccountsConfig {
        filters: None,
        account_config: RpcAccountInfoConfig {
            encoding: Some(UiAccountEncoding::Base64),
            data_slice: None,
            commitment: Some(CommitmentConfig::confirmed()),
            min_context_slot: None,
        },
        with_context: Some(true),
    };


    // let rpsee = jsonrpsee::http_client::HttpClientBuilder::default()
    //     .max_response_size(1 * 1024 * 1024 * 1024)
    //     .build(url)
    //     .unwrap();
    //
    // let ressss: OptionalContext<Vec<RpcKeyedAccount>> = rpsee.request(RpcRequest::GetProgramAccounts.to_string().as_str(),
    //                       rpc_params!(pubkey.to_string(), config)).await.unwrap();
    //
    //
    // if let OptionalContext::Context(response) = ressss {
    //     println!("context: {:?}", response.context.slot);
    // }

    let slot = rpc_client.get_slot().await?;
    let epoch_schedule = rpc_client.get_epoch_schedule().await?;
    let asdf = rpc_client.get_leader_schedule(Some(197145906)).await?;
    epoch_schedule.get_epoch_and_slot_index(197145906);

    // current epoc
    let res_vote_accounts = rpc_client.get_vote_accounts().await?;

    let total_stake = res_vote_accounts.current.iter().map(|acc| acc.activated_stake).sum::<u64>();

    let topn_count = 30;

    // confirmaation threshold 0.7
    let top_n_validators =
        res_vote_accounts.current.iter().sorted_by_key(|acc| acc.activated_stake).rev().take(topn_count).collect_vec();

    let top_n_stake = top_n_validators.iter().map(|acc| acc.activated_stake).sum::<u64>();

    for acc in top_n_validators {

        println!("vote account: {:?} {:?}", acc.node_pubkey,  acc.activated_stake);
    }

    println!("total stake: {:?} topn stake: {:?} ratio {:?}", total_stake, top_n_stake, top_n_stake as f64 / total_stake as f64);

    print!("vote accounts: {:?}", &res_vote_accounts.current.len());

    // let response = rpc_client
    //     .send::<OptionalContext<Vec<RpcKeyedAccount>>>(
    //         RpcRequest::GetProgramAccounts,
    //         json!([pubkey.to_string(), config]),
    //     )
    //     .await?;
    // if let OptionalContext::Context(response) = response {
    //     println!("context: {:?}", response.context.slot);
    // }

    // let vote_accounts = rpc_client.get_program_accounts_with_config(
    //     &solana_vote_program::id(),
    //     RpcProgramAccountsConfig {
    //         filters: None,
    //         account_config: RpcAccountInfoConfig {
    //             encoding: Some(UiAccountEncoding::Base64),
    //             data_slice: None,
    //             commitment: Some(CommitmentConfig::confirmed()),
    //             min_context_slot: None,
    //         },
    //         with_context: Some(true),
    //     },
    // ).await.with_context(|| "explain me").expect("accounts?")?;
    //
    // vote_accounts.context.slot;

    // for account in vote_accounts {
    //
    //     println!("account: {:?}", account
    //
    // }



    // for stuff in vote_accounts {
    //     // println!("account: {:?}", account.data.len());
    // }
    // println!("vote accounts: {:?}", vote_accounts.len());


    // let stake_accounts = rpc_client.get_program_accounts_with_config(
    //     &solana_sdk::stake::program::id(),
    //     RpcProgramAccountsConfig {
    //         filters: None,
    //         account_config: RpcAccountInfoConfig {
    //             encoding: Some(UiAccountEncoding::Base64),
    //             data_slice: None,
    //             commitment: Some(CommitmentConfig::confirmed()),
    //             min_context_slot: None,
    //         },
    //         with_context: None,
    //     },
    // ).await.with_context(|| "explain me").expect("accounts?")?;


    // for (pubkey, account) in accounts {
    //     println!("account: {:?}", account.data.len());
    // }
    // println!("stake accounts: {:?}", stake_accounts.len());


    Ok(())
}
