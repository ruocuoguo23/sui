// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use consensus_core::CommittedSubDag;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{AppInfo, BlockExecutor, ChainBlock, ConsensusPosition, ExecutionResult, GENESIS_HASH};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Cursor {
    chain_id: String,
    initialized: bool,
    height: u64,
    app_hash: Vec<u8>,
    block_hash: String,
    position: Option<ConsensusPosition>,
}

struct Record {
    block: ChainBlock,
    result: Option<ExecutionResult>,
}

pub struct Journal {
    directory: PathBuf,
    cursor: Cursor,
    records: BTreeMap<u64, Record>,
    // The OS releases this exclusive lock after a crash, without stale-lock cleanup.
    _lock: File,
}

impl Journal {
    pub fn open(data_dir: impl AsRef<Path>, chain_id: impl Into<String>) -> Result<Self> {
        let chain_id = chain_id.into();
        ensure!(!chain_id.is_empty(), "chain ID cannot be empty");
        let directory = data_dir.as_ref().join("journal");
        fs::create_dir_all(&directory).context("create journal directory")?;
        File::open(data_dir.as_ref())?.sync_all()?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("LOCK"))?;
        lock.try_lock()
            .context("data directory is already in use")?;

        let mut inputs = BTreeMap::new();
        let mut results = BTreeMap::new();
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(height) = name.strip_suffix(".input.json") {
                let height: u64 = height.parse().context("invalid journal input filename")?;
                ensure!(height > 0, "journal height must be positive");
                let block: ChainBlock = read_json(&entry.path())?;
                ensure!(
                    inputs.insert(height, block).is_none(),
                    "duplicate journal height"
                );
            } else if let Some(height) = name.strip_suffix(".result.json") {
                let height: u64 = height.parse().context("invalid journal result filename")?;
                let result: ExecutionResult = read_json(&entry.path())?;
                ensure!(
                    results.insert(height, result).is_none(),
                    "duplicate result height"
                );
            } else {
                ensure!(
                    name == "LOCK" || name == "cursor.json" || name.ends_with(".tmp"),
                    "unrecognized journal file: {name}"
                );
            }
        }

        let cursor_path = directory.join("cursor.json");
        let cursor: Cursor = if cursor_path.exists() {
            read_json(&cursor_path)?
        } else {
            ensure!(
                inputs.is_empty() && results.is_empty(),
                "journal records exist but the durable cursor is missing"
            );
            let cursor = Cursor {
                chain_id: chain_id.clone(),
                initialized: false,
                height: 0,
                app_hash: GENESIS_HASH.to_vec(),
                block_hash: hex::encode(GENESIS_HASH),
                position: None,
            };
            atomic_write(&directory, "cursor.json", &cursor)?;
            cursor
        };
        ensure!(cursor.chain_id == chain_id, "journal chain ID mismatch");
        ensure!(
            cursor.app_hash.len() == 32,
            "invalid cursor application hash"
        );
        crate::model::validate_hash(&cursor.block_hash)?;
        ensure!(
            cursor.height == 0 || cursor.initialized,
            "uninitialized non-genesis cursor"
        );

        let mut records = BTreeMap::new();
        let mut previous_hash = hex::encode(GENESIS_HASH);
        let mut previous_timestamp = 0;
        let mut previous_commit_index = 0u32;
        for (offset, (height, block)) in inputs.into_iter().enumerate() {
            ensure!(height == offset as u64 + 1, "gap in journal input heights");
            block.validate()?;
            ensure!(
                block.chain_id == chain_id,
                "journal input chain ID mismatch"
            );
            ensure!(block.height == height, "journal input height mismatch");
            ensure!(
                block.position.epoch == 0,
                "V1 journal consensus position mismatch"
            );
            ensure!(
                previous_commit_index.checked_add(1) == Some(block.position.index),
                "gap in journal consensus positions"
            );
            ensure!(
                block.previous_block_hash == previous_hash,
                "journal block chain mismatch"
            );
            ensure!(
                block.timestamp_ms >= previous_timestamp,
                "journal timestamp regressed"
            );
            previous_hash = block.block_hash.clone();
            previous_timestamp = block.timestamp_ms;
            previous_commit_index = block.position.index;
            let result = results.remove(&height);
            if let Some(result) = &result {
                result.validate(&block)?;
            }
            records.insert(height, Record { block, result });
        }
        ensure!(
            results.is_empty(),
            "execution result is missing its journal input"
        );
        ensure!(
            records.len() as u64 >= cursor.height,
            "cursor refers to a missing journal input"
        );
        ensure!(
            records.len() as u64 <= cursor.height + 1,
            "more than one unapplied journal input"
        );
        for (_, record) in records.range(..=cursor.height) {
            ensure!(
                record.result.is_some(),
                "committed block is missing execution result"
            );
        }
        if cursor.height == 0 {
            ensure!(
                cursor.position.is_none()
                    && cursor.app_hash == GENESIS_HASH
                    && cursor.block_hash == hex::encode(GENESIS_HASH),
                "invalid genesis cursor"
            );
        } else {
            let record = &records[&cursor.height];
            ensure!(
                cursor.position.as_ref() == Some(&record.block.position),
                "cursor position mismatch"
            );
            ensure!(
                cursor.block_hash == record.block.block_hash,
                "cursor block hash mismatch"
            );
            ensure!(
                cursor.app_hash == record.result.as_ref().unwrap().app_hash,
                "cursor application hash mismatch"
            );
        }
        Ok(Self {
            directory,
            cursor,
            records,
            _lock: lock,
        })
    }

    pub fn height(&self) -> u64 {
        self.cursor.height
    }

    pub fn commit_index(&self) -> u32 {
        self.cursor
            .position
            .as_ref()
            .map_or(0, |position| position.index)
    }

    pub fn app_hash(&self) -> &[u8] {
        &self.cursor.app_hash
    }

    pub fn latest_block(&self) -> Option<&ChainBlock> {
        self.block(self.cursor.height)
    }

    pub fn block(&self, height: u64) -> Option<&ChainBlock> {
        if height > self.cursor.height {
            return None;
        }
        self.records.get(&height).map(|record| &record.block)
    }

    pub fn result(&self, height: u64) -> Option<&ExecutionResult> {
        if height > self.cursor.height {
            return None;
        }
        self.records
            .get(&height)
            .and_then(|record| record.result.as_ref())
    }

    pub fn execution_result(&self, height: u64) -> Option<&ExecutionResult> {
        self.result(height)
    }

    pub fn pending_height(&self) -> Option<u64> {
        self.records
            .range((self.cursor.height + 1)..)
            .next()
            .map(|(height, _)| *height)
    }

    pub async fn recover(&mut self, executor: &mut dyn BlockExecutor) -> Result<()> {
        let mut info = executor.info().await.context("ABCI Info during recovery")?;
        ensure!(info.app_hash.len() == 32, "invalid application Info hash");
        if !self.cursor.initialized || (self.cursor.height == 0 && info.height == 0) {
            ensure!(
                info.height == 0 && info.app_hash == GENESIS_HASH,
                "application has no matching genesis journal"
            );
            // Same-chain InitChain must be replayable if the app initialized before a crash.
            let hash = executor.init_chain(&self.cursor.chain_id).await?;
            ensure!(hash == GENESIS_HASH, "unexpected genesis application hash");
            info = executor.info().await?;
            ensure!(
                info.height == 0 && info.app_hash == hash,
                "InitChain Info mismatch"
            );
            if !self.cursor.initialized {
                let mut cursor = self.cursor.clone();
                cursor.initialized = true;
                self.save_cursor(cursor)?;
            }
        }
        if info.height == self.cursor.height {
            ensure!(
                info.app_hash == self.cursor.app_hash,
                "application hash differs from durable cursor"
            );
            if let Some(height) = self.pending_height() {
                self.execute_pending(height, executor).await?;
            }
        } else if info.height == self.cursor.height + 1 {
            self.verify_and_advance(&info)?;
        } else {
            bail!(
                "application height {} disagrees with durable cursor {}",
                info.height,
                self.cursor.height
            );
        }
        Ok(())
    }

    pub async fn process_subdag(
        &mut self,
        epoch: u64,
        subdag: &CommittedSubDag,
        executor: &mut dyn BlockExecutor,
    ) -> Result<ChainBlock> {
        ensure!(
            self.cursor.initialized,
            "recover the journal before processing commits"
        );
        ensure!(epoch == 0, "V1 supports only epoch 0");
        let index = subdag.commit_ref.index;
        ensure!(index > 0, "commit index must be positive");
        let record = if index <= self.commit_index() {
            Some(
                self.records
                    .values()
                    .find(|record| {
                        record.block.position.epoch == epoch && record.block.position.index == index
                    })
                    .context("replayed consensus position is missing its journal input")?,
            )
        } else {
            ensure!(
                self.commit_index().checked_add(1) == Some(index),
                "consensus commit index is not consecutive"
            );
            self.pending_height().map(|height| &self.records[&height])
        };
        if let Some(record) = record {
            let height = record.block.height;
            let block = ChainBlock::from_subdag(
                &self.cursor.chain_id,
                epoch,
                subdag,
                height,
                &record.block.previous_block_hash,
            )?;
            ensure!(
                block == record.block,
                "replayed consensus commit differs from journal input"
            );
            if height > self.cursor.height {
                self.recover(executor).await?;
            }
            return Ok(block);
        }
        ensure!(
            self.pending_height().is_none(),
            "recover the pending block before accepting another commit"
        );
        let height = self
            .cursor
            .height
            .checked_add(1)
            .context("application block height exhausted")?;
        let block = ChainBlock::from_subdag(
            &self.cursor.chain_id,
            epoch,
            subdag,
            height,
            &self.cursor.block_hash,
        )?;
        if let Some(previous) = self.latest_block() {
            ensure!(
                block.timestamp_ms >= previous.timestamp_ms,
                "commit timestamp regressed"
            );
        }
        atomic_write(&self.directory, &format!("{height}.input.json"), &block)?;
        self.records.insert(
            height,
            Record {
                block: block.clone(),
                result: None,
            },
        );
        self.execute_pending(height, executor).await?;
        Ok(block)
    }

    async fn execute_pending(
        &mut self,
        height: u64,
        executor: &mut dyn BlockExecutor,
    ) -> Result<()> {
        let block = self.records[&height].block.clone();
        let result = executor
            .finalize_block(&block)
            .await
            .context("ABCI FinalizeBlock")?;
        result.validate(&block)?;
        if let Some(saved) = &self.records[&height].result {
            ensure!(
                saved == &result,
                "replayed FinalizeBlock result differs from durable journal"
            );
        } else {
            atomic_write(&self.directory, &format!("{height}.result.json"), &result)?;
            self.records.get_mut(&height).unwrap().result = Some(result);
        }

        // A failed Commit response can mean that the app committed but the acknowledgement was lost.
        let commit = executor.commit().await;
        let info = executor
            .info()
            .await
            .context("Commit outcome is uncertain: ABCI Info failed")?;
        if info.height == height {
            self.verify_and_advance(&info)?;
            return Ok(());
        }
        ensure!(
            info.height == self.cursor.height && info.app_hash == self.cursor.app_hash,
            "Commit returned inconsistent application state"
        );
        if let Err(error) = commit {
            return Err(error)
                .context("Commit did not advance the application; pending journal retained");
        }
        bail!("Commit acknowledged without advancing application height; pending journal retained")
    }

    fn verify_and_advance(&mut self, info: &AppInfo) -> Result<()> {
        ensure!(
            info.height == self.cursor.height + 1,
            "nonconsecutive application height"
        );
        let record = self
            .records
            .get(&info.height)
            .context("application advanced without a journal input")?;
        let result = record
            .result
            .as_ref()
            .context("application advanced without a journal execution result")?;
        ensure!(
            info.app_hash == result.app_hash,
            "application hash differs from journal result"
        );
        let cursor = Cursor {
            chain_id: self.cursor.chain_id.clone(),
            initialized: true,
            height: info.height,
            app_hash: result.app_hash.clone(),
            block_hash: record.block.block_hash.clone(),
            position: Some(record.block.position.clone()),
        };
        self.save_cursor(cursor)
    }

    fn save_cursor(&mut self, cursor: Cursor) -> Result<()> {
        atomic_write(&self.directory, "cursor.json", &cursor)?;
        self.cursor = cursor;
        Ok(())
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("decode {}", path.display()))
}

fn atomic_write(directory: &Path, name: &str, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    let destination = directory.join(name);
    if name != "cursor.json" && destination.exists() {
        ensure!(
            fs::read(&destination)? == bytes,
            "attempt to overwrite an immutable journal record"
        );
        File::open(directory)?.sync_all()?;
        return Ok(());
    }
    let temporary = directory.join(format!(".{name}.tmp"));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, destination)?;
    File::open(directory)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use consensus_core::{CommitDigest, CommitRef, TestBlock, Transaction, VerifiedBlock};
    use tempfile::TempDir;

    use super::*;
    use crate::TxResult;

    #[derive(Default)]
    struct FakeExecutor {
        height: u64,
        app_hash: [u8; 32],
        pending: Option<(u64, ExecutionResult)>,
        finalizations: usize,
        commits: usize,
        init_calls: usize,
        fail_finalize: bool,
        fail_commit_before: bool,
        fail_commit_after: bool,
        fail_info_after_commit: bool,
        info_unavailable: bool,
        divergent_result: bool,
        chain_id: Option<String>,
    }

    #[async_trait]
    impl BlockExecutor for FakeExecutor {
        async fn info(&mut self) -> Result<AppInfo> {
            if self.info_unavailable {
                self.info_unavailable = false;
                bail!("mock unavailable");
            }
            Ok(AppInfo {
                height: self.height,
                app_hash: self.app_hash.to_vec(),
            })
        }

        async fn init_chain(&mut self, chain_id: &str) -> Result<Vec<u8>> {
            if let Some(previous) = &self.chain_id {
                ensure!(previous == chain_id, "mock chain ID mismatch");
            }
            self.chain_id = Some(chain_id.to_owned());
            self.init_calls += 1;
            Ok(GENESIS_HASH.to_vec())
        }

        async fn finalize_block(&mut self, block: &ChainBlock) -> Result<ExecutionResult> {
            self.finalizations += 1;
            if self.fail_finalize {
                self.fail_finalize = false;
                bail!("crash during FinalizeBlock");
            }
            let result = ExecutionResult {
                app_hash: vec![block.height as u8; 32],
                tx_results: block
                    .transactions
                    .iter()
                    .map(|data| TxResult {
                        code: u32::from(self.divergent_result),
                        data: data.clone(),
                        gas_wanted: 0,
                        gas_used: 0,
                    })
                    .collect(),
            };
            self.pending = Some((block.height, result.clone()));
            Ok(result)
        }

        async fn commit(&mut self) -> Result<()> {
            self.commits += 1;
            if self.fail_commit_before {
                self.fail_commit_before = false;
                bail!("crash before Commit");
            }
            let (height, result) = self.pending.take().context("missing mock FinalizeBlock")?;
            self.height = height;
            self.app_hash.copy_from_slice(&result.app_hash);
            if self.fail_info_after_commit {
                self.fail_info_after_commit = false;
                self.info_unavailable = true;
            }
            if self.fail_commit_after {
                self.fail_commit_after = false;
                bail!("Commit acknowledgement lost");
            }
            Ok(())
        }
    }

    fn commit(index: u32, transactions: Vec<Vec<u8>>) -> CommittedSubDag {
        let block = VerifiedBlock::new_for_test(
            TestBlock::new(index, 0)
                .set_transactions(transactions.into_iter().map(Transaction::new).collect())
                .build(),
        );
        CommittedSubDag::new(
            block.reference(),
            vec![block],
            u64::from(index) * 100,
            CommitRef::new(index, CommitDigest::default()),
        )
    }

    #[tokio::test]
    async fn persists_blocks_and_results_and_skips_completed_replay() {
        let directory = TempDir::new().unwrap();
        let mut journal = Journal::open(directory.path(), "dagora-test").unwrap();
        let mut executor = FakeExecutor::default();
        journal.recover(&mut executor).await.unwrap();
        let first = commit(1, vec![b"a".to_vec()]);
        let block = journal
            .process_subdag(0, &first, &mut executor)
            .await
            .unwrap();
        assert_eq!(journal.height(), 1);
        assert_eq!(journal.app_hash(), &[1; 32]);
        assert!(journal.execution_result(1).is_some());
        assert!(Journal::open(directory.path(), "dagora-test").is_err());
        let mut replay = first.clone();
        replay.decided_with_local_blocks = false;
        replay.recovered_rejected_transactions = true;
        assert_eq!(
            journal
                .process_subdag(0, &replay, &mut executor)
                .await
                .unwrap(),
            block
        );
        assert_eq!(executor.finalizations, 1);
        assert_eq!(executor.commits, 1);
        drop(journal);
        let mut journal = Journal::open(directory.path(), "dagora-test").unwrap();
        journal.recover(&mut executor).await.unwrap();
        assert_eq!(journal.latest_block(), Some(&block));
        assert_eq!(executor.init_calls, 1);
    }

    #[tokio::test]
    async fn replays_input_after_finalize_crash() {
        let directory = TempDir::new().unwrap();
        let mut executor = FakeExecutor {
            fail_finalize: true,
            ..Default::default()
        };
        let mut journal = Journal::open(directory.path(), "test").unwrap();
        journal.recover(&mut executor).await.unwrap();
        assert!(
            journal
                .process_subdag(0, &commit(1, vec![]), &mut executor)
                .await
                .is_err()
        );
        assert_eq!(journal.pending_height(), Some(1));
        assert!(journal.result(1).is_none());
        drop(journal);
        let mut journal = Journal::open(directory.path(), "test").unwrap();
        journal.recover(&mut executor).await.unwrap();
        assert_eq!(journal.height(), 1);
        assert_eq!(executor.finalizations, 2);
    }

    #[tokio::test]
    async fn replays_durable_result_before_unapplied_commit() {
        let directory = TempDir::new().unwrap();
        let mut executor = FakeExecutor {
            fail_commit_before: true,
            ..Default::default()
        };
        let mut journal = Journal::open(directory.path(), "test").unwrap();
        journal.recover(&mut executor).await.unwrap();
        assert!(
            journal
                .process_subdag(0, &commit(1, vec![vec![7]]), &mut executor)
                .await
                .is_err()
        );
        let saved = journal.records[&1].result.as_ref().unwrap().clone();
        assert!(journal.result(1).is_none());
        assert!(journal.block(1).is_none());
        assert_eq!(executor.height, 0);
        executor.pending = None;
        drop(journal);
        let mut journal = Journal::open(directory.path(), "test").unwrap();
        journal.recover(&mut executor).await.unwrap();
        assert_eq!(journal.height(), 1);
        assert_eq!(journal.result(1), Some(&saved));
        assert_eq!(executor.finalizations, 2);
    }

    #[tokio::test]
    async fn rejects_different_finalize_result_after_restart() {
        let directory = TempDir::new().unwrap();
        let mut executor = FakeExecutor {
            fail_commit_before: true,
            ..Default::default()
        };
        let mut journal = Journal::open(directory.path(), "test").unwrap();
        journal.recover(&mut executor).await.unwrap();
        assert!(
            journal
                .process_subdag(0, &commit(1, vec![vec![7]]), &mut executor)
                .await
                .is_err()
        );
        let saved = journal.records[&1].result.as_ref().unwrap().clone();
        drop(journal);
        executor.divergent_result = true;
        let mut journal = Journal::open(directory.path(), "test").unwrap();
        assert!(journal.recover(&mut executor).await.is_err());
        assert_eq!(journal.height(), 0);
        assert_eq!(journal.records[&1].result.as_ref(), Some(&saved));
        assert_eq!(executor.commits, 1);
    }

    #[tokio::test]
    async fn info_resolves_lost_commit_acknowledgement() {
        let directory = TempDir::new().unwrap();
        let mut executor = FakeExecutor {
            fail_commit_after: true,
            ..Default::default()
        };
        let mut journal = Journal::open(directory.path(), "test").unwrap();
        journal.recover(&mut executor).await.unwrap();
        journal
            .process_subdag(0, &commit(1, vec![]), &mut executor)
            .await
            .unwrap();
        assert_eq!(journal.height(), 1);
        assert_eq!(executor.commits, 1);
    }

    #[tokio::test]
    async fn committed_app_resumes_cursor_without_repeat_after_uncertain_info() {
        let directory = TempDir::new().unwrap();
        let mut executor = FakeExecutor {
            fail_info_after_commit: true,
            ..Default::default()
        };
        let mut journal = Journal::open(directory.path(), "test").unwrap();
        journal.recover(&mut executor).await.unwrap();
        assert!(
            journal
                .process_subdag(0, &commit(1, vec![]), &mut executor)
                .await
                .is_err()
        );
        assert_eq!(executor.height, 1);
        assert_eq!(journal.height(), 0);
        drop(journal);
        let mut journal = Journal::open(directory.path(), "test").unwrap();
        journal.recover(&mut executor).await.unwrap();
        assert_eq!(journal.height(), 1);
        assert_eq!(executor.finalizations, 1);
        assert_eq!(executor.commits, 1);
    }

    #[tokio::test]
    async fn detects_missing_journal_and_application_hash_mismatch() {
        let directory = TempDir::new().unwrap();
        let mut executor = FakeExecutor::default();
        let mut journal = Journal::open(directory.path(), "test").unwrap();
        journal.recover(&mut executor).await.unwrap();
        journal
            .process_subdag(0, &commit(1, vec![]), &mut executor)
            .await
            .unwrap();
        executor.app_hash = [9; 32];
        assert!(journal.recover(&mut executor).await.is_err());
        drop(journal);
        assert!(Journal::open(directory.path(), "another-chain").is_err());
        fs::remove_file(directory.path().join("journal/1.input.json")).unwrap();
        assert!(Journal::open(directory.path(), "test").is_err());
    }

    #[tokio::test]
    async fn detects_corrupted_input_and_nonconsecutive_commits() {
        let directory = TempDir::new().unwrap();
        let mut executor = FakeExecutor::default();
        let mut journal = Journal::open(directory.path(), "test").unwrap();
        journal.recover(&mut executor).await.unwrap();
        assert!(
            journal
                .process_subdag(0, &commit(2, vec![]), &mut executor)
                .await
                .is_err()
        );
        journal
            .process_subdag(0, &commit(1, vec![]), &mut executor)
            .await
            .unwrap();
        let mut changed = commit(1, vec![vec![5]]);
        assert!(
            journal
                .process_subdag(0, &changed, &mut executor)
                .await
                .is_err()
        );
        changed.commit_ref.index = 3;
        assert!(
            journal
                .process_subdag(1, &changed, &mut executor)
                .await
                .is_err()
        );
        let mut corrupted = journal.block(1).unwrap().clone();
        corrupted.transactions.push(vec![3]);
        drop(journal);
        fs::write(
            directory.path().join("journal/1.input.json"),
            serde_json::to_vec(&corrupted).unwrap(),
        )
        .unwrap();
        assert!(Journal::open(directory.path(), "test").is_err());
    }

    #[tokio::test]
    async fn initialized_genesis_journal_checks_application_chain_binding() {
        let directory = TempDir::new().unwrap();
        let mut executor = FakeExecutor::default();
        let mut journal = Journal::open(directory.path(), "test").unwrap();
        journal.recover(&mut executor).await.unwrap();
        drop(journal);
        let mut journal = Journal::open(directory.path(), "test").unwrap();
        let mut other_application = FakeExecutor {
            chain_id: Some("another-chain".to_owned()),
            ..Default::default()
        };
        assert!(journal.recover(&mut other_application).await.is_err());
        journal.recover(&mut executor).await.unwrap();
        assert_eq!(executor.init_calls, 2);
    }

    #[tokio::test]
    async fn application_ahead_without_matching_result_fails_closed() {
        let directory = TempDir::new().unwrap();
        let mut executor = FakeExecutor::default();
        let mut journal = Journal::open(directory.path(), "test").unwrap();
        journal.recover(&mut executor).await.unwrap();
        executor.height = 1;
        executor.app_hash = [1; 32];
        assert!(journal.recover(&mut executor).await.is_err());
        assert_eq!(journal.height(), 0);
        assert_eq!(executor.finalizations, 0);
    }

    #[tokio::test]
    async fn application_ahead_with_wrong_result_hash_fails_closed() {
        let directory = TempDir::new().unwrap();
        let mut executor = FakeExecutor {
            fail_info_after_commit: true,
            ..Default::default()
        };
        let mut journal = Journal::open(directory.path(), "test").unwrap();
        journal.recover(&mut executor).await.unwrap();
        assert!(
            journal
                .process_subdag(0, &commit(1, vec![]), &mut executor)
                .await
                .is_err()
        );
        executor.app_hash = [9; 32];
        assert!(journal.recover(&mut executor).await.is_err());
        assert_eq!(journal.height(), 0);
        assert_eq!(executor.finalizations, 1);
        assert_eq!(executor.commits, 1);
    }
}
