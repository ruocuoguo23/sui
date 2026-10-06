// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::{net::SocketAddr, time::Duration};

use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use dagora_abci::{
    MAX_MESSAGE_BYTES,
    tendermint::abci::{
        RequestCheckTx, RequestCommit, RequestFinalizeBlock, RequestInfo, RequestInitChain,
        abci_client::AbciClient,
    },
};
use dagora_core::{AppInfo, BlockExecutor, ChainBlock, ExecutionResult, TxResult};
use tonic::{
    Request,
    transport::{Channel, Endpoint},
};

const RPC_TIMEOUT: Duration = Duration::from_secs(5);

pub struct AbciExecutor {
    client: AbciClient<Channel>,
}

fn request<T>(body: T) -> Request<T> {
    let mut request = Request::new(body);
    request.set_timeout(RPC_TIMEOUT);
    request
}

pub async fn connect(address: SocketAddr) -> Result<AbciClient<Channel>> {
    let endpoint = Endpoint::from_shared(format!("http://{address}"))?
        .connect_timeout(Duration::from_secs(1))
        .timeout(RPC_TIMEOUT);
    let mut last_error = None;
    for attempt in 0..20 {
        match endpoint.connect().await {
            Ok(channel) => {
                return Ok(AbciClient::new(channel)
                    .max_decoding_message_size(MAX_MESSAGE_BYTES)
                    .max_encoding_message_size(MAX_MESSAGE_BYTES));
            }
            Err(error) => last_error = Some(error),
        }
        if attempt < 19 {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    Err(last_error.unwrap()).with_context(|| {
        format!("ABCI application at {address} did not become available during startup")
    })
}

impl AbciExecutor {
    pub fn new(client: AbciClient<Channel>) -> Self {
        Self { client }
    }
}

pub async fn check_tx(client: &mut AbciClient<Channel>, tx: Vec<u8>) -> Result<()> {
    let response = client
        .check_tx(request(RequestCheckTx { tx, r#type: 0 }))
        .await
        .context("ABCI CheckTx failed")?
        .into_inner();
    ensure!(
        response.code == 0,
        "CheckTx rejected transaction (code {}): {}",
        response.code,
        response.log
    );
    Ok(())
}

#[async_trait]
impl BlockExecutor for AbciExecutor {
    async fn info(&mut self) -> Result<AppInfo> {
        let response = self
            .client
            .info(request(RequestInfo {
                version: env!("CARGO_PKG_VERSION").into(),
                abci_version: "2.0.0".into(),
                ..Default::default()
            }))
            .await
            .context("ABCI Info failed")?
            .into_inner();
        Ok(AppInfo {
            height: response
                .last_block_height
                .try_into()
                .context("ABCI reported a negative block height")?,
            app_hash: response.last_block_app_hash,
        })
    }

    async fn init_chain(&mut self, chain_id: &str) -> Result<Vec<u8>> {
        let response = self
            .client
            .init_chain(request(RequestInitChain {
                chain_id: chain_id.into(),
                initial_height: 1,
                ..Default::default()
            }))
            .await
            .context("ABCI InitChain failed")?
            .into_inner();
        ensure!(
            response.validators.is_empty() && response.consensus_params.is_none(),
            "ABCI InitChain validator/consensus parameter updates are unsupported in fixed-committee v1"
        );
        Ok(response.app_hash)
    }

    async fn finalize_block(&mut self, block: &ChainBlock) -> Result<ExecutionResult> {
        let response = self
            .client
            .finalize_block(request(RequestFinalizeBlock {
                txs: block.transactions.clone(),
                hash: hex::decode(&block.block_hash).context("invalid block hash")?,
                height: block
                    .height
                    .try_into()
                    .context("block height exceeds ABCI int64")?,
                time: Some(prost_types::Timestamp {
                    seconds: (block.timestamp_ms / 1000).try_into()?,
                    nanos: ((block.timestamp_ms % 1000) * 1_000_000) as i32,
                }),
                ..Default::default()
            }))
            .await
            .context("ABCI FinalizeBlock failed")?
            .into_inner();
        ensure!(
            response.validator_updates.is_empty() && response.consensus_param_updates.is_none(),
            "ABCI validator/consensus parameter updates are unsupported in fixed-committee v1"
        );
        let result = ExecutionResult {
            app_hash: response.app_hash,
            tx_results: response
                .tx_results
                .into_iter()
                .map(|tx| TxResult {
                    code: tx.code,
                    data: tx.data,
                    gas_wanted: tx.gas_wanted,
                    gas_used: tx.gas_used,
                })
                .collect(),
        };
        result.validate(block)?;
        Ok(result)
    }

    async fn commit(&mut self) -> Result<()> {
        self.client
            .commit(request(RequestCommit {}))
            .await
            .context("ABCI Commit failed")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dagora_core::{ConsensusPosition, GENESIS_HASH};
    use tokio_stream::wrappers::TcpListenerStream;

    #[tokio::test]
    async fn grpc_adapter_preserves_application_failures_and_durable_info() {
        let directory = tempfile::tempdir().unwrap();
        let app = dagora_abci_mock::MockApplication::open(directory.path()).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(app.server())
                .serve_with_incoming(TcpListenerStream::new(listener)),
        );
        let client = connect(address).await.unwrap();
        let mut ingress = client.clone();
        let mut executor = AbciExecutor::new(client);
        assert_eq!(
            executor.init_chain("adapter-test").await.unwrap(),
            GENESIS_HASH
        );
        check_tx(&mut ingress, b"fail:valid-but-fails".to_vec())
            .await
            .unwrap();
        let mut block = ChainBlock {
            chain_id: "adapter-test".into(),
            position: ConsensusPosition {
                epoch: 0,
                index: 1,
                digest: "11".repeat(32),
            },
            height: 1,
            timestamp_ms: 1001,
            previous_block_hash: hex::encode(GENESIS_HASH),
            transactions: vec![b"ok".to_vec(), b"fail:valid-but-fails".to_vec()],
            block_hash: String::new(),
        };
        block.block_hash = block.calculate_hash().unwrap();
        let result = executor.finalize_block(&block).await.unwrap();
        assert_eq!(
            result
                .tx_results
                .iter()
                .map(|tx| tx.code)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(executor.info().await.unwrap().height, 0);
        executor.commit().await.unwrap();
        assert_eq!(
            executor.info().await.unwrap(),
            AppInfo {
                height: 1,
                app_hash: result.app_hash
            }
        );
        server.abort();
    }
}
