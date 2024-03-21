use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use futures::StreamExt;
use itertools::Itertools;
use log::info;
use merge_streams::MergeStreams;
use prometheus::{opts, register_int_gauge, IntGauge};
use solana_lite_rpc_accounts::account_store_interface::AccountStorageInterface;
use solana_lite_rpc_cluster_endpoints::geyser_grpc_connector::GrpcSourceConfig;
use solana_lite_rpc_core::{
    commitment_utils::Commitment,
    structures::{
        account_data::{AccountData, AccountNotificationMessage, AccountStream},
        account_filter::{AccountFilterType, AccountFilters, MemcmpFilterData},
    },
    AnyhowJoinHandle,
};
use solana_sdk::{account::Account, pubkey::Pubkey};
use tokio::sync::{
    broadcast::{self, Sender},
    watch, Notify,
};
use tracing::{debug, warn};
use yellowstone_grpc_proto::geyser::{
    subscribe_request_filter_accounts_filter::Filter,
    subscribe_request_filter_accounts_filter_memcmp::Data, subscribe_update::UpdateOneof,
    SubscribeRequest, SubscribeRequestFilterAccounts, SubscribeRequestFilterAccountsFilter,
    SubscribeRequestFilterAccountsFilterMemcmp,
};
use solana_lite_rpc_cluster_endpoints::geyser_grpc_connector::grpc_subscription_autoreconnect_tasks::create_geyser_autoconnection_task_with_mpsc;

lazy_static::lazy_static! {
static ref ON_DEMAND_SUBSCRIPTION_RESTARTED: IntGauge =
        register_int_gauge!(opts!("literpc_count_account_on_demand_resubscribe", "Count number of account on demand has resubscribed")).unwrap();

        static ref ON_DEMAND_UPDATES: IntGauge =
        register_int_gauge!(opts!("literpc_count_account_on_demand_updates", "Count number of updates for account on demand")).unwrap();
}

pub struct SubscriptionManger {
    account_filter_watch: watch::Sender<AccountFilters>,
}

impl SubscriptionManger {
    pub fn new(
        grpc_sources: Vec<GrpcSourceConfig>,
        accounts_storage: Arc<dyn AccountStorageInterface>,
        account_notification_sender: Sender<AccountNotificationMessage>,
    ) -> Self {
        let (account_filter_watch, reciever) = watch::channel::<AccountFilters>(vec![]);

        let (_, mut account_stream) = create_grpc_account_streaming_tasks(grpc_sources, reciever);

        tokio::spawn(async move {
            loop {
                match account_stream.recv().await {
                    Ok(message) => {
                        ON_DEMAND_UPDATES.inc();
                        let _ = account_notification_sender.send(message.clone());
                        accounts_storage
                            .update_account(message.data, message.commitment)
                            .await;
                    }
                    Err(e) => match e {
                        broadcast::error::RecvError::Closed => {
                            panic!("Account stream channel is broken");
                        }
                        broadcast::error::RecvError::Lagged(lag) => {
                            log::error!("Account on demand stream lagged by {lag:?}, missed some account updates; continue");
                            continue;
                        }
                    },
                }
            }
        });
        Self {
            account_filter_watch,
        }
    }

    pub async fn update_subscriptions(&self, filters: AccountFilters) {
        debug!("Notify accounts subscription update with {:?}", filters);
        if let Err(e) = self.account_filter_watch.send(filters) {
            log::error!(
                "Error updating accounts on demand subscription with {}",
                e.to_string()
            );
        }
    }
}

pub fn start_account_streaming_task(
    grpc_config: GrpcSourceConfig,
    accounts_filters: AccountFilters,
    account_stream_sx: broadcast::Sender<AccountNotificationMessage>,
    has_started: Arc<Notify>,
) -> AnyhowJoinHandle {
    tokio::spawn(async move {
        'main_loop: loop {
            let processed_commitment = yellowstone_grpc_proto::geyser::CommitmentLevel::Processed;

            let mut subscribe_programs: HashMap<String, SubscribeRequestFilterAccounts> =
                HashMap::new();

            let mut accounts_to_subscribe = HashSet::new();

            for (index, accounts_filter) in accounts_filters.iter().enumerate() {
                if !accounts_filter.accounts.is_empty() {
                    accounts_filter.accounts.iter().for_each(|account| {
                        accounts_to_subscribe.insert(account.clone());
                    });
                }
                if let Some(program_id) = &accounts_filter.program_id {
                    let filters = if let Some(filters) = &accounts_filter.filters {
                        filters
                            .iter()
                            .map(|filter| match filter {
                                AccountFilterType::Datasize(size) => {
                                    SubscribeRequestFilterAccountsFilter {
                                        filter: Some(Filter::Datasize(*size)),
                                    }
                                }
                                AccountFilterType::Memcmp(memcmp) => {
                                    SubscribeRequestFilterAccountsFilter {
                                        filter: Some(Filter::Memcmp(
                                            SubscribeRequestFilterAccountsFilterMemcmp {
                                                offset: memcmp.offset,
                                                data: Some(match &memcmp.data {
                                                    MemcmpFilterData::Bytes(bytes) => {
                                                        Data::Bytes(bytes.clone())
                                                    }
                                                    MemcmpFilterData::Base58(data) => {
                                                        Data::Base58(data.clone())
                                                    }
                                                    MemcmpFilterData::Base64(data) => {
                                                        Data::Base64(data.clone())
                                                    }
                                                }),
                                            },
                                        )),
                                    }
                                }
                                AccountFilterType::TokenAccountState => {
                                    SubscribeRequestFilterAccountsFilter {
                                        filter: Some(Filter::TokenAccountState(false)),
                                    }
                                }
                            })
                            .collect_vec()
                    } else {
                        vec![]
                    };
                    subscribe_programs.insert(
                        format!("program_accounts_on_demand_{}", index),
                        SubscribeRequestFilterAccounts {
                            account: vec![],
                            owner: vec![program_id.clone()],
                            filters,
                        },
                    );
                }
            }

            let program_subscribe_request = SubscribeRequest {
                accounts: subscribe_programs,
                slots: Default::default(),
                transactions: Default::default(),
                blocks: Default::default(),
                blocks_meta: Default::default(),
                entry: Default::default(),
                commitment: Some(processed_commitment.into()),
                accounts_data_slice: Default::default(),
                ping: None,
            };

            log::info!(
                "Accounts on demand subscribing to {}",
                grpc_config.grpc_addr
            );
            let Ok(mut client) = yellowstone_grpc_client::GeyserGrpcClient::connect(
                grpc_config.grpc_addr.clone(),
                grpc_config.grpc_x_token.clone(),
                None,
            ) else {
                // problem connecting to grpc, retry after a sec
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            };

            let Ok(account_stream) = client.subscribe_once2(program_subscribe_request).await else {
                // problem subscribing to geyser stream, retry after a sec
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            };

            // each account subscription batch will require individual stream
            let mut subscriptions = vec![account_stream];
            let mut index = 0;
            for accounts_chunk in accounts_to_subscribe.iter().collect_vec().chunks(100) {
                let mut accounts_subscription: HashMap<String, SubscribeRequestFilterAccounts> =
                    HashMap::new();
                index += 1;
                accounts_subscription.insert(
                    format!("account_sub_{}", index),
                    SubscribeRequestFilterAccounts {
                        account: accounts_chunk
                            .iter()
                            .map(|acc| (*acc).clone())
                            .collect_vec(),
                        owner: vec![],
                        filters: vec![],
                    },
                );
                let mut client = yellowstone_grpc_client::GeyserGrpcClient::connect(
                    grpc_config.grpc_addr.clone(),
                    grpc_config.grpc_x_token.clone(),
                    None,
                )
                .unwrap();

                let account_request = SubscribeRequest {
                    accounts: accounts_subscription,
                    slots: Default::default(),
                    transactions: Default::default(),
                    blocks: Default::default(),
                    blocks_meta: Default::default(),
                    entry: Default::default(),
                    commitment: Some(processed_commitment.into()),
                    accounts_data_slice: Default::default(),
                    ping: None,
                };

                let account_stream = client.subscribe_once2(account_request).await.unwrap();
                subscriptions.push(account_stream);
            }
            let mut merged_stream = subscriptions.merge();

            while let Some(message) = merged_stream.next().await {
                let message = match message {
                    Ok(message) => message,
                    Err(status) => {
                        log::error!("Account on demand grpc error : {}", status.message());
                        continue;
                    }
                };
                let Some(update) = message.update_oneof else {
                    continue;
                };

                has_started.notify_one();

                match update {
                    UpdateOneof::Account(account) => {
                        if let Some(account_data) = account.account {
                            let account_pk_bytes: [u8; 32] = account_data
                                .pubkey
                                .try_into()
                                .expect("Pubkey should be 32 byte long");
                            let owner: [u8; 32] = account_data
                                .owner
                                .try_into()
                                .expect("owner pubkey should be deserializable");
                            let notification = AccountNotificationMessage {
                                data: AccountData {
                                    pubkey: Pubkey::new_from_array(account_pk_bytes),
                                    account: Arc::new(Account {
                                        lamports: account_data.lamports,
                                        data: account_data.data,
                                        owner: Pubkey::new_from_array(owner),
                                        executable: account_data.executable,
                                        rent_epoch: account_data.rent_epoch,
                                    }),
                                    updated_slot: account.slot,
                                },
                                // TODO update with processed commitment / check above
                                commitment: Commitment::Processed,
                            };
                            if account_stream_sx.send(notification).is_err() {
                                // non recoverable, i.e the whole stream is being restarted
                                log::error!("Account stream broken, breaking from main loop");
                                break 'main_loop;
                            }
                        }
                    }
                    UpdateOneof::Ping(_) => {
                        log::trace!("GRPC Ping accounts stream");
                    }
                    _ => {
                        log::error!("GRPC accounts steam misconfigured");
                    }
                };
            }
        }
        Ok(())
    })
}

pub fn create_grpc_account_streaming_tasks(
    grpc_sources: Vec<GrpcSourceConfig>,
    mut account_filter_watch: watch::Receiver<AccountFilters>,
) -> (AnyhowJoinHandle, AccountStream) {
    let (account_sender, accounts_stream) = broadcast::channel::<AccountNotificationMessage>(128);

    let jh: AnyhowJoinHandle = tokio::spawn(async move {
        debug!("Accounts on demand task starting");
        match account_filter_watch.changed().await {
            Ok(_) => {
                // do nothing
                warn!("Account filter watch changed before the task started");
            }
            Err(e) => {
                log::error!("account filter watch failed with error {}", e);
                anyhow::bail!("Accounts on demand task failed");
            }
        }
        let accounts_filters = account_filter_watch.borrow_and_update().clone();
        debug!("First account filter received: {:?}", accounts_filters);

        let has_started = Arc::new(tokio::sync::Notify::new());
        let mut current_tasks = grpc_sources
            .iter()
            .map(|grpc_config| {
                start_account_streaming_task(
                    grpc_config.clone(),
                    accounts_filters.clone(),
                    account_sender.clone(),
                    has_started.clone(),
                )
            })
            .collect_vec();

        while account_filter_watch.changed().await.is_ok() {
            ON_DEMAND_SUBSCRIPTION_RESTARTED.inc();
            // wait for a second to get all the accounts to update
            tokio::time::sleep(Duration::from_secs(1)).await;
            let accounts_filters = account_filter_watch.borrow_and_update().clone();
            let started = tokio::time::Instant::now();
            debug!("Account filter update received from watch: {:?}", accounts_filters);

            let has_started = Arc::new(tokio::sync::Notify::new());
            let new_tasks = grpc_sources
                .iter()
                .map(|grpc_config| {
                    start_account_streaming_task(
                        grpc_config.clone(),
                        accounts_filters.clone(),
                        account_sender.clone(),
                        has_started.clone(),
                    )
                })
                .collect_vec();

            // warn!("CHAOSMONKEY: slow subscribe");
            // tokio::time::sleep(Duration::from_secs(5)).await;

            if let Err(_elapsed) =
                tokio::time::timeout(Duration::from_secs(60), has_started.notified()).await
            {
                // check if time elapsed during restart is greater than 60s
                log::error!("Tried to restart the accounts on demand task but failed - keep old subscription");
                new_tasks.iter().for_each(|x| x.abort());
                continue;
            }

            // abort previous tasks
            current_tasks.iter().for_each(|x| x.abort());

            current_tasks = new_tasks;
            // takes 2 secs or so
            info!("Accounts on demand task subscribed with new filter in {:?}", started.elapsed());
        }
        log::error!("Accounts on demand task stopped");
        anyhow::bail!("Accounts on demand task stopped");
    });

    (jh, accounts_stream)
}
