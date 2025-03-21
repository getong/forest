// Copyright 2019-2025 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::SystemTime,
};

use crate::chain::{ChainStore, Error as ChainStoreError};
use crate::chain_sync::{
    bad_block_cache::BadBlockCache,
    metrics,
    network_context::SyncNetworkContext,
    sync_state::SyncState,
    tipset_syncer::{
        TipsetProcessor, TipsetProcessorError, TipsetRangeSyncer, TipsetRangeSyncerError,
    },
    validation::{TipsetValidationError, TipsetValidator},
};
use crate::libp2p::{
    hello::HelloRequest, NetworkEvent, NetworkMessage, PeerId, PeerManager, PubsubMessage,
};
use crate::message::SignedMessage;
use crate::message_pool::{MessagePool, Provider};
use crate::state_manager::StateManager;
use crate::{
    blocks::{Block, CreateTipsetError, FullTipset, Tipset, TipsetKey},
    networks::calculate_expected_epoch,
};
use cid::Cid;
use futures::{future::Future, stream::FuturesUnordered, StreamExt};
use fvm_ipld_blockstore::Blockstore;
use itertools::Itertools;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, error, info, trace, warn};

// Sync the messages for one or many tipsets @ a time
// Lotus uses a window size of 8: https://github.com/filecoin-project/lotus/blob/c1d22d8b3298fdce573107413729be608e72187d/chain/sync.go#L56
const DEFAULT_REQUEST_WINDOW: usize = 8;
const DEFAULT_TIPSET_SAMPLE_SIZE: usize = 1;
const DEFAULT_RECENT_STATE_ROOTS: i64 = 2000;

pub(in crate::chain_sync) type WorkerState = Arc<RwLock<SyncState>>;

type ChainMuxerFuture<T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send>>;

#[derive(Debug, Error)]
pub enum ChainMuxerError {
    #[error("Tipset processor error: {0}")]
    TipsetProcessor(#[from] TipsetProcessorError),
    #[error("Tipset range syncer error: {0}")]
    TipsetRangeSyncer(#[from] TipsetRangeSyncerError),
    #[error("Tipset validation error: {0}")]
    TipsetValidator(#[from] TipsetValidationError),
    #[error("Sending tipset on channel failed: {0}")]
    TipsetChannelSend(String),
    #[error("Receiving p2p network event failed: {0}")]
    P2PEventStreamReceive(String),
    #[error("Chain store error: {0}")]
    ChainStore(#[from] ChainStoreError),
    #[error("Chain exchange: {0}")]
    ChainExchange(String),
    #[error("Block error: {0}")]
    Block(#[from] CreateTipsetError),
    #[error("Following network unexpectedly failed: {0}")]
    NetworkFollowingFailure(String),
}

/// Structure that defines syncing configuration options
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(derive_quickcheck_arbitrary::Arbitrary))]
pub struct SyncConfig {
    /// Request window length for tipsets during chain exchange
    #[cfg_attr(test, arbitrary(gen(|g| u32::arbitrary(g) as _)))]
    pub request_window: usize,
    /// Number of recent state roots to keep in the database after `sync`
    /// and to include in the exported snapshot.
    pub recent_state_roots: i64,
    /// Sample size of tipsets to acquire before determining what the network
    /// head is
    #[cfg_attr(test, arbitrary(gen(|g| u32::arbitrary(g) as _)))]
    pub tipset_sample_size: usize,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            request_window: DEFAULT_REQUEST_WINDOW,
            recent_state_roots: DEFAULT_RECENT_STATE_ROOTS,
            tipset_sample_size: DEFAULT_TIPSET_SAMPLE_SIZE,
        }
    }
}

/// Represents the result of evaluating the network head tipset against the
/// local head tipset
enum NetworkHeadEvaluation {
    /// Local head is behind the network and needs move into the BOOTSTRAP state
    Behind {
        network_head: FullTipset,
        local_head: Arc<Tipset>,
    },
    /// Local head is the direct ancestor of the network head. The node should
    /// move into the FOLLOW state and immediately sync the network head
    InRange { network_head: FullTipset },
    /// Local head is at the same height as the network head. The node should
    /// move into the FOLLOW state and wait for new tipsets
    InSync,
}

/// The `ChainMuxer` handles events from the P2P network and orchestrates the
/// chain synchronization.
pub struct ChainMuxer<DB, M> {
    /// State of the `ChainSyncer` `Future` implementation
    state: ChainMuxerState,

    /// Syncing state of chain sync workers.
    worker_state: WorkerState,

    /// manages retrieving and updates state objects
    state_manager: Arc<StateManager<DB>>,

    /// Context to be able to send requests to P2P network
    network: SyncNetworkContext<DB>,

    /// Genesis tipset
    genesis: Arc<Tipset>,

    /// Bad blocks cache, updates based on invalid state transitions.
    /// Will mark any invalid blocks and all children as bad in this bounded
    /// cache
    bad_blocks: Arc<BadBlockCache>,

    /// Incoming network events to be handled by synchronizer
    net_handler: flume::Receiver<NetworkEvent>,

    /// Message pool
    mpool: Arc<MessagePool<M>>,

    /// Tipset channel sender
    tipset_sender: flume::Sender<Arc<Tipset>>,

    /// Tipset channel receiver
    tipset_receiver: flume::Receiver<Arc<Tipset>>,

    /// When `stateless_mode` is true, forest connects to the P2P network but does not sync to HEAD.
    stateless_mode: bool,
}

impl<DB, M> ChainMuxer<DB, M>
where
    DB: Blockstore + Sync + Send + 'static,
    M: Provider + Sync + Send + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state_manager: Arc<StateManager<DB>>,
        peer_manager: Arc<PeerManager>,
        mpool: Arc<MessagePool<M>>,
        network_send: flume::Sender<NetworkMessage>,
        network_rx: flume::Receiver<NetworkEvent>,
        genesis: Arc<Tipset>,
        stateless_mode: bool,
    ) -> Result<Self, ChainMuxerError> {
        let network =
            SyncNetworkContext::new(network_send, peer_manager, state_manager.blockstore_owned());
        let (tipset_sender, tipset_receiver) = flume::bounded(20);
        Ok(Self {
            state: ChainMuxerState::Idle,
            worker_state: Default::default(),
            network,
            genesis,
            bad_blocks: Arc::new(BadBlockCache::default()),
            net_handler: network_rx,
            mpool,
            tipset_sender,
            tipset_receiver,
            state_manager,
            stateless_mode,
        })
    }

    pub fn mpool(&self) -> &Arc<MessagePool<M>> {
        &self.mpool
    }

    pub fn tipset_sender(&self) -> &flume::Sender<Arc<Tipset>> {
        &self.tipset_sender
    }

    /// Returns the inner [`SyncNetworkContext`]
    pub fn sync_network_context(&self) -> &SyncNetworkContext<DB> {
        &self.network
    }

    /// Returns the bad blocks cache to be used outside of chain
    /// sync.
    pub fn bad_blocks(&self) -> &Arc<BadBlockCache> {
        &self.bad_blocks
    }

    /// Returns the sync worker state.
    pub fn sync_state(&self) -> &WorkerState {
        &self.worker_state
    }

    async fn get_full_tipset(
        network: SyncNetworkContext<DB>,
        chain_store: Arc<ChainStore<DB>>,
        peer_id: Option<PeerId>,
        tipset_keys: TipsetKey,
    ) -> Result<FullTipset, ChainMuxerError> {
        // Attempt to load from the store
        if let Ok(full_tipset) = Self::load_full_tipset(chain_store, tipset_keys.clone()) {
            return Ok(full_tipset);
        }
        // Load from the network
        network
            .chain_exchange_fts(peer_id, &tipset_keys.clone())
            .await
            .map_err(ChainMuxerError::ChainExchange)
    }

    fn load_full_tipset(
        chain_store: Arc<ChainStore<DB>>,
        tipset_keys: TipsetKey,
    ) -> Result<FullTipset, ChainMuxerError> {
        // Retrieve tipset from store based on passed in TipsetKey
        let ts = chain_store.chain_index.load_required_tipset(&tipset_keys)?;

        let blocks: Vec<_> = ts
            .block_headers()
            .iter()
            .map(|header| -> Result<Block, ChainMuxerError> {
                let (bls_msgs, secp_msgs) =
                    crate::chain::block_messages(chain_store.blockstore(), header)?;
                Ok(Block {
                    header: header.clone(),
                    bls_messages: bls_msgs,
                    secp_messages: secp_msgs,
                })
            })
            .try_collect()?;

        // Construct FullTipset
        let fts = FullTipset::new(blocks)?;
        Ok(fts)
    }

    async fn handle_peer_connected_event(
        network: SyncNetworkContext<DB>,
        chain_store: Arc<ChainStore<DB>>,
        peer_id: PeerId,
        genesis_block_cid: Cid,
    ) {
        // Query the heaviest TipSet from the store
        if network.peer_manager().is_peer_new(&peer_id) {
            // Since the peer is new, send them a hello request
            // Query the heaviest TipSet from the store
            let heaviest = chain_store.heaviest_tipset();
            let request = HelloRequest {
                heaviest_tip_set: heaviest.cids(),
                heaviest_tipset_height: heaviest.epoch(),
                heaviest_tipset_weight: heaviest.weight().clone().into(),
                genesis_cid: genesis_block_cid,
            };
            let (peer_id, moment_sent, response) =
                match network.hello_request(peer_id, request).await {
                    Ok(response) => response,
                    Err(e) => {
                        debug!("Hello request failed: {}", e);
                        return;
                    }
                };
            let dur = SystemTime::now()
                .duration_since(moment_sent)
                .unwrap_or_default();

            // Update the peer metadata based on the response
            match response {
                Some(_) => {
                    network.peer_manager().log_success(&peer_id, dur);
                }
                None => {
                    network.peer_manager().log_failure(&peer_id, dur);
                }
            }
        }
    }

    fn handle_peer_disconnected_event(network: SyncNetworkContext<DB>, peer_id: PeerId) {
        network.peer_manager().remove_peer(&peer_id);
        network.peer_manager().unmark_peer_bad(&peer_id);
    }

    fn handle_pubsub_message(mem_pool: Arc<MessagePool<M>>, message: SignedMessage) {
        if let Err(why) = mem_pool.add(message) {
            debug!(
                "GossipSub message could not be added to the mem pool: {}",
                why
            );
        }
    }

    // Wait for a network event and handle mandatory protocol responses (hello
    // requests, peer disconnects).
    async fn recv_gossipsub_event(
        p2p_messages: flume::Receiver<NetworkEvent>,
        network: SyncNetworkContext<DB>,
        chain_store: Arc<ChainStore<DB>>,
        genesis: &Tipset,
    ) -> Result<NetworkEvent, ChainMuxerError> {
        let event = match p2p_messages.recv_async().await {
            Ok(event) => event,
            Err(why) => {
                debug!("Receiving event from p2p event stream failed: {}", why);
                return Err(ChainMuxerError::P2PEventStreamReceive(why.to_string()));
            }
        };
        Self::inc_gossipsub_event_metrics(&event);
        Self::upd_peer_information(&event, network, chain_store, genesis);
        Ok(event)
    }

    // Increment the gossipsub event metrics.
    fn inc_gossipsub_event_metrics(event: &NetworkEvent) {
        let label = match event {
            NetworkEvent::HelloRequestInbound => metrics::values::HELLO_REQUEST_INBOUND,
            NetworkEvent::HelloResponseOutbound { .. } => metrics::values::HELLO_RESPONSE_OUTBOUND,
            NetworkEvent::HelloRequestOutbound => metrics::values::HELLO_REQUEST_OUTBOUND,
            NetworkEvent::HelloResponseInbound => metrics::values::HELLO_RESPONSE_INBOUND,
            NetworkEvent::PeerConnected(_) => metrics::values::PEER_CONNECTED,
            NetworkEvent::PeerDisconnected(_) => metrics::values::PEER_DISCONNECTED,
            NetworkEvent::PubsubMessage { message } => match message {
                PubsubMessage::Block(_) => metrics::values::PUBSUB_BLOCK,
                PubsubMessage::Message(_) => metrics::values::PUBSUB_MESSAGE,
            },
            NetworkEvent::ChainExchangeRequestOutbound => {
                metrics::values::CHAIN_EXCHANGE_REQUEST_OUTBOUND
            }
            NetworkEvent::ChainExchangeResponseInbound => {
                metrics::values::CHAIN_EXCHANGE_RESPONSE_INBOUND
            }
            NetworkEvent::ChainExchangeRequestInbound => {
                metrics::values::CHAIN_EXCHANGE_REQUEST_INBOUND
            }
            NetworkEvent::ChainExchangeResponseOutbound => {
                metrics::values::CHAIN_EXCHANGE_RESPONSE_OUTBOUND
            }
        };

        metrics::LIBP2P_MESSAGE_TOTAL.get_or_create(&label).inc();
    }

    // Keep our peer manager up to date.
    fn upd_peer_information(
        event: &NetworkEvent,
        network: SyncNetworkContext<DB>,
        chain_store: Arc<ChainStore<DB>>,
        genesis: &Tipset,
    ) {
        match event {
            NetworkEvent::PeerConnected(peer_id) => {
                let genesis_cid = *genesis.block_headers().first().cid();
                // Spawn and immediately move on to the next event
                tokio::task::spawn(Self::handle_peer_connected_event(
                    network,
                    chain_store,
                    *peer_id,
                    genesis_cid,
                ));
            }
            NetworkEvent::PeerDisconnected(peer_id) => {
                Self::handle_peer_disconnected_event(network, *peer_id);
            }
            _ => {}
        }
    }

    // Extract `Tipset` from the network event. `MessagePool` also happens here
    // (ugly, this should be refactored).
    async fn get_gossipsub_tipset(
        event: NetworkEvent,
        network: SyncNetworkContext<DB>,
        chain_store: Arc<ChainStore<DB>>,
        mem_pool: Arc<MessagePool<M>>,
    ) -> Result<Option<FullTipset>, ChainMuxerError> {
        match event {
            NetworkEvent::HelloRequestInbound => Ok(None),
            NetworkEvent::HelloResponseOutbound { request, source } => {
                let tipset_keys = TipsetKey::from(request.heaviest_tip_set.clone());
                Self::get_full_tipset(
                    network.clone(),
                    chain_store.clone(),
                    Some(source),
                    tipset_keys,
                )
                .await
                .inspect_err(|e| debug!("Querying full tipset failed: {}", e))
                .map(Some)
            }
            NetworkEvent::HelloRequestOutbound => Ok(None),
            NetworkEvent::HelloResponseInbound => Ok(None),
            NetworkEvent::PeerConnected(_) => Ok(None),
            NetworkEvent::PeerDisconnected(_) => Ok(None),
            NetworkEvent::PubsubMessage { message } => match message {
                PubsubMessage::Block(b) => Self::get_full_tipset(
                    network.clone(),
                    chain_store.clone(),
                    None,
                    TipsetKey::from(nunny::vec![*b.header.cid()]),
                )
                .await
                .map(Some),
                PubsubMessage::Message(m) => {
                    Self::handle_pubsub_message(mem_pool, m);
                    Ok(None)
                }
            },
            NetworkEvent::ChainExchangeRequestOutbound => Ok(None),
            NetworkEvent::ChainExchangeResponseInbound => Ok(None),
            NetworkEvent::ChainExchangeRequestInbound => Ok(None),
            NetworkEvent::ChainExchangeResponseOutbound => Ok(None),
        }
    }

    // Quick sanity checks across the blocks in a tipset. This rejects tipsets
    // with obvious issues before they're comitted to the database. Full
    // validation happens at a later stage.
    fn shallow_validate_tipset(
        tipset: &FullTipset,
        chain_store: &ChainStore<DB>,
        bad_block_cache: &BadBlockCache,
        genesis: &Tipset,
        block_delay: u32,
    ) -> Result<(), ChainMuxerError> {
        // Validate tipset
        if let Err(why) = TipsetValidator(tipset).validate(
            chain_store,
            Some(bad_block_cache),
            genesis,
            block_delay,
        ) {
            metrics::INVALID_TIPSET_TOTAL.inc();
            warn!(
                "Validating tipset received through GossipSub failed: {}",
                why
            );
            return Err(why.into());
        }

        // Store block messages in the block store
        for block in tipset.blocks() {
            block.persist(&chain_store.db)?;
        }

        Ok(())
    }

    fn stateless_node(&self) -> ChainMuxerFuture<(), ChainMuxerError> {
        let p2p_messages = self.net_handler.clone();
        let chain_store = self.state_manager.chain_store().clone();
        let network = self.network.clone();
        let genesis = self.genesis.clone();

        let future = async move {
            loop {
                Self::recv_gossipsub_event(
                    p2p_messages.clone(),
                    network.clone(),
                    chain_store.clone(),
                    &genesis,
                )
                .await?;
            }
        };

        Box::pin(future)
    }

    fn evaluate_network_head(&self) -> ChainMuxerFuture<NetworkHeadEvaluation, ChainMuxerError> {
        let p2p_messages = self.net_handler.clone();
        let chain_store = self.state_manager.chain_store().clone();
        let network = self.network.clone();
        let genesis = self.genesis.clone();
        let genesis_timestamp = self.genesis.block_headers().first().timestamp;
        let bad_block_cache = self.bad_blocks.clone();
        let mem_pool = self.mpool.clone();
        let tipset_sample_size = self.state_manager.sync_config().tipset_sample_size;
        let block_delay = self.state_manager.chain_config().block_delay_secs;

        let evaluator = async move {
            // If `local_epoch >= now_epoch`, return `NetworkHeadEvaluation::InSync`
            // and enter FOLLOW mode directly instead of waiting to collect `tipset_sample_size` tipsets.
            // Otherwise in some conditions, `forest-cli sync wait` takes very long to exit (only when the node enters FOLLOW mode)
            match (
                chain_store.heaviest_tipset().epoch(),
                calculate_expected_epoch(
                    chrono::Utc::now().timestamp() as u64,
                    genesis_timestamp,
                    block_delay,
                ) as i64,
            ) {
                (local_epoch, now_epoch) if local_epoch >= now_epoch => {
                    return Ok(NetworkHeadEvaluation::InSync)
                }
                (local_epoch, now_epoch) => {
                    info!("local head is behind the network, local_epoch: {local_epoch}, now_epoch: {now_epoch}");
                }
            };

            let mut tipsets = Vec::with_capacity(tipset_sample_size);
            while tipsets.len() < tipset_sample_size {
                let event = Self::recv_gossipsub_event(
                    p2p_messages.clone(),
                    network.clone(),
                    chain_store.clone(),
                    &genesis,
                )
                .await?;

                let tipset = match Self::get_gossipsub_tipset(
                    event,
                    network.clone(),
                    chain_store.clone(),
                    mem_pool.clone(),
                )
                .await?
                {
                    Some(tipset) => tipset,
                    None => continue,
                };

                if let Err(why) = Self::shallow_validate_tipset(
                    &tipset,
                    &chain_store,
                    &bad_block_cache,
                    &genesis,
                    block_delay,
                ) {
                    debug!("Processing GossipSub event failed: {:?}", why);
                    continue;
                }

                let now_epoch = calculate_expected_epoch(
                    chrono::Utc::now().timestamp() as u64,
                    genesis_timestamp,
                    block_delay,
                ) as i64;
                let is_block_valid = |block: &Block| -> bool {
                    let header = &block.header;
                    if !header.is_within_clock_drift() {
                        warn!(
                            "Skipping tipset with invalid block timestamp from the future, now_epoch: {now_epoch}, epoch: {}, timestamp: {}",
                            header.epoch, header.timestamp
                        );
                        false
                    } else if tipset.epoch() > now_epoch {
                        warn!(
                                "Skipping tipset with invalid epoch from the future, now_epoch: {now_epoch}, epoch: {}, timestamp: {}",
                                header.epoch, header.timestamp
                            );
                        false
                    } else {
                        true
                    }
                };

                if tipset.blocks().iter().all(is_block_valid) {
                    // Add to tipset sample
                    tipsets.push(tipset);
                }
            }

            // Find the heaviest tipset in the sample
            // Unwrapping is safe because we ensure the sample size is not 0
            let network_head = tipsets
                .into_iter()
                .max_by_key(|ts| ts.weight().clone())
                .unwrap();

            // Query the heaviest tipset in the store
            let local_head = chain_store.heaviest_tipset();

            // We are in sync if the local head weight is heavier or
            // as heavy as the network head
            if local_head.weight() >= network_head.weight() {
                return Ok(NetworkHeadEvaluation::InSync);
            }
            // We are in range if the network epoch is only 1 ahead of the local epoch
            if (network_head.epoch() - local_head.epoch()) == 1 {
                return Ok(NetworkHeadEvaluation::InRange { network_head });
            }
            // Local node is behind the network and we need to do an initial sync
            Ok(NetworkHeadEvaluation::Behind {
                network_head,
                local_head,
            })
        };

        Box::pin(evaluator)
    }

    fn bootstrap(
        &self,
        network_head: FullTipset,
        local_head: Arc<Tipset>,
    ) -> ChainMuxerFuture<(), ChainMuxerError> {
        // Instantiate a TipsetRangeSyncer
        let trs_state_manager = self.state_manager.clone();
        let trs_bad_block_cache = self.bad_blocks.clone();
        let trs_chain_store = self.state_manager.chain_store().clone();
        let trs_network = self.network.clone();
        let trs_tracker = self.worker_state.clone();
        let trs_genesis = self.genesis.clone();
        let tipset_range_syncer: ChainMuxerFuture<(), ChainMuxerError> = Box::pin(async move {
            let network_head_epoch = network_head.epoch();
            let tipset_range_syncer = match TipsetRangeSyncer::new(
                trs_tracker,
                Arc::new(network_head.into_tipset()),
                local_head,
                trs_state_manager,
                trs_network,
                trs_chain_store,
                trs_bad_block_cache,
                trs_genesis,
            ) {
                Ok(tipset_range_syncer) => tipset_range_syncer,
                Err(why) => {
                    metrics::TIPSET_RANGE_SYNC_FAILURE_TOTAL.inc();
                    return Err(ChainMuxerError::TipsetRangeSyncer(why));
                }
            };

            tipset_range_syncer
                .await
                .map_err(ChainMuxerError::TipsetRangeSyncer)?;

            metrics::HEAD_EPOCH.set(network_head_epoch);

            Ok(())
        });

        // The stream processor _must_ only error if the stream ends
        let p2p_messages = self.net_handler.clone();
        let network = self.network.clone();
        let chain_store = self.state_manager.chain_store().clone();
        let genesis = self.genesis.clone();
        let stream_processor: ChainMuxerFuture<(), ChainMuxerError> = Box::pin(async move {
            loop {
                Self::recv_gossipsub_event(
                    p2p_messages.clone(),
                    network.clone(),
                    chain_store.clone(),
                    &genesis,
                )
                .await?;
            }
        });

        let mut tasks = FuturesUnordered::new();
        tasks.push(tipset_range_syncer);
        tasks.push(stream_processor);

        Box::pin(async move {
            // The stream processor will not return unless the p2p event stream is closed.
            // In this case it will return with an error. Only wait for one task
            // to complete before returning to the caller
            match tasks.next().await {
                Some(Ok(_)) => Ok(()),
                Some(Err(e)) => Err(e),
                // This arm is reliably unreachable because the FuturesUnordered
                // has two futures and we only wait for one before returning
                None => unreachable!(),
            }
        })
    }

    fn follow(&self, tipset_opt: Option<FullTipset>) -> ChainMuxerFuture<(), ChainMuxerError> {
        // Instantiate a TipsetProcessor
        let tp_state_manager = self.state_manager.clone();
        let tp_network = self.network.clone();
        let tp_chain_store = self.state_manager.chain_store().clone();
        let tp_bad_block_cache = self.bad_blocks.clone();
        let tp_tipset_receiver = self.tipset_receiver.clone();
        let tp_tracker = self.worker_state.clone();
        let tp_genesis = self.genesis.clone();
        enum UnexpectedReturnKind {
            TipsetProcessor,
        }
        let tipset_processor: ChainMuxerFuture<UnexpectedReturnKind, ChainMuxerError> =
            Box::pin(async move {
                TipsetProcessor::new(
                    tp_tracker,
                    Box::pin(tp_tipset_receiver.into_stream()),
                    tp_state_manager,
                    tp_network,
                    tp_chain_store,
                    tp_bad_block_cache,
                    tp_genesis,
                )
                .await
                .map_err(ChainMuxerError::TipsetProcessor)?;

                Ok(UnexpectedReturnKind::TipsetProcessor)
            });

        // The stream processor _must_ only error if the p2p event stream ends or if the
        // tipset channel is unexpectedly closed
        let p2p_messages = self.net_handler.clone();
        let chain_store = self.state_manager.chain_store().clone();
        let network = self.network.clone();
        let genesis = self.genesis.clone();
        let bad_block_cache = self.bad_blocks.clone();
        let mem_pool = self.mpool.clone();
        let tipset_sender = self.tipset_sender.clone();
        let block_delay = self.state_manager.chain_config().block_delay_secs;
        let stream_processor: ChainMuxerFuture<UnexpectedReturnKind, ChainMuxerError> = Box::pin(
            async move {
                // If a tipset has been provided, pass it to the tipset processor
                if let Some(tipset) = tipset_opt {
                    if let Err(why) = tipset_sender
                        .send_async(Arc::new(tipset.into_tipset()))
                        .await
                    {
                        debug!("Sending tipset to TipsetProcessor failed: {}", why);
                        return Err(ChainMuxerError::TipsetChannelSend(why.to_string()));
                    };
                }
                loop {
                    let event = Self::recv_gossipsub_event(
                        p2p_messages.clone(),
                        network.clone(),
                        chain_store.clone(),
                        &genesis,
                    )
                    .await?;

                    let tipset = match Self::get_gossipsub_tipset(
                        event,
                        network.clone(),
                        chain_store.clone(),
                        mem_pool.clone(),
                    )
                    .await
                    {
                        Ok(Some(tipset)) => tipset,
                        Ok(None) => continue,
                        Err(why) => {
                            debug!("Processing GossipSub event failed: {:?}", why);
                            continue;
                        }
                    };

                    if let Err(why) = Self::shallow_validate_tipset(
                        &tipset,
                        &chain_store,
                        &bad_block_cache,
                        &genesis,
                        block_delay,
                    ) {
                        debug!("Processing GossipSub event failed: {:?}", why);
                        continue;
                    }

                    // Validate that the tipset is heavier that the heaviest
                    // tipset in the store
                    if tipset.weight() < chain_store.heaviest_tipset().weight() {
                        // Only send heavier Tipsets to the TipsetProcessor
                        trace!("Dropping tipset [Key = {:?}] that is not heavier than the heaviest tipset in the store", tipset.key());
                        continue;
                    }

                    if let Err(why) = tipset_sender
                        .send_async(Arc::new(tipset.into_tipset()))
                        .await
                    {
                        debug!("Sending tipset to TipsetProcessor failed: {}", why);
                        return Err(ChainMuxerError::TipsetChannelSend(why.to_string()));
                    };
                }
            },
        );

        let mut tasks = FuturesUnordered::new();
        tasks.push(tipset_processor);
        tasks.push(stream_processor);

        Box::pin(async move {
            // Only wait for one of the tasks to complete before returning to the caller
            match tasks.next().await {
                // Either the TipsetProcessor or the StreamProcessor has returned.
                // Both of these should be long running, so we have to return control
                // back to caller in order to direct the next action.
                Some(Ok(kind)) => {
                    // Log the expected return
                    match kind {
                        UnexpectedReturnKind::TipsetProcessor => {
                            Err(ChainMuxerError::NetworkFollowingFailure(String::from(
                                "Tipset processor unexpectedly returned",
                            )))
                        }
                    }
                }
                Some(Err(e)) => {
                    error!("Following the network failed unexpectedly: {}", e);
                    Err(e)
                }
                // This arm is reliably unreachable because the FuturesUnordered
                // has two futures and we only resolve one before returning
                None => unreachable!(),
            }
        })
    }
}

enum ChainMuxerState {
    Idle,
    Connect(ChainMuxerFuture<NetworkHeadEvaluation, ChainMuxerError>),
    Bootstrap(ChainMuxerFuture<(), ChainMuxerError>),
    Follow(ChainMuxerFuture<(), ChainMuxerError>),
    /// In stateless mode, forest still connects to the P2P swarm but does not sync to HEAD.
    Stateless(ChainMuxerFuture<(), ChainMuxerError>),
}

impl<DB, M> Future for ChainMuxer<DB, M>
where
    DB: Blockstore + Sync + Send + 'static,
    M: Provider + Sync + Send + 'static,
{
    type Output = ChainMuxerError;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        loop {
            match self.state {
                ChainMuxerState::Idle => {
                    if self.stateless_mode {
                        info!("Running chain muxer in stateless mode...");
                        self.state = ChainMuxerState::Stateless(self.stateless_node());
                    } else if self.state_manager.sync_config().tipset_sample_size == 0 {
                        // A standalone node might use this option to not be stuck waiting for P2P
                        // messages.
                        info!("Skip evaluating network head, assume in-sync.");
                        self.state = ChainMuxerState::Follow(self.follow(None));
                    } else {
                        // Create the connect future and set the state to connect
                        info!("Evaluating network head...");
                        self.state = ChainMuxerState::Connect(self.evaluate_network_head());
                    }
                }
                ChainMuxerState::Stateless(ref mut future) => {
                    if let Err(why) = std::task::ready!(future.as_mut().poll(cx)) {
                        return Poll::Ready(why);
                    }
                }
                ChainMuxerState::Connect(ref mut connect) => match connect.as_mut().poll(cx) {
                    Poll::Ready(Ok(evaluation)) => match evaluation {
                        NetworkHeadEvaluation::Behind {
                            network_head,
                            local_head,
                        } => {
                            info!("Local node is behind the network, starting BOOTSTRAP from LOCAL_HEAD = {} -> NETWORK_HEAD = {}", local_head.epoch(), network_head.epoch());
                            self.state = ChainMuxerState::Bootstrap(
                                self.bootstrap(network_head, local_head),
                            );
                        }
                        NetworkHeadEvaluation::InRange { network_head } => {
                            info!("Local node is within range of the NETWORK_HEAD = {}, starting FOLLOW", network_head.epoch());
                            self.state = ChainMuxerState::Follow(self.follow(Some(network_head)));
                        }
                        NetworkHeadEvaluation::InSync => {
                            info!("Local node is in sync with the network");
                            self.state = ChainMuxerState::Follow(self.follow(None));
                        }
                    },
                    Poll::Ready(Err(why)) => {
                        error!(
                            "Evaluating the network head failed, retrying. Error = {:?}",
                            why
                        );
                        metrics::NETWORK_HEAD_EVALUATION_ERRORS.inc();
                        self.state = ChainMuxerState::Idle;

                        // By default bail on errors
                        return Poll::Ready(why);
                    }
                    Poll::Pending => return Poll::Pending,
                },
                ChainMuxerState::Bootstrap(ref mut bootstrap) => {
                    match bootstrap.as_mut().poll(cx) {
                        Poll::Ready(Ok(_)) => {
                            info!("Bootstrap successfully completed, now evaluating the network head to ensure the node is in sync");
                            self.state = ChainMuxerState::Idle;
                        }
                        Poll::Ready(Err(why)) => {
                            error!("Bootstrapping failed, re-evaluating the network head to retry the bootstrap. Error = {:?}", why);
                            metrics::BOOTSTRAP_ERRORS.inc();
                            self.state = ChainMuxerState::Idle;
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }
                ChainMuxerState::Follow(ref mut follow) => match follow.as_mut().poll(cx) {
                    Poll::Ready(Ok(_)) => {
                        error!("Following the network unexpectedly ended without an error; restarting the sync process.");
                        metrics::FOLLOW_NETWORK_INTERRUPTIONS.inc();
                        self.state = ChainMuxerState::Idle;
                    }
                    Poll::Ready(Err(why)) => {
                        error!("Following the network failed, restarted. Error = {:?}", why);
                        metrics::FOLLOW_NETWORK_ERRORS.inc();
                        self.state = ChainMuxerState::Idle;
                    }
                    Poll::Pending => {
                        let tp_tracker = self.worker_state.clone();
                        tp_tracker
                            .write()
                            .set_stage(crate::chain_sync::SyncStage::Complete);

                        return Poll::Pending;
                    }
                },
            }
        }
    }
}
