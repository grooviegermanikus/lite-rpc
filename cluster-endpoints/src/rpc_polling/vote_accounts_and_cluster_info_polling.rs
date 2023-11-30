use anyhow::Context;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_lite_rpc_core::AnyhowJoinHandle;
use solana_rpc_client_api::response::{RpcContactInfo, RpcVoteAccountStatus};
use std::{sync::Arc, time::Duration};
use log::{debug, info};
use tokio::sync::broadcast::Sender;

pub fn poll_vote_accounts_and_cluster_info(
    rpc_client: Arc<RpcClient>,
    contact_info_sender: Sender<Vec<RpcContactInfo>>,
    vote_account_sender: Sender<RpcVoteAccountStatus>,
) -> AnyhowJoinHandle {
    debug!("Starting polling vote accounts and cluster info from RPC");
    tokio::spawn(async move {
        loop {
            if let Ok(cluster_nodes) = rpc_client.get_cluster_nodes().await {
                debug!("Get {} cluster nodes via RPC", cluster_nodes.len());
                contact_info_sender
                    .send(cluster_nodes)
                    .context("Should be able to send cluster info")?;
            }
            if let Ok(vote_accounts) = rpc_client.get_vote_accounts().await {
                debug!("Get {} vote accounts via RPC", vote_accounts.current.len());
                vote_account_sender
                    .send(vote_accounts)
                    .context("Should be able to send vote accounts")?;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    })
}
