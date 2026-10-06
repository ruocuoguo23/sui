// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

mod abci;
mod config;

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use clap::{Parser, Subcommand};
use consensus_core::{Priority, TransactionClient, storage::Store};
use dagora_abci::tendermint::abci::abci_client::AbciClient;
use dagora_core::{
    ChainBlock, ExecutionResult, GENESIS_HASH, Journal, MAX_TRANSACTION_BYTES, start_consensus,
};
use fastcrypto::hash::{HashFunction, Sha256};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};
use tonic::transport::Channel;

#[derive(Parser)]
#[command(about = "Local Mysticeti ordering chain with a separate ABCI application")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate fresh keys and configurations for four equal validators.
    Init {
        #[arg(long)]
        dir: PathBuf,
        /// Consensus ports start here; HTTP uses +10, ABCI uses +20.
        #[arg(long, default_value_t = 26000)]
        port_base: u16,
    },
    /// Run one validator, connecting to its configured ABCI application.
    Run {
        #[arg(long)]
        config: PathBuf,
    },
    /// Submit a UTF-8 transaction. Inclusion does not imply successful execution.
    Submit {
        #[arg(long)]
        url: String,
        #[arg(long)]
        tx: String,
    },
    Status {
        #[arg(long)]
        url: String,
    },
    Block {
        #[arg(long)]
        url: String,
        #[arg(long)]
        height: u64,
    },
}

#[derive(Clone)]
struct ApiState {
    chain_id: String,
    epoch: u64,
    journal: Arc<Mutex<Journal>>,
    transactions: Arc<TransactionClient>,
    consensus_store: Arc<dyn Store>,
    ingress: Arc<Mutex<AbciClient<Channel>>>,
    failure: Arc<RwLock<Option<String>>>,
}

#[derive(Serialize)]
struct Status {
    chain_id: String,
    epoch: u64,
    consensus_commit_index: u32,
    execution_commit_index: u32,
    execution_height: u64,
    app_hash: String,
    block_hash: String,
    pending_height: Option<u64>,
}

#[derive(Deserialize, Serialize)]
struct SubmitRequest {
    tx: String,
}

#[derive(Serialize)]
struct SubmitResponse {
    tx_hash: String,
    consensus_block: String,
    /// The transaction was proposed; final inclusion is observed in /blocks.
    status: &'static str,
}

#[derive(Serialize)]
struct BlockResponse {
    block: ChainBlock,
    execution: ExecutionResult,
}

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({"error": self.1}))).into_response()
    }
}

async fn healthy(state: &ApiState) -> std::result::Result<(), ApiError> {
    if let Some(error) = state.failure.read().await.as_ref() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("execution stopped: {error}"),
        ));
    }
    Ok(())
}

async fn status(State(state): State<ApiState>) -> std::result::Result<Json<Status>, ApiError> {
    healthy(&state).await?;
    let journal = state.journal.lock().await;
    let consensus_commit_index = state
        .consensus_store
        .read_last_commit()
        .map_err(|error| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("reading consensus store: {error}"),
            )
        })?
        .map_or(0, |commit| commit.reference().index);
    Ok(Json(Status {
        chain_id: state.chain_id.clone(),
        epoch: state.epoch,
        consensus_commit_index,
        execution_commit_index: journal.commit_index(),
        execution_height: journal.height(),
        app_hash: hex::encode(journal.app_hash()),
        block_hash: journal
            .latest_block()
            .map(|block| block.block_hash.clone())
            .unwrap_or_else(|| hex::encode(GENESIS_HASH)),
        pending_height: journal.pending_height(),
    }))
}

async fn block(
    State(state): State<ApiState>,
    Path(height): Path<u64>,
) -> std::result::Result<Json<BlockResponse>, ApiError> {
    healthy(&state).await?;
    let journal = state.journal.lock().await;
    if height == 0 || height > journal.height() {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            format!("executed block {height} is unavailable"),
        ));
    }
    let block = journal
        .block(height)
        .ok_or_else(|| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "journal block is missing".into(),
            )
        })?
        .clone();
    let execution = journal
        .execution_result(height)
        .ok_or_else(|| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "journal execution receipt is missing".into(),
            )
        })?
        .clone();
    Ok(Json(BlockResponse { block, execution }))
}

async fn submit(
    State(state): State<ApiState>,
    Json(request): Json<SubmitRequest>,
) -> std::result::Result<Json<SubmitResponse>, ApiError> {
    healthy(&state).await?;
    let transaction = request.tx.into_bytes();
    if transaction.is_empty() || transaction.len() > MAX_TRANSACTION_BYTES {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!("transaction must contain 1 to {MAX_TRANSACTION_BYTES} UTF-8 bytes"),
        ));
    }
    abci::check_tx(&mut *state.ingress.lock().await, transaction.clone())
        .await
        .map_err(|error| {
            let code = if error.to_string().starts_with("CheckTx rejected") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            };
            ApiError(code, error.to_string())
        })?;
    let (reference, _, _) = tokio::time::timeout(
        Duration::from_secs(10),
        state
            .transactions
            .submit(vec![transaction.clone()], Priority::Normal),
    )
    .await
    .map_err(|_| {
        ApiError(
            StatusCode::GATEWAY_TIMEOUT,
            "submission timed out; its final status is unknown".into(),
        )
    })?
    .map_err(|error| ApiError(StatusCode::SERVICE_UNAVAILABLE, error.to_string()))?;
    Ok(Json(SubmitResponse {
        tx_hash: hex::encode(Sha256::digest(&transaction).as_ref()),
        consensus_block: reference.to_string(),
        status: "proposed",
    }))
}

async fn run(config_path: PathBuf) -> Result<()> {
    let loaded = config::load(&config_path)?;
    let config = loaded.config;
    let client = abci::connect(config.abci_address).await?;
    let ingress = abci::connect(config.abci_address).await?;
    let mut executor = abci::AbciExecutor::new(client);
    let mut journal = Journal::open(config.data_dir.join("chain"), config.chain_id.clone())?;
    journal
        .recover(&mut executor)
        .await
        .context("recovering ABCI/application journal")?;
    // Bind before starting consensus, so an occupied HTTP port leaves no engine running.
    let listener = tokio::net::TcpListener::bind(config.api_address)
        .await
        .with_context(|| format!("binding API address {}", config.api_address))?;
    let runtime = start_consensus(
        loaded.committee,
        loaded.protocol_keypair,
        loaded.network_keypair,
        config.data_dir.join("consensus/0"),
        config.epoch_start_timestamp_ms,
        journal.commit_index(),
        prometheus::Registry::new(),
    )
    .await?;
    let authority = runtime.authority;
    let mut commits = runtime.commits;
    let monitor = runtime.monitor;
    let journal = Arc::new(Mutex::new(journal));
    let state = ApiState {
        chain_id: config.chain_id.clone(),
        epoch: config.epoch,
        journal: journal.clone(),
        transactions: authority.transaction_client(),
        consensus_store: authority.store(),
        ingress: Arc::new(Mutex::new(ingress)),
        failure: Arc::new(RwLock::new(None)),
    };
    let failure = state.failure.clone();
    let mut execution_task = tokio::spawn(async move {
        while let Some(subdag) = commits.recv().await {
            let result = journal
                .lock()
                .await
                .process_subdag(config.epoch, &subdag, &mut executor)
                .await;
            if let Err(error) = result {
                *failure.write().await = Some(format!("{error:#}"));
                return Err(error.context("execution loop stopped"));
            }
            monitor.set_highest_handled_commit(subdag.commit_ref.index);
        }
        Err(anyhow!("consensus commit stream closed"))
    });
    let router = Router::new()
        .route("/status", get(status))
        .route("/blocks/{height}", get(block))
        .route("/transactions", post(submit))
        .layer(DefaultBodyLimit::max(MAX_TRANSACTION_BYTES * 6 + 1024))
        .with_state(state.clone());
    let mut server_task = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .context("HTTP server stopped")
    });
    tracing::info!(chain_id = %config.chain_id, api_address = %config.api_address, "Dagora validator is running");
    let result = tokio::select! {
        result = &mut execution_task => result.context("execution task panicked")?,
        result = &mut server_task => result.context("HTTP task panicked")?,
        signal = tokio::signal::ctrl_c() => signal.context("waiting for shutdown signal"),
    };
    if let Err(error) = &result {
        *state.failure.write().await = Some(format!("{error:#}"));
    }
    execution_task.abort();
    server_task.abort();
    let _ = tokio::time::timeout(Duration::from_secs(5), authority.stop()).await;
    result
}

async fn request_json(url: &str, route: &str, transaction: Option<String>) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    let url = format!("{}{}", url.trim_end_matches('/'), route);
    let response = match transaction {
        Some(tx) => client.post(url).json(&SubmitRequest { tx }).send().await?,
        None => client.get(url).send().await?,
    };
    let status = response.status();
    let body = response.text().await?;
    anyhow::ensure!(status.is_success(), "HTTP {status}: {body}");
    let value: serde_json::Value = serde_json::from_str(&body)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dagora_node=info,consensus_core=warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().command {
        Command::Init { dir, port_base } => {
            config::init(&dir, port_base)?;
            println!("{}", dir.display());
            Ok(())
        }
        Command::Run { config } => run(config).await,
        Command::Submit { url, tx } => request_json(&url, "/transactions", Some(tx)).await,
        Command::Status { url } => request_json(&url, "/status", None).await,
        Command::Block { url, height } => {
            request_json(&url, &format!("/blocks/{height}"), None).await
        }
    }
}
