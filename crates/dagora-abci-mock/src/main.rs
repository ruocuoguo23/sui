// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use dagora_abci_mock::MockApplication;

#[derive(Parser)]
#[command(about = "Persistent mock application served over CometBFT v0.38.22 gRPC ABCI")]
struct Arguments {
    #[arg(long)]
    listen: SocketAddr,
    #[arg(long)]
    data_dir: PathBuf,
    /// Exit once after persisting Commit, before replying, to exercise recovery.
    #[arg(long)]
    fail_after_commit_once: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    let mut application = MockApplication::open(&arguments.data_dir)?;
    if arguments.fail_after_commit_once {
        application = application.with_fail_after_commit_once();
    }
    tonic::transport::Server::builder()
        .add_service(application.server())
        .serve_with_shutdown(arguments.listen, async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
