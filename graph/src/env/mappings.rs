use std::fmt;
use std::path::PathBuf;

use anyhow::anyhow;

use super::*;
#[derive(Clone)]
pub struct EnvVarsMapping {
    /// Forces the cache eviction policy to take its own memory overhead into account.
    ///
    /// Set by the flag `DEAD_WEIGHT`. Setting `DEAD_WEIGHT` is dangerous since it can lead to a
    /// situation where an empty cache is bigger than the max_weight,
    /// which leads to a panic. Off by default.
    pub entity_cache_dead_weight: bool,
    /// Size limit of the entity LFU cache.
    ///
    /// Set by the environment variable `GRAPH_ENTITY_CACHE_SIZE` (expressed in
    /// kilobytes). The default value is 10 megabytes.
    pub entity_cache_size: usize,
    /// Set by the environment variable `GRAPH_MAX_API_VERSION`. The default
    /// value is `0.0.8`.
    pub max_api_version: Version,
    /// Set by the environment variable `GRAPH_MAPPING_HANDLER_TIMEOUT`
    /// (expressed in seconds). No default is provided.
    pub timeout: Option<Duration>,
    /// Maximum stack size for the WASM runtime.
    ///
    /// Set by the environment variable `GRAPH_RUNTIME_MAX_STACK_SIZE`
    /// (expressed in bytes). The default value is 512KiB.
    pub max_stack_size: usize,

    /// Set by the environment variable `GRAPH_MAX_IPFS_CACHE_FILE_SIZE`
    /// (expressed in bytes). The default value is 1MiB.
    pub max_ipfs_cache_file_size: usize,
    /// Set by the environment variable `GRAPH_MAX_IPFS_CACHE_SIZE`. The default
    /// value is 50 items.
    pub max_ipfs_cache_size: u64,
    /// The timeout for all IPFS requests.
    ///
    /// Set by the environment variable `GRAPH_IPFS_TIMEOUT` (expressed in
    /// seconds). The default value is 60s.
    pub ipfs_timeout: Duration,
    /// Sets the `ipfs.map` file size limit.
    ///
    /// Set by the environment variable `GRAPH_MAX_IPFS_MAP_FILE_SIZE_LIMIT`
    /// (expressed in bytes). The default value is 256MiB.
    pub max_ipfs_map_file_size: usize,
    /// Sets the `ipfs.cat` file size limit.
    ///
    /// Set by the environment variable `GRAPH_MAX_IPFS_FILE_BYTES` (expressed in
    /// bytes). Defaults to 25 MiB.
    pub max_ipfs_file_bytes: usize,

    /// Limits per second requests to IPFS for file data sources.
    ///
    /// Set by the environment variable `GRAPH_IPFS_REQUEST_LIMIT`. Defaults to 100.
    pub ipfs_request_limit: u16,
    /// Limit of max IPFS attempts to retrieve a file.
    ///
    /// Set by the environment variable `GRAPH_IPFS_MAX_ATTEMPTS`. Defaults to 100000.
    pub ipfs_max_attempts: usize,

    /// Set by the flag `GRAPH_IPFS_CACHE_LOCATION`.
    pub ipfs_cache_location: Option<PathBuf>,

    /// Set by the flag `GRAPH_ALLOW_NON_DETERMINISTIC_IPFS`. Off by
    /// default.
    pub allow_non_deterministic_ipfs: bool,

    /// Set by the flag `GRAPH_DISABLE_DECLARED_CALLS`. Disables performing
    /// eth calls before running triggers; instead eth calls happen when
    /// mappings call `ethereum.call`. Off by default.
    pub disable_declared_calls: bool,

    /// Set by the flag `GRAPH_STORE_ERRORS_ARE_NON_DETERMINISTIC`. Off by
    /// default. Setting this to `true` will revert to the old behavior of
    /// treating all store errors as nondeterministic. This is a temporary
    /// measure and can be removed after 2025-07-01, once we are sure the
    /// new behavior works as intended.
    pub store_errors_are_nondeterministic: bool,

    /// Maximum backoff time for FDS requests. Set by
    /// `GRAPH_FDS_MAX_BACKOFF` in seconds, defaults to 600.
    pub fds_max_backoff: Duration,
}

// This does not print any values avoid accidentally leaking any sensitive env vars
impl fmt::Debug for EnvVarsMapping {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "env vars")
    }
}

impl TryFrom<InnerMappingHandlers> for EnvVarsMapping {
    type Error = anyhow::Error;

    fn try_from(x: InnerMappingHandlers) -> Result<Self, Self::Error> {
        let ipfs_cache_location = x
            .ipfs_cache_location
            .map(PathBuf::from)
            .map(validate_ipfs_cache_location)
            .transpose()?;

        let vars = Self {
            entity_cache_dead_weight: x.entity_cache_dead_weight.0,
            entity_cache_size: x.entity_cache_size_in_kb * 1000,

            max_api_version: x.max_api_version,
            timeout: x.mapping_handler_timeout_in_secs.map(Duration::from_secs),
            max_stack_size: x.runtime_max_stack_size.0 .0,

            max_ipfs_cache_file_size: x.max_ipfs_cache_file_size.0,
            max_ipfs_cache_size: x.max_ipfs_cache_size,
            ipfs_timeout: Duration::from_secs(x.ipfs_timeout_in_secs),
            max_ipfs_map_file_size: x.max_ipfs_map_file_size.0,
            max_ipfs_file_bytes: x.max_ipfs_file_bytes.0,
            ipfs_request_limit: x.ipfs_request_limit,
            ipfs_max_attempts: x.ipfs_max_attempts,
            ipfs_cache_location: ipfs_cache_location,
            allow_non_deterministic_ipfs: x.allow_non_deterministic_ipfs.0,
            disable_declared_calls: x.disable_declared_calls.0,
            store_errors_are_nondeterministic: x.store_errors_are_nondeterministic.0,
            fds_max_backoff: Duration::from_secs(x.fds_max_backoff),
        };
        Ok(vars)
    }
}

#[derive(Clone, Debug, Envconfig)]
pub struct InnerMappingHandlers {
    #[envconfig(from = "DEAD_WEIGHT", default = "false")]
    entity_cache_dead_weight: EnvVarBoolean,
    #[envconfig(from = "GRAPH_ENTITY_CACHE_SIZE", default = "10000")]
    entity_cache_size_in_kb: usize,
    #[envconfig(from = "GRAPH_MAX_API_VERSION", default = "0.0.9")]
    max_api_version: Version,
    #[envconfig(from = "GRAPH_MAPPING_HANDLER_TIMEOUT")]
    mapping_handler_timeout_in_secs: Option<u64>,
    #[envconfig(from = "GRAPH_RUNTIME_MAX_STACK_SIZE", default = "")]
    runtime_max_stack_size: WithDefaultUsize<NoUnderscores<usize>, { 512 * 1024 }>,

    // IPFS.
    #[envconfig(from = "GRAPH_MAX_IPFS_CACHE_FILE_SIZE", default = "")]
    max_ipfs_cache_file_size: WithDefaultUsize<usize, { 1024 * 1024 }>,
    #[envconfig(from = "GRAPH_MAX_IPFS_CACHE_SIZE", default = "50")]
    max_ipfs_cache_size: u64,
    #[envconfig(from = "GRAPH_IPFS_TIMEOUT", default = "60")]
    ipfs_timeout_in_secs: u64,
    #[envconfig(from = "GRAPH_MAX_IPFS_MAP_FILE_SIZE", default = "")]
    max_ipfs_map_file_size: WithDefaultUsize<usize, { 256 * 1024 * 1024 }>,
    #[envconfig(from = "GRAPH_MAX_IPFS_FILE_BYTES", default = "")]
    max_ipfs_file_bytes: WithDefaultUsize<usize, { 25 * 1024 * 1024 }>,
    #[envconfig(from = "GRAPH_IPFS_REQUEST_LIMIT", default = "100")]
    ipfs_request_limit: u16,
    #[envconfig(from = "GRAPH_IPFS_MAX_ATTEMPTS", default = "100000")]
    ipfs_max_attempts: usize,
    #[envconfig(from = "GRAPH_IPFS_CACHE_LOCATION")]
    ipfs_cache_location: Option<String>,
    #[envconfig(from = "GRAPH_ALLOW_NON_DETERMINISTIC_IPFS", default = "false")]
    allow_non_deterministic_ipfs: EnvVarBoolean,
    #[envconfig(from = "GRAPH_DISABLE_DECLARED_CALLS", default = "false")]
    disable_declared_calls: EnvVarBoolean,
    #[envconfig(from = "GRAPH_STORE_ERRORS_ARE_NON_DETERMINISTIC", default = "false")]
    store_errors_are_nondeterministic: EnvVarBoolean,
    #[envconfig(from = "GRAPH_FDS_MAX_BACKOFF", default = "600")]
    fds_max_backoff: u64,
}

fn validate_ipfs_cache_location(path: PathBuf) -> Result<PathBuf, anyhow::Error> {
    if path.starts_with("redis://") {
        // We validate this later when we set up the Redis client
        return Ok(path);
    }
    let path = path.canonicalize().map_err(|e| {
        anyhow!(
            "GRAPH_IPFS_CACHE_LOCATION {} is invalid: {e}",
            path.display()
        )
    })?;
    if !path.is_absolute() {
        return Err(anyhow::anyhow!(
            "GRAPH_IPFS_CACHE_LOCATION must be an absolute path: {}",
            path.display()
        ));
    }
    if !path.is_dir() {
        return Err(anyhow::anyhow!(
            "GRAPH_IPFS_CACHE_LOCATION must be a directory: {}",
            path.display()
        ));
    }
    let metadata = path.metadata()?;
    if metadata.permissions().readonly() {
        return Err(anyhow::anyhow!(
            "GRAPH_IPFS_CACHE_LOCATION must be a writable directory: {}",
            path.display()
        ));
    }
    Ok(path)
}
