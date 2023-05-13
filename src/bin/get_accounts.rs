use std::time::Duration;
use anyhow::Context;
use serde_json::json;
use solana_account_decoder::UiAccountEncoding;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client_api::config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_rpc_client_api::request::RpcRequest;
use solana_rpc_client_api::response::{OptionalContext, RpcKeyedAccount};
use solana_sdk::commitment_config::CommitmentConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {

    // let url = "https://api.devnet.solana.com".to_string();
    let url = "http://5.62.126.197:8899/".to_string(); // mangobox testnet
    // let url = "http://147.28.169.13:8899/".to_string(); //?? small multinode machine

    let rpc_client = RpcClient::new_with_timeout(url, Duration::from_secs(90));


    let pubkey = solana_vote_program::id();
    let config =RpcProgramAccountsConfig {
        filters: None,
        account_config: RpcAccountInfoConfig {
            encoding: Some(UiAccountEncoding::Base64),
            data_slice: None,
            commitment: Some(CommitmentConfig::confirmed()),
            min_context_slot: None,
        },
        with_context: Some(true),
    };
    let response = rpc_client
        .send::<OptionalContext<Vec<RpcKeyedAccount>>>(
            RpcRequest::GetProgramAccounts,
            json!([pubkey.to_string(), config]),
        )
        .await?;
    if let OptionalContext::Context(response) = response {
        println!("context: {:?}", response.context.slot);
    }

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
