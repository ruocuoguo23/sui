// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::{path::PathBuf, sync::Arc};

use anyhow::{Result, ensure};
use consensus_config::{
    ChainType, Committee, ConsensusProtocolConfig, NetworkKeyPair, Parameters, ProtocolKeyPair,
};
use consensus_core::{
    Clock, CommitConsumerArgs, CommitConsumerMonitor, CommittedSubDag, ConsensusAuthority,
    NetworkType, TransactionVerifier, ValidationError,
};
use consensus_types::block::{BlockRef, TransactionIndex};
use mysten_metrics::monitored_mpsc::UnboundedReceiver;
use prometheus::Registry;

pub const MAX_TRANSACTION_BYTES: usize = 256 * 1024;

pub struct ConsensusRuntime {
    pub authority: ConsensusAuthority,
    pub commits: UnboundedReceiver<CommittedSubDag>,
    pub monitor: Arc<CommitConsumerMonitor>,
}

struct BytesVerifier;

impl TransactionVerifier for BytesVerifier {
    fn verify_batch(&self, _block_ref: &BlockRef, batch: &[&[u8]]) -> Result<(), ValidationError> {
        if batch
            .iter()
            .any(|transaction| transaction.len() > MAX_TRANSACTION_BYTES)
        {
            return Err(ValidationError::InvalidTransaction(
                "transaction exceeds 256 KiB".to_owned(),
            ));
        }
        Ok(())
    }

    fn verify_and_vote_batch(
        &self,
        block_ref: &BlockRef,
        batch: &[&[u8]],
    ) -> Result<Vec<TransactionIndex>, ValidationError> {
        self.verify_batch(block_ref, batch)?;
        Ok(Vec::new())
    }
}

pub async fn start_consensus(
    committee: Committee,
    protocol_keypair: ProtocolKeyPair,
    network_keypair: NetworkKeyPair,
    db_path: PathBuf,
    epoch_start_timestamp_ms: u64,
    last_processed_commit_index: u32,
    registry: Registry,
) -> Result<ConsensusRuntime> {
    ensure!(committee.epoch() == 0, "V1 supports only epoch 0");
    ensure!(
        committee
            .authorities()
            .any(
                |(_, authority)| authority.protocol_key == protocol_keypair.public()
                    && authority.network_key == network_keypair.public()
            ),
        "validator keys do not match the committee"
    );
    let protocol_config = ConsensusProtocolConfig::new(
        0,
        ChainType::Unknown,
        MAX_TRANSACTION_BYTES as u64,
        512 * 1024,
        512,
        60,
        false,
        false,
        Some(1),
        30,
        false,
        600,
        60,
    );
    let parameters = Parameters {
        db_path,
        ..Default::default()
    };
    let (consumer, commits) =
        CommitConsumerArgs::new(last_processed_commit_index, last_processed_commit_index);
    let monitor = consumer.monitor();
    let authority = ConsensusAuthority::start(
        NetworkType::Tonic,
        epoch_start_timestamp_ms,
        committee,
        parameters,
        protocol_config,
        Some(protocol_keypair),
        network_keypair,
        Arc::new(Clock::default()),
        Arc::new(BytesVerifier),
        None,
        consumer,
        registry,
        0,
        None,
    )
    .await;
    Ok(ConsensusRuntime {
        authority,
        commits,
        monitor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn verifier_accepts_opaque_bytes_at_limit_and_rejects_oversized_bytes() {
        let reference = BlockRef::default();
        let at_limit = vec![255; MAX_TRANSACTION_BYTES];
        let oversized = vec![0; MAX_TRANSACTION_BYTES + 1];
        let verifier = BytesVerifier;
        verifier
            .verify_batch(&reference, &[b"", &at_limit])
            .unwrap();
        assert!(
            verifier
                .verify_and_vote_batch(&reference, &[&at_limit])
                .unwrap()
                .is_empty()
        );
        assert!(verifier.verify_batch(&reference, &[&oversized]).is_err());
        assert!(
            verifier
                .verify_and_vote_batch(&reference, &[&oversized])
                .is_err()
        );
    }
}
