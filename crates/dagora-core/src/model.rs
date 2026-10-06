// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, ensure};
use async_trait::async_trait;
use consensus_core::{BlockAPI, CommittedSubDag};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const GENESIS_HASH: [u8; 32] = [0; 32];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsensusPosition {
    pub epoch: u64,
    pub index: u32,
    pub digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainBlock {
    pub chain_id: String,
    pub position: ConsensusPosition,
    pub height: u64,
    pub timestamp_ms: u64,
    pub previous_block_hash: String,
    pub transactions: Vec<Vec<u8>>,
    pub block_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxResult {
    pub code: u32,
    pub data: Vec<u8>,
    pub gas_wanted: i64,
    pub gas_used: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub app_hash: Vec<u8>,
    pub tx_results: Vec<TxResult>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub height: u64,
    pub app_hash: Vec<u8>,
}

#[async_trait]
pub trait BlockExecutor: Send {
    async fn info(&mut self) -> Result<AppInfo>;
    async fn init_chain(&mut self, chain_id: &str) -> Result<Vec<u8>>;
    async fn finalize_block(&mut self, block: &ChainBlock) -> Result<ExecutionResult>;
    async fn commit(&mut self) -> Result<()>;
}

impl ChainBlock {
    pub fn from_subdag(
        chain_id: &str,
        epoch: u64,
        subdag: &CommittedSubDag,
        height: u64,
        previous_block_hash: &str,
    ) -> Result<Self> {
        ensure!(!chain_id.is_empty(), "chain ID cannot be empty");
        ensure!(subdag.commit_ref.index > 0, "commit index must be positive");
        let mut blocks = BTreeMap::new();
        for block in &subdag.blocks {
            ensure!(block.epoch() == epoch, "DAG block epoch mismatch");
            ensure!(
                blocks.insert(block.reference(), block).is_none(),
                "duplicate DAG block in commit"
            );
        }
        ensure!(
            blocks.contains_key(&subdag.leader),
            "commit leader is missing"
        );
        for (reference, rejected) in &subdag.rejected_transactions_by_block {
            let block = blocks.get(reference).ok_or_else(|| {
                anyhow::anyhow!("rejected transactions refer to an unknown DAG block")
            })?;
            let indices: BTreeSet<_> = rejected.iter().copied().collect();
            ensure!(indices.len() == rejected.len(), "duplicate rejected index");
            ensure!(
                rejected
                    .iter()
                    .all(|index| usize::from(*index) < block.transactions().len()),
                "rejected transaction index is out of bounds"
            );
        }
        // BlockRef's total order includes the digest, including equivocations in one slot.
        let transactions = blocks
            .into_iter()
            .flat_map(|(reference, block)| {
                let rejected: BTreeSet<_> = subdag
                    .rejected_transactions_by_block
                    .get(&reference)
                    .into_iter()
                    .flatten()
                    .map(|index| usize::from(*index))
                    .collect();
                block
                    .transactions()
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| !rejected.contains(index))
                    .map(|(_, transaction)| transaction.data().to_vec())
                    .collect::<Vec<_>>()
            })
            .collect();
        let mut block = Self {
            chain_id: chain_id.to_owned(),
            position: ConsensusPosition {
                epoch,
                index: subdag.commit_ref.index,
                digest: hex::encode(subdag.commit_ref.digest.into_inner()),
            },
            height,
            timestamp_ms: subdag.timestamp_ms,
            previous_block_hash: previous_block_hash.to_owned(),
            transactions,
            block_hash: String::new(),
        };
        block.block_hash = block.calculate_hash()?;
        block.validate()?;
        Ok(block)
    }

    pub fn calculate_hash(&self) -> Result<String> {
        let bytes = bcs::to_bytes(&(
            &self.chain_id,
            &self.position,
            self.height,
            self.timestamp_ms,
            &self.previous_block_hash,
            &self.transactions,
        ))?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(!self.chain_id.is_empty(), "chain ID cannot be empty");
        ensure!(self.height > 0, "block height must be positive");
        ensure!(self.position.index > 0, "commit index must be positive");
        validate_hash(&self.position.digest)?;
        validate_hash(&self.previous_block_hash)?;
        validate_hash(&self.block_hash)?;
        ensure!(
            self.calculate_hash()? == self.block_hash,
            "block hash mismatch"
        );
        Ok(())
    }
}

impl ExecutionResult {
    pub fn validate(&self, block: &ChainBlock) -> Result<()> {
        ensure!(
            self.app_hash.len() == 32,
            "application hash must be 32 bytes"
        );
        ensure!(
            self.tx_results.len() == block.transactions.len(),
            "execution transaction result count mismatch"
        );
        Ok(())
    }
}

pub(crate) fn validate_hash(hash: &str) -> Result<()> {
    ensure!(
        hash.len() == 64
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "hash must be 64 lowercase hexadecimal characters"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use consensus_core::{CommitDigest, CommitRef, TestBlock, Transaction, VerifiedBlock};

    use super::*;

    fn fixture() -> CommittedSubDag {
        let blocks = vec![
            VerifiedBlock::new_for_test(
                TestBlock::new(2, 1)
                    .set_transactions(vec![Transaction::new(vec![3])])
                    .build(),
            ),
            VerifiedBlock::new_for_test(
                TestBlock::new(1, 0)
                    .set_transactions(vec![Transaction::new(vec![1]), Transaction::new(vec![2])])
                    .build(),
            ),
        ];
        CommittedSubDag::new(
            blocks[0].reference(),
            blocks,
            100,
            CommitRef::new(1, CommitDigest::default()),
        )
    }

    fn convert(subdag: &CommittedSubDag) -> Result<ChainBlock> {
        ChainBlock::from_subdag("test", 0, subdag, 1, &hex::encode(GENESIS_HASH))
    }

    #[tokio::test]
    async fn canonical_order_survives_reordered_input_and_local_flags() {
        let mut subdag = fixture();
        let first = convert(&subdag).unwrap();
        assert_eq!(first.transactions, vec![vec![1], vec![2], vec![3]]);
        subdag.blocks.reverse();
        subdag.decided_with_local_blocks = false;
        subdag.recovered_rejected_transactions = true;
        assert_eq!(convert(&subdag).unwrap(), first);
        assert_eq!(first.block_hash.len(), 64);
        first.validate().unwrap();
    }

    #[tokio::test]
    async fn rejected_indices_are_zero_based_and_all_rejected_still_form_a_block() {
        let mut subdag = fixture();
        subdag
            .rejected_transactions_by_block
            .insert(subdag.blocks[1].reference(), vec![0]);
        assert_eq!(
            convert(&subdag).unwrap().transactions,
            vec![vec![2], vec![3]]
        );
        subdag
            .rejected_transactions_by_block
            .insert(subdag.blocks[1].reference(), vec![0, 1]);
        subdag
            .rejected_transactions_by_block
            .insert(subdag.blocks[0].reference(), vec![0]);
        let block = convert(&subdag).unwrap();
        assert!(block.transactions.is_empty());
        assert_eq!(block.height, 1);
        assert_eq!(block.position.index, 1);
    }

    #[tokio::test]
    async fn empty_transaction_commit_has_a_canonical_block() {
        let block = VerifiedBlock::new_for_test(TestBlock::new(1, 0).build());
        let subdag = CommittedSubDag::new(
            block.reference(),
            vec![block],
            100,
            CommitRef::new(1, CommitDigest::default()),
        );
        let block = convert(&subdag).unwrap();
        assert!(block.transactions.is_empty());
        assert_ne!(block.block_hash, hex::encode(GENESIS_HASH));
    }

    #[tokio::test]
    async fn invalid_commit_references_and_reject_indices_fail() {
        let mut subdag = fixture();
        subdag
            .rejected_transactions_by_block
            .insert(subdag.blocks[0].reference(), vec![1]);
        assert!(convert(&subdag).is_err());
        subdag.rejected_transactions_by_block.clear();
        subdag.blocks.push(subdag.blocks[0].clone());
        assert!(convert(&subdag).is_err());
        subdag.blocks.pop();
        subdag.blocks.remove(0);
        assert!(convert(&subdag).is_err());
        assert!(
            ChainBlock::from_subdag("test", 1, &fixture(), 1, &hex::encode(GENESIS_HASH)).is_err()
        );
    }

    #[tokio::test]
    async fn block_hash_and_execution_result_validation_detect_changes() {
        let mut block = convert(&fixture()).unwrap();
        let result = ExecutionResult {
            app_hash: vec![0; 32],
            tx_results: vec![],
        };
        assert!(result.validate(&block).is_err());
        let result = ExecutionResult {
            app_hash: vec![0; 31],
            tx_results: block
                .transactions
                .iter()
                .map(|_| TxResult {
                    code: 0,
                    data: vec![],
                    gas_wanted: 0,
                    gas_used: 0,
                })
                .collect(),
        };
        assert!(result.validate(&block).is_err());
        block.timestamp_ms += 1;
        assert!(block.validate().is_err());
    }

    #[tokio::test]
    async fn application_height_is_independent_of_epoch_local_commit_position() {
        let dag_block = VerifiedBlock::new_for_test(TestBlock::new(1, 0).set_epoch(7).build());
        let subdag = CommittedSubDag::new(
            dag_block.reference(),
            vec![dag_block],
            100,
            CommitRef::new(1, CommitDigest::default()),
        );
        let block =
            ChainBlock::from_subdag("test", 7, &subdag, 101, &hex::encode(GENESIS_HASH)).unwrap();
        assert_eq!(block.height, 101);
        assert_eq!(block.position.epoch, 7);
        assert_eq!(block.position.index, 1);
        block.validate().unwrap();
    }
}
