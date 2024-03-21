use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use futures::future::join_all;
use log::info;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client_api::client_error::ErrorKind;
use solana_rpc_client_api::request::RpcError;
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
        let mut account_pks = vec![];
        for account_pk in account_chunk {
            for _i in 0..3 {
                let fun = rpc_client.get_account_with_commitment(&account_pk, CommitmentConfig::processed());
                account_fns.push(fun);
                account_pks.push(account_pk);
            }
        }

        let results = join_all(account_fns).await;

        let ok_count = results.iter().filter(|x| x.is_ok()).count();
        let err_count = results.iter().filter(|x| x.is_err()).count();

        let zipped = account_pks.iter().zip(results.iter());

        info!("accounts OK: {:?} ERR: {:?}", ok_count, err_count);

        let mut errors = HashMap::<Pubkey, AtomicU32>::with_capacity(1000);
        for (account_pk, ref res) in zipped {
            match res {
                Ok(rpc_repsonse) => {
                    info!("success account: {:?}", account_pk)
                }
                Err(res_err) => {

                    if let ErrorKind::RpcError(RpcError::ForUser(for_user)) = &res_err.kind {
                        info!("ForUser: {:?}", for_user);
                        let pubkey = parse_pubkey(for_user);
                        info!("pubkey: {:?}", pubkey);
                        errors.entry(pubkey)
                            .and_modify(|counter| { counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed); } )
                            .or_insert(AtomicU32::new(0));
                    }
                    info!("ERROR account: {:?}", res_err);
                }
            }
        }
    }

}

fn parse_pubkey(input: &str) -> Pubkey {
    let split: Vec<&str> = input.split(":").collect();
    let split = split[1].trim();
    let split = split.replace("pubkey=", "");
    Pubkey::from_str(&split).unwrap()
}

#[test]
fn parser_foruser_error() {
    let input = "AccountNotFound: pubkey=95oWcswpsjjoeWcWFZzpAQ1CzsAiYur5PwYbs8SxxZCG: error sending request for url (http://127.0.0.1:8890/): error trying to connect: tcp connect error: Connection refused (os error 61)";
    let split: Vec<&str> = input.split(":").collect();
    let split = split[1].trim();
    assert_eq!(
        split,
        "pubkey=95oWcswpsjjoeWcWFZzpAQ1CzsAiYur5PwYbs8SxxZCG"
    );
    let split = split.replace("pubkey=", "");
    assert_eq!(
        split,
        "95oWcswpsjjoeWcWFZzpAQ1CzsAiYur5PwYbs8SxxZCG"
    );
    assert_eq!(split, parse_pubkey(&input).to_string());
}