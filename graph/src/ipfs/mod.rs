use std::sync::Arc;

use anyhow::anyhow;
use cache::CachingClient;
use futures03::future::BoxFuture;
use futures03::stream::FuturesUnordered;
use futures03::stream::StreamExt;
use slog::info;
use slog::Logger;

use crate::util::security::SafeDisplay;

mod cache;
mod client;
mod content_path;
mod error;
mod gateway_client;
mod pool;
mod retry_policy;
mod rpc_client;
mod server_address;

pub mod test_utils;

pub use self::client::IpfsClient;
pub use self::client::IpfsRequest;
pub use self::client::IpfsResponse;
pub use self::content_path::ContentPath;
pub use self::error::IpfsError;
pub use self::error::RequestError;
pub use self::gateway_client::IpfsGatewayClient;
pub use self::pool::IpfsClientPool;
pub use self::retry_policy::RetryPolicy;
pub use self::rpc_client::IpfsRpcClient;
pub use self::server_address::ServerAddress;

pub type IpfsResult<T> = Result<T, IpfsError>;

/// Creates and returns the most appropriate IPFS client for the given IPFS server addresses.
///
/// If multiple IPFS server addresses are specified, an IPFS client pool is created internally
/// and for each IPFS request, the fastest client that can provide the content is
/// automatically selected and the response is streamed from that client.
///
/// All clients are set up to cache results
pub async fn new_ipfs_client<I, S>(
    server_addresses: I,
    logger: &Logger,
) -> IpfsResult<Arc<dyn IpfsClient>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut clients: Vec<Arc<dyn IpfsClient>> = Vec::new();

    for server_address in server_addresses {
        let server_address = server_address.as_ref();

        info!(
            logger,
            "Connecting to IPFS server at '{}'",
            SafeDisplay(server_address)
        );

        let client = use_first_valid_api(server_address, logger).await?;
        let client = Arc::new(CachingClient::new(client).await?);
        clients.push(client);
    }

    match clients.len() {
        0 => Err(IpfsError::InvalidServerAddress {
            input: "".to_owned(),
            source: anyhow!("at least one server address is required"),
        }),
        1 => Ok(clients.pop().unwrap().into()),
        n => {
            info!(logger, "Creating a pool of {} IPFS clients", n);

            let pool = IpfsClientPool::new(clients, logger);

            Ok(Arc::new(pool))
        }
    }
}

async fn use_first_valid_api(
    server_address: &str,
    logger: &Logger,
) -> IpfsResult<Arc<dyn IpfsClient>> {
    let supported_apis: Vec<BoxFuture<IpfsResult<Arc<dyn IpfsClient>>>> = vec![
        Box::pin(async {
            IpfsGatewayClient::new(server_address, logger)
                .await
                .map(|client| {
                    info!(
                        logger,
                        "Successfully connected to IPFS gateway at: '{}'",
                        SafeDisplay(server_address)
                    );

                    Arc::new(client) as Arc<dyn IpfsClient>
                })
        }),
        Box::pin(async {
            IpfsRpcClient::new(server_address, logger)
                .await
                .map(|client| {
                    info!(
                        logger,
                        "Successfully connected to IPFS RPC API at: '{}'",
                        SafeDisplay(server_address)
                    );

                    Arc::new(client) as Arc<dyn IpfsClient>
                })
        }),
    ];

    let mut stream = supported_apis.into_iter().collect::<FuturesUnordered<_>>();
    while let Some(result) = stream.next().await {
        match result {
            Ok(client) => return Ok(client),
            Err(err) if err.is_invalid_server() => {}
            Err(err) => return Err(err),
        };
    }

    Err(IpfsError::InvalidServer {
        server_address: server_address.parse()?,
        reason: anyhow!("unknown server kind"),
    })
}
