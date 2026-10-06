// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use dagora_abci::MAX_MESSAGE_BYTES;
use dagora_abci::tendermint::abci::{
    RequestCheckTx, RequestCommit, RequestFinalizeBlock, RequestInfo, RequestInitChain,
    abci_client::AbciClient, abci_server::Abci,
};
use dagora_abci_mock::MockApplication;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::{Channel, Server};
use tonic::{Code, Request};

fn init(chain: &str) -> RequestInitChain {
    RequestInitChain {
        chain_id: chain.into(),
        initial_height: 1,
        ..Default::default()
    }
}

fn block(height: i64) -> RequestFinalizeBlock {
    RequestFinalizeBlock {
        height,
        hash: vec![height as u8; 32],
        txs: vec![b"ok".to_vec(), b"fail:test".to_vec()],
        ..Default::default()
    }
}

#[tokio::test]
async fn lifecycle_checks_and_execution_results() {
    let directory = TempDir::new().unwrap();
    let app = MockApplication::open(directory.path()).unwrap();
    assert!(MockApplication::open(directory.path()).is_err());
    assert_eq!(
        app.finalize_block(Request::new(block(1)))
            .await
            .unwrap_err()
            .code(),
        Code::FailedPrecondition
    );
    let genesis = app
        .init_chain(Request::new(init("test")))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(genesis.app_hash, vec![0; 32]);
    assert_eq!(
        app.init_chain(Request::new(init("test")))
            .await
            .unwrap()
            .into_inner(),
        genesis
    );
    assert!(app.init_chain(Request::new(init("other"))).await.is_err());
    let mut wrong_height = init("test");
    wrong_height.initial_height = 2;
    assert_eq!(
        app.init_chain(Request::new(wrong_height))
            .await
            .unwrap_err()
            .code(),
        Code::InvalidArgument
    );
    assert!(app.finalize_block(Request::new(block(2))).await.is_err());
    let mut malformed = block(1);
    malformed.hash.pop();
    assert_eq!(
        app.finalize_block(Request::new(malformed))
            .await
            .unwrap_err()
            .code(),
        Code::InvalidArgument
    );

    let output = app
        .finalize_block(Request::new(block(1)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(output.tx_results.len(), 2);
    assert_eq!(
        output
            .tx_results
            .iter()
            .map(|result| result.code)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    assert!(output.events.is_empty());
    assert!(output.validator_updates.is_empty());
    assert!(output.consensus_param_updates.is_none());
    assert_eq!(
        output.app_hash,
        Sha256::digest([vec![0; 32], vec![1; 32]].concat()).to_vec()
    );
    assert_eq!(
        app.finalize_block(Request::new(block(1)))
            .await
            .unwrap()
            .into_inner(),
        output
    );
    let mut conflict = block(1);
    conflict.txs.push(b"different".to_vec());
    assert!(app.finalize_block(Request::new(conflict)).await.is_err());
    assert_eq!(
        app.info(Request::new(RequestInfo::default()))
            .await
            .unwrap()
            .get_ref()
            .last_block_height,
        0
    );

    for (tx, code) in [(b"fail:test".as_slice(), 0), (b"ok".as_slice(), 0)] {
        assert_eq!(
            app.check_tx(Request::new(RequestCheckTx {
                tx: tx.to_vec(),
                ..Default::default()
            }))
            .await
            .unwrap()
            .get_ref()
            .code,
            code
        );
    }
    assert_eq!(
        app.check_tx(Request::new(RequestCheckTx {
            tx: vec![0; 256 * 1024],
            ..Default::default()
        }))
        .await
        .unwrap()
        .get_ref()
        .code,
        0
    );
    assert_eq!(
        app.check_tx(Request::new(RequestCheckTx {
            tx: vec![0; 256 * 1024 + 1],
            ..Default::default()
        }))
        .await
        .unwrap()
        .get_ref()
        .code,
        1
    );
    app.commit(Request::new(RequestCommit {})).await.unwrap();
    app.commit(Request::new(RequestCommit {})).await.unwrap();
    let info = app
        .info(Request::new(RequestInfo::default()))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(info.last_block_height, 1);
    assert_eq!(info.last_block_app_hash, output.app_hash);
    assert!(app.init_chain(Request::new(init("test"))).await.is_err());
    assert!(app.finalize_block(Request::new(block(1))).await.is_err());
}

#[tokio::test]
async fn restart_discards_pending_state_and_ignores_incomplete_temporary_snapshot() {
    let directory = TempDir::new().unwrap();
    let app = MockApplication::open(directory.path()).unwrap();
    app.init_chain(Request::new(init("test"))).await.unwrap();
    let expected = app
        .finalize_block(Request::new(block(1)))
        .await
        .unwrap()
        .into_inner();
    drop(app);
    std::fs::write(
        directory.path().join("state.json.tmp"),
        b"interrupted write",
    )
    .unwrap();
    let app = MockApplication::open(directory.path()).unwrap();
    assert!(app.init_chain(Request::new(init("other"))).await.is_err());
    assert_eq!(
        app.info(Request::new(RequestInfo::default()))
            .await
            .unwrap()
            .get_ref()
            .last_block_height,
        0
    );
    app.init_chain(Request::new(init("test"))).await.unwrap();
    assert_eq!(
        app.finalize_block(Request::new(block(1)))
            .await
            .unwrap()
            .into_inner(),
        expected
    );
    app.commit(Request::new(RequestCommit {})).await.unwrap();
    drop(app);
    let app = MockApplication::open(directory.path()).unwrap();
    assert_eq!(
        app.info(Request::new(RequestInfo::default()))
            .await
            .unwrap()
            .get_ref()
            .last_block_app_hash,
        expected.app_hash
    );
    drop(app);
    std::fs::write(
        directory.path().join("state.json"),
        b"corrupt committed snapshot",
    )
    .unwrap();
    assert!(MockApplication::open(directory.path()).is_err());
}

async fn server(
    app: MockApplication,
) -> (
    AbciClient<Channel>,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown, stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        Server::builder()
            .add_service(app.server())
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    (connect(address).await, shutdown, task)
}

async fn connect(address: SocketAddr) -> AbciClient<Channel> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match AbciClient::connect(format!("http://{address}")).await {
            Ok(client) => {
                return client
                    .max_decoding_message_size(MAX_MESSAGE_BYTES)
                    .max_encoding_message_size(MAX_MESSAGE_BYTES);
            }
            Err(error) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "ABCI server did not start: {error}"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
}

#[tokio::test]
async fn grpc_calls_persist_across_server_restart_and_accept_full_block_sized_payloads() {
    let directory = TempDir::new().unwrap();
    let (mut client, stop, task) = server(MockApplication::open(directory.path()).unwrap()).await;
    client.init_chain(init("test")).await.unwrap();
    let result = client.finalize_block(block(1)).await.unwrap().into_inner();
    // The node may restart after FinalizeBlock while the application process survives.
    assert_eq!(
        client
            .init_chain(init("test"))
            .await
            .unwrap()
            .get_ref()
            .app_hash,
        vec![0; 32]
    );
    assert_eq!(
        client.finalize_block(block(1)).await.unwrap().into_inner(),
        result
    );
    client.commit(RequestCommit {}).await.unwrap();
    let oversized_default = RequestCheckTx {
        tx: vec![b'x'; 4 * 1024 * 1024 + 1],
        ..Default::default()
    };
    assert_eq!(
        client
            .check_tx(oversized_default)
            .await
            .unwrap()
            .get_ref()
            .code,
        1
    );
    drop(client);
    stop.send(()).unwrap();
    task.await.unwrap();
    let (mut client, stop, task) = server(MockApplication::open(directory.path()).unwrap()).await;
    let info = client
        .info(RequestInfo::default())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(info.last_block_height, 1);
    assert_eq!(info.last_block_app_hash, result.app_hash);
    let mut empty_block = block(2);
    empty_block.txs.clear();
    let output = client
        .finalize_block(empty_block)
        .await
        .unwrap()
        .into_inner();
    assert!(output.tx_results.is_empty());
    assert_eq!(
        output.app_hash,
        Sha256::digest([result.app_hash, vec![2; 32]].concat()).to_vec()
    );
    client.commit(RequestCommit {}).await.unwrap();
    drop(client);
    stop.send(()).unwrap();
    task.await.unwrap();
}

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn process(address: SocketAddr, directory: &TempDir) -> Process {
    Process(
        Command::new(env!("CARGO_BIN_EXE_dagora-abci-mock"))
            .args(["--listen", &address.to_string(), "--data-dir"])
            .arg(directory.path())
            .arg("--fail-after-commit-once")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}

#[tokio::test]
async fn process_exit_after_durable_commit_loses_response_but_preserves_state() {
    let directory = TempDir::new().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut child = process(address, &directory);
    let mut client = connect(address).await;
    client.init_chain(init("test")).await.unwrap();
    let result = client.finalize_block(block(1)).await.unwrap().into_inner();
    assert!(client.commit(RequestCommit {}).await.is_err());
    assert_eq!(child.0.wait().unwrap().code(), Some(86));
    drop(client);
    drop(child);
    let _child = process(address, &directory);
    let mut client = connect(address).await;
    let info = client
        .info(RequestInfo::default())
        .await
        .unwrap()
        .into_inner();
    assert_eq!(info.last_block_height, 1);
    assert_eq!(info.last_block_app_hash, result.app_hash);
    client.commit(RequestCommit {}).await.unwrap();
    client.finalize_block(block(2)).await.unwrap();
    client.commit(RequestCommit {}).await.unwrap();
    assert_eq!(
        client
            .info(RequestInfo::default())
            .await
            .unwrap()
            .get_ref()
            .last_block_height,
        2
    );
}
