// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Persistent ABCI mock application for the Dagora local network.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use dagora_abci::MAX_MESSAGE_BYTES;
use dagora_abci::tendermint::abci::{
    ExecTxResult, RequestCheckTx, RequestCommit, RequestFinalizeBlock, RequestInfo,
    RequestInitChain, ResponseCheckTx, ResponseCommit, ResponseFinalizeBlock, ResponseInfo,
    ResponseInitChain,
    abci_server::{Abci, AbciServer},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tonic::{Request, Response, Status};

const MAX_TRANSACTION_BYTES: usize = 256 * 1024;

#[derive(Clone)]
pub struct MockApplication {
    store: Arc<Mutex<Store>>,
}

struct Store {
    directory: PathBuf,
    _lock: File,
    state: AppState,
    pending: Option<PendingBlock>,
    fail_after_commit_once: bool,
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct AppState {
    chain_id: Option<String>,
    committed_height: i64,
    app_hash: [u8; 32],
    last_block_hash: Option<[u8; 32]>,
}

struct PendingBlock {
    request: RequestFinalizeBlock,
    response: ResponseFinalizeBlock,
}

impl MockApplication {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let directory = data_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&directory).context("create application data directory")?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join(".lock"))?;
        lock.try_lock()
            .context("application data directory is already in use")?;
        let state_path = directory.join("state.json");
        let state: AppState = match std::fs::read(&state_path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).context("decode committed application state")?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => AppState::default(),
            Err(error) => return Err(error.into()),
        };
        if state.chain_id.as_ref().is_some_and(String::is_empty)
            || state.committed_height < 0
            || (state.committed_height > 0
                && (state.chain_id.is_none() || state.last_block_hash.is_none()))
            || (state.committed_height == 0
                && (state.app_hash != [0; 32] || state.last_block_hash.is_some()))
        {
            bail!("invalid committed application state");
        }
        Ok(Self {
            store: Arc::new(Mutex::new(Store {
                directory,
                _lock: lock,
                state,
                pending: None,
                fail_after_commit_once: false,
            })),
        })
    }

    /// The binary exits after its first durable block commit, before the RPC response.
    pub fn with_fail_after_commit_once(self) -> Self {
        let mut store = self.store.lock().expect("application state mutex poisoned");
        store.fail_after_commit_once = !store.directory.join("commit-fault-fired").exists();
        drop(store);
        self
    }

    pub fn server(self) -> AbciServer<Self> {
        AbciServer::new(self)
            .max_decoding_message_size(MAX_MESSAGE_BYTES)
            .max_encoding_message_size(MAX_MESSAGE_BYTES)
    }
}

fn persist(directory: &Path, state: &AppState) -> Result<()> {
    let bytes = serde_json::to_vec(state)?;
    let temporary = directory.join("state.json.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    std::fs::rename(temporary, directory.join("state.json"))?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

#[tonic::async_trait]
impl Abci for MockApplication {
    async fn info(&self, _request: Request<RequestInfo>) -> Result<Response<ResponseInfo>, Status> {
        let store = self
            .store
            .lock()
            .map_err(|_| Status::internal("application state mutex poisoned"))?;
        Ok(Response::new(ResponseInfo {
            data: "dagora-abci-mock".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            app_version: 1,
            last_block_height: store.state.committed_height,
            last_block_app_hash: store.state.app_hash.to_vec(),
        }))
    }

    async fn init_chain(
        &self,
        request: Request<RequestInitChain>,
    ) -> Result<Response<ResponseInitChain>, Status> {
        let request = request.into_inner();
        if request.chain_id.is_empty() || request.initial_height != 1 {
            return Err(Status::invalid_argument(
                "chain_id must be nonempty and initial_height must be 1",
            ));
        }
        let mut store = self
            .store
            .lock()
            .map_err(|_| Status::internal("application state mutex poisoned"))?;
        if store.state.committed_height != 0 {
            return Err(Status::failed_precondition(
                "application has already executed a block",
            ));
        }
        match store.state.chain_id.as_ref() {
            Some(chain_id) if chain_id != &request.chain_id => {
                return Err(Status::failed_precondition(
                    "application belongs to another chain",
                ));
            }
            Some(_) => {}
            None => {
                let mut state = store.state.clone();
                state.chain_id = Some(request.chain_id);
                persist(&store.directory, &state)
                    .map_err(|error| Status::internal(error.to_string()))?;
                store.state = state;
            }
        }
        Ok(Response::new(ResponseInitChain {
            app_hash: store.state.app_hash.to_vec(),
            ..Default::default()
        }))
    }

    async fn check_tx(
        &self,
        request: Request<RequestCheckTx>,
    ) -> Result<Response<ResponseCheckTx>, Status> {
        Ok(Response::new(ResponseCheckTx {
            code: u32::from(request.get_ref().tx.len() > MAX_TRANSACTION_BYTES),
            ..Default::default()
        }))
    }

    async fn finalize_block(
        &self,
        request: Request<RequestFinalizeBlock>,
    ) -> Result<Response<ResponseFinalizeBlock>, Status> {
        let request = request.into_inner();
        let block_hash: [u8; 32] = request
            .hash
            .as_slice()
            .try_into()
            .map_err(|_| Status::invalid_argument("block hash must contain 32 bytes"))?;
        let mut store = self
            .store
            .lock()
            .map_err(|_| Status::internal("application state mutex poisoned"))?;
        if store.state.chain_id.is_none() {
            return Err(Status::failed_precondition("InitChain has not completed"));
        }
        if request.height
            != store
                .state
                .committed_height
                .checked_add(1)
                .ok_or_else(|| Status::out_of_range("application height exhausted"))?
        {
            return Err(Status::failed_precondition(
                "block height is not the next committed height",
            ));
        }
        if let Some(pending) = &store.pending {
            return if pending.request == request {
                Ok(Response::new(pending.response.clone()))
            } else {
                Err(Status::failed_precondition(
                    "a different block is pending at this height",
                ))
            };
        }
        let mut hash = Sha256::new();
        hash.update(store.state.app_hash);
        hash.update(block_hash);
        let response = ResponseFinalizeBlock {
            app_hash: hash.finalize().to_vec(),
            tx_results: request
                .txs
                .iter()
                .map(|tx| ExecTxResult {
                    code: u32::from(tx.starts_with(b"fail:")),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        store.pending = Some(PendingBlock {
            request,
            response: response.clone(),
        });
        Ok(Response::new(response))
    }

    async fn commit(
        &self,
        _request: Request<RequestCommit>,
    ) -> Result<Response<ResponseCommit>, Status> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| Status::internal("application state mutex poisoned"))?;
        if let Some(pending) = &store.pending {
            let state = AppState {
                chain_id: store.state.chain_id.clone(),
                committed_height: pending.request.height,
                app_hash: pending
                    .response
                    .app_hash
                    .as_slice()
                    .try_into()
                    .expect("mock hash has 32 bytes"),
                last_block_hash: Some(
                    pending
                        .request
                        .hash
                        .as_slice()
                        .try_into()
                        .expect("validated block hash"),
                ),
            };
            persist(&store.directory, &state)
                .map_err(|error| Status::internal(error.to_string()))?;
            store.state = state;
            store.pending = None;
            if store.fail_after_commit_once {
                let marker = store.directory.join("commit-fault-fired");
                let file =
                    File::create(marker).map_err(|error| Status::internal(error.to_string()))?;
                file.sync_all()
                    .map_err(|error| Status::internal(error.to_string()))?;
                File::open(&store.directory)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| Status::internal(error.to_string()))?;
                eprintln!(
                    "injected exit after durable Commit at height {}",
                    store.state.committed_height
                );
                std::process::exit(86);
            }
        }
        Ok(Response::new(ResponseCommit { retain_height: 0 }))
    }
}
