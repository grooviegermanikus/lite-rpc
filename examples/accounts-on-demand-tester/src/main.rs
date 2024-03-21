use std::str::FromStr;
use std::sync::Arc;
use futures::future::join_all;
use log::info;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;

#[tokio::main]
pub async fn main() {
    tracing_subscriber::fmt::fmt().with_max_level(tracing::Level::INFO).init();

    let mut reader = csv::Reader::from_path("/Users/stefan/mango/projects/RPCv2/loadtest/accounts-testnet.csv").unwrap();

    let mut accounts = vec![];
    for line in reader.records().take(1000) {
        let record = line.unwrap();
        let pubkey = record.get(0).unwrap();
        let pubkey = Pubkey::from_str(pubkey).unwrap();
        accounts.push(pubkey);
    }

    let rpc_client = Arc::new(RpcClient::new("http://127.0.0.1:8890/".to_string()));

    for account_chunk in accounts.chunks(5) {
        let mut account_fns = vec![];
        for account in account_chunk {
            for _i in 0..3 {
                let fun = rpc_client.get_account_with_commitment(&account, CommitmentConfig::processed());
                account_fns.push(fun);
            }
        }

        let results = join_all(account_fns).await;

        let ok_count = results.iter().filter(|x| x.is_ok()).count();
        let err_count = results.iter().filter(|x| x.is_err()).count();

        info!("accounts OK: {:?} ERR: {:?}", ok_count, err_count);

        for ref res in results {
            match res {
                Ok(_) => {}
                Err(res_err) => {
                    info!("ERROR account: {:?}", res_err);
                }
            }
        }
    }

}