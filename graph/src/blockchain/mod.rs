//! The `blockchain` module exports the necessary traits and data structures to integrate a
//! blockchain into Graph Node. A blockchain is represented by an implementation of the `Blockchain`
//! trait which is the centerpiece of this module.

pub mod block_stream;
mod builder;
pub mod client;
mod empty_node_capabilities;
pub mod firehose_block_ingestor;
pub mod firehose_block_stream;
pub mod mock;
mod noop_runtime_adapter;
pub mod substreams_block_stream;
mod types;

// Try to reexport most of the necessary types
use crate::{
    cheap_clone::CheapClone,
    components::{
        metrics::subgraph::SubgraphInstanceMetrics,
        store::{
            DeploymentCursorTracker, DeploymentLocator, SourceableStore, StoredDynamicDataSource,
        },
        subgraph::{HostMetrics, InstanceDSTemplateInfo, MappingError},
        trigger_processor::RunnableTriggers,
    },
    data::subgraph::{UnifiedMappingApiVersion, MIN_SPEC_VERSION},
    data_source::{self, subgraph, DataSourceTemplateInfo},
    prelude::{DataSourceContext, DeploymentHash},
    runtime::{gas::GasCounter, AscHeap, HostExportError},
};
use crate::{
    components::store::BlockNumber,
    prelude::{thiserror::Error, LinkResolver},
};
use anyhow::{anyhow, Context, Error};
use async_trait::async_trait;
use graph_derive::CheapClone;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use slog::{error, Logger};
use std::{
    any::Any,
    collections::{HashMap, HashSet},
    fmt::{self, Debug},
    str::FromStr,
    sync::Arc,
};
use web3::types::H256;

pub use block_stream::{ChainHeadUpdateListener, ChainHeadUpdateStream, TriggersAdapter};
pub use builder::{BasicBlockchainBuilder, BlockchainBuilder};
pub use empty_node_capabilities::EmptyNodeCapabilities;
pub use noop_runtime_adapter::NoopRuntimeAdapter;
pub use types::{BlockHash, BlockPtr, BlockTime, ChainIdentifier, ExtendedBlockPtr};

use self::{
    block_stream::{BlockStream, FirehoseCursor},
    client::ChainClient,
};
use crate::components::network_provider::ChainName;

#[async_trait]
pub trait BlockIngestor: 'static + Send + Sync {
    async fn run(self: Box<Self>);
    fn network_name(&self) -> ChainName;
    fn kind(&self) -> BlockchainKind;
}

pub trait TriggersAdapterSelector<C: Blockchain>: Sync + Send {
    fn triggers_adapter(
        &self,
        loc: &DeploymentLocator,
        capabilities: &C::NodeCapabilities,
        unified_api_version: UnifiedMappingApiVersion,
    ) -> Result<Arc<dyn TriggersAdapter<C>>, Error>;
}

pub trait Block: Send + Sync {
    fn ptr(&self) -> BlockPtr;
    fn parent_ptr(&self) -> Option<BlockPtr>;

    fn number(&self) -> i32 {
        self.ptr().number
    }

    fn hash(&self) -> BlockHash {
        self.ptr().hash
    }

    fn parent_hash(&self) -> Option<BlockHash> {
        self.parent_ptr().map(|ptr| ptr.hash)
    }

    /// The data that should be stored for this block in the `ChainStore`
    /// TODO: Return ChainStoreData once it is available for all chains
    fn data(&self) -> Result<serde_json::Value, serde_json::Error> {
        Ok(serde_json::Value::Null)
    }

    fn timestamp(&self) -> BlockTime;
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// This is the root data for the chain store. This stucture provides backwards
/// compatibility with existing data for ethereum.
pub struct ChainStoreData {
    pub block: ChainStoreBlock,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// ChainStoreBlock is intended to standardize the information stored in the data
/// field of the ChainStore. All the chains should eventually return this type
/// on the data() implementation for block. This will ensure that any part of the
/// structured data can be relied upon for all chains.
pub struct ChainStoreBlock {
    /// Unix timestamp (seconds since epoch), can be stored as hex or decimal.
    timestamp: String,
    data: serde_json::Value,
}

impl ChainStoreBlock {
    pub fn new(unix_timestamp: i64, data: serde_json::Value) -> Self {
        Self {
            timestamp: unix_timestamp.to_string(),
            data,
        }
    }

    pub fn timestamp_str(&self) -> &str {
        &self.timestamp
    }

    pub fn timestamp(&self) -> i64 {
        let (rdx, i) = if self.timestamp.starts_with("0x") {
            (16, 2)
        } else {
            (10, 0)
        };

        i64::from_str_radix(&self.timestamp[i..], rdx).unwrap_or(0)
    }
}

#[async_trait]
// This is only `Debug` because some tests require that
pub trait Blockchain: Debug + Sized + Send + Sync + Unpin + 'static {
    const KIND: BlockchainKind;
    const ALIASES: &'static [&'static str] = &[];

    type Client: Debug + Sync + Send;
    // The `Clone` bound is used when reprocessing a block, because `triggers_in_block` requires an
    // owned `Block`. It would be good to come up with a way to remove this bound.
    type Block: Block + Clone + Debug + Default;

    type DataSource: DataSource<Self>;
    type UnresolvedDataSource: UnresolvedDataSource<Self>;

    type DataSourceTemplate: DataSourceTemplate<Self> + Clone;
    type UnresolvedDataSourceTemplate: UnresolvedDataSourceTemplate<Self> + Clone;

    /// Trigger data as parsed from the triggers adapter.
    type TriggerData: TriggerData + Ord + Send + Sync + Debug;

    /// Decoded trigger ready to be processed by the mapping.
    /// New implementations should have this be the same as `TriggerData`.
    type MappingTrigger: MappingTriggerTrait + Send + Sync + Debug;

    /// Trigger filter used as input to the triggers adapter.
    type TriggerFilter: TriggerFilter<Self>;

    type NodeCapabilities: NodeCapabilities<Self> + std::fmt::Display;

    /// A callback that is called after the triggers have been decoded and
    /// gets an opportunity to post-process triggers before they are run on
    /// hosts
    type DecoderHook: DecoderHook<Self> + Sync + Send;

    fn triggers_adapter(
        &self,
        log: &DeploymentLocator,
        capabilities: &Self::NodeCapabilities,
        unified_api_version: UnifiedMappingApiVersion,
    ) -> Result<Arc<dyn TriggersAdapter<Self>>, Error>;

    async fn new_block_stream(
        &self,
        deployment: DeploymentLocator,
        store: impl DeploymentCursorTracker,
        start_blocks: Vec<BlockNumber>,
        source_subgraph_stores: Vec<Arc<dyn SourceableStore>>,
        filter: Arc<TriggerFilterWrapper<Self>>,
        unified_api_version: UnifiedMappingApiVersion,
    ) -> Result<Box<dyn BlockStream<Self>>, Error>;

    /// Return the pointer for the latest block that we are aware of
    async fn chain_head_ptr(&self) -> Result<Option<BlockPtr>, Error>;

    async fn block_pointer_from_number(
        &self,
        logger: &Logger,
        number: BlockNumber,
    ) -> Result<BlockPtr, IngestorError>;

    async fn refetch_firehose_block(
        &self,
        logger: &Logger,
        cursor: FirehoseCursor,
    ) -> Result<Self::Block, Error>;

    fn is_refetch_block_required(&self) -> bool;

    fn runtime(&self) -> anyhow::Result<(Arc<dyn RuntimeAdapter<Self>>, Self::DecoderHook)>;

    fn chain_client(&self) -> Arc<ChainClient<Self>>;

    async fn block_ingestor(&self) -> anyhow::Result<Box<dyn BlockIngestor>>;
}

#[derive(Error, Debug)]
pub enum IngestorError {
    /// The Ethereum node does not know about this block for some reason, probably because it
    /// disappeared in a chain reorg.
    #[error("Block data unavailable, block was likely uncled (block hash = {0:?})")]
    BlockUnavailable(H256),

    /// The Ethereum node does not know about this block for some reason, probably because it
    /// disappeared in a chain reorg.
    #[error("Receipt for tx {1:?} unavailable, block was likely uncled (block hash = {0:?})")]
    ReceiptUnavailable(H256, H256),

    /// The Ethereum node does not know about this block for some reason
    #[error("Transaction receipts for block (block hash = {0:?}) is unavailable")]
    BlockReceiptsUnavailable(H256),

    /// The Ethereum node does not know about this block for some reason
    #[error("Received confliciting block receipts for block (block hash = {0:?})")]
    BlockReceiptsMismatched(H256),

    /// An unexpected error occurred.
    #[error("Ingestor error: {0:#}")]
    Unknown(#[from] Error),
}

impl From<web3::Error> for IngestorError {
    fn from(e: web3::Error) -> Self {
        IngestorError::Unknown(anyhow::anyhow!(e))
    }
}

/// The `TriggerFilterWrapper` is a higher-level wrapper around the chain-specific `TriggerFilter`,
/// enabling subgraph-based trigger filtering for subgraph datasources. This abstraction is necessary
/// because subgraph filtering operates at a higher level than chain-based filtering. By using this wrapper,
/// we reduce code duplication, allowing subgraph-based filtering to be implemented once, instead of
/// duplicating it across different chains.
#[derive(Debug)]
pub struct TriggerFilterWrapper<C: Blockchain> {
    pub chain_filter: Arc<C::TriggerFilter>,
    pub subgraph_filter: Vec<SubgraphFilter>,
}

#[derive(Clone, Debug)]
pub struct SubgraphFilter {
    pub subgraph: DeploymentHash,
    pub start_block: BlockNumber,
    pub entities: Vec<String>,
    pub manifest_idx: u32,
}

impl<C: Blockchain> TriggerFilterWrapper<C> {
    pub fn new(filter: C::TriggerFilter, subgraph_filter: Vec<SubgraphFilter>) -> Self {
        Self {
            chain_filter: Arc::new(filter),
            subgraph_filter,
        }
    }
}

impl<C: Blockchain> Clone for TriggerFilterWrapper<C> {
    fn clone(&self) -> Self {
        Self {
            chain_filter: self.chain_filter.cheap_clone(),
            subgraph_filter: self.subgraph_filter.clone(),
        }
    }
}

pub trait TriggerFilter<C: Blockchain>: Default + Clone + Send + Sync {
    fn from_data_sources<'a>(
        data_sources: impl Iterator<Item = &'a C::DataSource> + Clone,
    ) -> Self {
        let mut this = Self::default();
        this.extend(data_sources);
        this
    }

    fn extend_with_template(&mut self, data_source: impl Iterator<Item = C::DataSourceTemplate>);

    fn extend<'a>(&mut self, data_sources: impl Iterator<Item = &'a C::DataSource> + Clone);

    fn node_capabilities(&self) -> C::NodeCapabilities;

    fn to_firehose_filter(self) -> Vec<prost_types::Any>;
}

pub trait DataSource<C: Blockchain>: 'static + Sized + Send + Sync + Clone {
    fn from_template_info(
        info: InstanceDSTemplateInfo,
        template: &data_source::DataSourceTemplate<C>,
    ) -> Result<Self, Error>;

    fn from_stored_dynamic_data_source(
        template: &C::DataSourceTemplate,
        stored: StoredDynamicDataSource,
    ) -> Result<Self, Error>;

    fn address(&self) -> Option<&[u8]>;
    fn start_block(&self) -> BlockNumber;
    fn end_block(&self) -> Option<BlockNumber>;
    fn name(&self) -> &str;
    fn kind(&self) -> &str;
    fn network(&self) -> Option<&str>;
    fn context(&self) -> Arc<Option<DataSourceContext>>;
    fn creation_block(&self) -> Option<BlockNumber>;
    fn api_version(&self) -> semver::Version;

    fn min_spec_version(&self) -> semver::Version {
        MIN_SPEC_VERSION
    }

    fn runtime(&self) -> Option<Arc<Vec<u8>>>;

    fn handler_kinds(&self) -> HashSet<&str>;

    /// Checks if `trigger` matches this data source, and if so decodes it into a `MappingTrigger`.
    /// A return of `Ok(None)` mean the trigger does not match.
    ///
    /// Performance note: This is very hot code, because in the worst case it could be called a
    /// quadratic T*D times where T is the total number of triggers in the chain and D is the number
    /// of data sources in the subgraph. So it could be called billions, or even trillions, of times
    /// in the sync time of a subgraph.
    ///
    /// This is typicaly reduced by the triggers being pre-filtered in the block stream. But with
    /// dynamic data sources the block stream does not filter on the dynamic parameters, so the
    /// matching should efficently discard false positives.
    fn match_and_decode(
        &self,
        trigger: &C::TriggerData,
        block: &Arc<C::Block>,
        logger: &Logger,
    ) -> Result<Option<TriggerWithHandler<C>>, Error>;

    fn is_duplicate_of(&self, other: &Self) -> bool;

    fn as_stored_dynamic_data_source(&self) -> StoredDynamicDataSource;

    /// Used as part of manifest validation. If there are no errors, return an empty vector.
    fn validate(&self, spec_version: &semver::Version) -> Vec<Error>;

    fn has_expired(&self, block: BlockNumber) -> bool {
        self.end_block()
            .map_or(false, |end_block| block > end_block)
    }

    fn has_declared_calls(&self) -> bool {
        false
    }
}

#[async_trait]
pub trait UnresolvedDataSourceTemplate<C: Blockchain>:
    'static + Sized + Send + Sync + DeserializeOwned + Default
{
    async fn resolve(
        self,
        resolver: &Arc<dyn LinkResolver>,
        logger: &Logger,
        manifest_idx: u32,
    ) -> Result<C::DataSourceTemplate, anyhow::Error>;
}

pub trait DataSourceTemplate<C: Blockchain>: Send + Sync + Debug {
    fn api_version(&self) -> semver::Version;
    fn runtime(&self) -> Option<Arc<Vec<u8>>>;
    fn name(&self) -> &str;
    fn manifest_idx(&self) -> u32;
    fn kind(&self) -> &str;
    fn info(&self) -> DataSourceTemplateInfo {
        DataSourceTemplateInfo {
            api_version: self.api_version(),
            runtime: self.runtime(),
            name: self.name().to_string(),
            manifest_idx: Some(self.manifest_idx()),
            kind: self.kind().to_string(),
        }
    }
}

#[async_trait]
pub trait UnresolvedDataSource<C: Blockchain>:
    'static + Sized + Send + Sync + DeserializeOwned
{
    async fn resolve(
        self,
        resolver: &Arc<dyn LinkResolver>,
        logger: &Logger,
        manifest_idx: u32,
    ) -> Result<C::DataSource, anyhow::Error>;
}

#[derive(Debug)]
pub enum Trigger<C: Blockchain> {
    Chain(C::TriggerData),
    Subgraph(subgraph::TriggerData),
}

impl<C: Blockchain> Trigger<C> {
    pub fn as_chain(&self) -> Option<&C::TriggerData> {
        match self {
            Trigger::Chain(data) => Some(data),
            _ => None,
        }
    }

    pub fn as_subgraph(&self) -> Option<&subgraph::TriggerData> {
        match self {
            Trigger::Subgraph(data) => Some(data),
            _ => None,
        }
    }
}

impl<C: Blockchain> Eq for Trigger<C> where C::TriggerData: Eq {}

impl<C: Blockchain> PartialEq for Trigger<C>
where
    C::TriggerData: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Trigger::Chain(data1), Trigger::Chain(data2)) => data1 == data2,
            (Trigger::Subgraph(a), Trigger::Subgraph(b)) => a == b,
            _ => false,
        }
    }
}

impl<C: Blockchain> Clone for Trigger<C>
where
    C::TriggerData: Clone,
{
    fn clone(&self) -> Self {
        match self {
            Trigger::Chain(data) => Trigger::Chain(data.clone()),
            Trigger::Subgraph(data) => Trigger::Subgraph(data.clone()),
        }
    }
}

impl<C: Blockchain> Ord for Trigger<C>
where
    C::TriggerData: Ord,
{
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Trigger::Chain(data1), Trigger::Chain(data2)) => data1.cmp(data2),
            (Trigger::Subgraph(_), Trigger::Chain(_)) => std::cmp::Ordering::Greater,
            (Trigger::Chain(_), Trigger::Subgraph(_)) => std::cmp::Ordering::Less,
            (Trigger::Subgraph(t1), Trigger::Subgraph(t2)) => t1.cmp(t2),
        }
    }
}

impl<C: Blockchain> PartialOrd for Trigger<C> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

pub trait TriggerData {
    /// If there is an error when processing this trigger, this will called to add relevant context.
    /// For example an useful return is: `"block #<N> (<hash>), transaction <tx_hash>".
    fn error_context(&self) -> String;

    /// If this trigger can only possibly match data sources with a specific address, then it can be
    /// returned here for improved trigger matching performance, which helps subgraphs with many
    /// data sources. But this optimization is not required, so returning `None` is always correct.
    ///
    /// When this does return `Some`, make sure that the `DataSource::address` of matching data
    /// sources is equal to the addresssed returned here.
    fn address_match(&self) -> Option<&[u8]>;
}

pub trait MappingTriggerTrait {
    /// If there is an error when processing this trigger, this will called to add relevant context.
    /// For example an useful return is: `"block #<N> (<hash>), transaction <tx_hash>".
    fn error_context(&self) -> String;
}

/// A callback that is called after the triggers have been decoded.
#[async_trait]
pub trait DecoderHook<C: Blockchain> {
    async fn after_decode<'a>(
        &self,
        logger: &Logger,
        block_ptr: &BlockPtr,
        triggers: Vec<RunnableTriggers<'a, C>>,
        metrics: &Arc<SubgraphInstanceMetrics>,
    ) -> Result<Vec<RunnableTriggers<'a, C>>, MappingError>;
}

/// A decoder hook that does nothing and just returns the triggers that were
/// passed in
pub struct NoopDecoderHook;

#[async_trait]
impl<C: Blockchain> DecoderHook<C> for NoopDecoderHook {
    async fn after_decode<'a>(
        &self,
        _: &Logger,
        _: &BlockPtr,
        triggers: Vec<RunnableTriggers<'a, C>>,
        _: &Arc<SubgraphInstanceMetrics>,
    ) -> Result<Vec<RunnableTriggers<'a, C>>, MappingError> {
        Ok(triggers)
    }
}

pub struct HostFnCtx<'a> {
    pub logger: Logger,
    pub block_ptr: BlockPtr,
    pub heap: &'a mut dyn AscHeap,
    pub gas: GasCounter,
    pub metrics: Arc<HostMetrics>,
}

/// Host fn that receives one u32 argument and returns an u32.
/// The name for an AS fuction is in the format `<namespace>.<function>`.
#[derive(Clone, CheapClone)]
pub struct HostFn {
    pub name: &'static str,
    pub func: Arc<dyn Send + Sync + Fn(HostFnCtx, u32) -> Result<u32, HostExportError>>,
}

pub trait RuntimeAdapter<C: Blockchain>: Send + Sync {
    fn host_fns(&self, ds: &data_source::DataSource<C>) -> Result<Vec<HostFn>, Error>;
}

pub trait NodeCapabilities<C: Blockchain> {
    fn from_data_sources(data_sources: &[C::DataSource]) -> Self;
}

/// Blockchain technologies supported by Graph Node.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockchainKind {
    /// Ethereum itself or chains that are compatible.
    Ethereum,

    /// NEAR chains (Mainnet, Testnet) or chains that are compatible
    Near,

    Substreams,
}

impl fmt::Display for BlockchainKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let value = match self {
            BlockchainKind::Ethereum => "ethereum",
            BlockchainKind::Near => "near",
            BlockchainKind::Substreams => "substreams",
        };
        write!(f, "{}", value)
    }
}

impl FromStr for BlockchainKind {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ethereum" => Ok(BlockchainKind::Ethereum),
            "near" => Ok(BlockchainKind::Near),
            "substreams" => Ok(BlockchainKind::Substreams),
            "subgraph" => Ok(BlockchainKind::Ethereum), // TODO(krishna): We should detect the blockchain kind from the source subgraph
            _ => Err(anyhow!("unknown blockchain kind {}", s)),
        }
    }
}

impl BlockchainKind {
    pub fn from_manifest(manifest: &serde_yaml::Mapping) -> Result<Self, Error> {
        use serde_yaml::Value;

        // The `kind` field of the first data source in the manifest.
        //
        // Split by `/` to, for example, read 'ethereum' in 'ethereum/contracts'.
        manifest
            .get(&Value::String("dataSources".to_owned()))
            .and_then(|ds| ds.as_sequence())
            .and_then(|ds| ds.first())
            .and_then(|ds| ds.as_mapping())
            .and_then(|ds| ds.get(&Value::String("kind".to_owned())))
            .and_then(|kind| kind.as_str())
            .and_then(|kind| kind.split('/').next())
            .context("invalid manifest")
            .and_then(BlockchainKind::from_str)
    }
}

/// A collection of blockchains, keyed by `BlockchainKind` and network.
#[derive(Default, Debug, Clone)]
pub struct BlockchainMap(HashMap<(BlockchainKind, ChainName), Arc<dyn Any + Send + Sync>>);

impl BlockchainMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn iter(
        &self,
    ) -> impl Iterator<Item = (&(BlockchainKind, ChainName), &Arc<dyn Any + Sync + Send>)> {
        self.0.iter()
    }

    pub fn insert<C: Blockchain>(&mut self, network: ChainName, chain: Arc<C>) {
        self.0.insert((C::KIND, network), chain);
    }

    pub fn get_all_by_kind<C: Blockchain>(
        &self,
        kind: BlockchainKind,
    ) -> Result<Vec<Arc<C>>, Error> {
        self.0
            .iter()
            .flat_map(|((k, _), chain)| {
                if k.eq(&kind) {
                    Some(chain.cheap_clone().downcast().map_err(|_| {
                        anyhow!("unable to downcast, wrong type for blockchain {}", C::KIND)
                    }))
                } else {
                    None
                }
            })
            .collect::<Result<Vec<Arc<C>>, Error>>()
    }

    pub fn get<C: Blockchain>(&self, network: ChainName) -> Result<Arc<C>, Error> {
        self.0
            .get(&(C::KIND, network.clone()))
            .with_context(|| format!("no network {} found on chain {}", network, C::KIND))?
            .cheap_clone()
            .downcast()
            .map_err(|_| anyhow!("unable to downcast, wrong type for blockchain {}", C::KIND))
    }
}

pub type TriggerWithHandler<C> = data_source::TriggerWithHandler<<C as Blockchain>::MappingTrigger>;
