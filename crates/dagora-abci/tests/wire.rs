// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::path::Path;

use dagora_abci::FILE_DESCRIPTOR_SET;
use dagora_abci::tendermint::abci::{
    RequestFinalizeBlock, RequestInitChain, ResponseCommit, ResponseFinalizeBlock, ResponseInfo,
};
use prost::Message;
use prost_types::{DescriptorProto, FileDescriptorSet};
use sha2::{Digest, Sha256};

fn fixture<T: Message + Default>(input: &str) -> T {
    T::decode(hex::decode(input.trim()).unwrap().as_slice()).unwrap()
}

#[tokio::test]
async fn independently_encoded_protoc_fixtures_match_wire_types() {
    let info: ResponseInfo = fixture(include_str!("fixtures/info-response.hex"));
    assert_eq!(info.last_block_height, 7);
    assert_eq!(
        info.last_block_app_hash,
        b"0123456789abcdef0123456789abcdef"
    );
    assert_eq!(info.app_version, 1);

    let request: RequestFinalizeBlock = fixture(include_str!("fixtures/finalize-request.hex"));
    assert_eq!(request.txs, [b"ok".to_vec(), b"fail:test".to_vec()]);
    assert_eq!(request.height, 7);
    assert_eq!(request.hash, info.last_block_app_hash);
    assert_eq!(request.time.unwrap().seconds, 1_700_000_000);
    let commit = request.decided_last_commit.unwrap();
    assert_eq!(commit.round, 2);
    assert_eq!(commit.votes[0].block_id_flag, 2);
    assert_eq!(commit.votes[0].validator.as_ref().unwrap().power, 1);

    let response: ResponseFinalizeBlock = fixture(include_str!("fixtures/finalize-response.hex"));
    assert_eq!(response.app_hash, info.last_block_app_hash);
    assert_eq!(response.tx_results.len(), 2);
    assert_eq!(response.tx_results[0].code, 0);
    assert_eq!(response.tx_results[0].gas_wanted, 3);
    assert_eq!(response.tx_results[1].code, 1);

    let committed: ResponseCommit = fixture(include_str!("fixtures/commit-response.hex"));
    assert_eq!(committed.retain_height, 7);
    let initialized: RequestInitChain = fixture(include_str!("fixtures/init-request.hex"));
    assert_eq!(initialized.chain_id, "fixtures");
    assert_eq!(initialized.initial_height, 1);
}

fn field(message: &DescriptorProto, name: &str, number: i32) {
    assert!(
        message
            .field
            .iter()
            .any(|field| field.name.as_deref() == Some(name) && field.number == Some(number))
    );
}

#[tokio::test]
async fn descriptor_preserves_official_service_and_version_specific_fields() {
    let descriptor = FileDescriptorSet::decode(FILE_DESCRIPTOR_SET).unwrap();
    let abci = descriptor
        .file
        .iter()
        .find(|file| file.name.as_deref() == Some("tendermint/abci/types.proto"))
        .unwrap();
    assert_eq!(abci.package.as_deref(), Some("tendermint.abci"));
    let service = &abci.service[0];
    assert_eq!(service.name.as_deref(), Some("ABCI"));
    assert_eq!(service.method.len(), 16);
    for name in ["Info", "InitChain", "CheckTx", "FinalizeBlock", "Commit"] {
        let method = service
            .method
            .iter()
            .find(|method| method.name.as_deref() == Some(name))
            .unwrap();
        assert_eq!(
            method.input_type.as_deref(),
            Some(format!(".tendermint.abci.Request{name}").as_str())
        );
        assert_eq!(
            method.output_type.as_deref(),
            Some(format!(".tendermint.abci.Response{name}").as_str())
        );
    }
    let find = |name: &str| {
        abci.message_type
            .iter()
            .find(|message| message.name.as_deref() == Some(name))
            .unwrap()
    };
    field(find("ResponseInfo"), "last_block_height", 4);
    field(find("ResponseInfo"), "last_block_app_hash", 5);
    field(find("RequestFinalizeBlock"), "hash", 4);
    field(find("RequestFinalizeBlock"), "height", 5);
    field(find("ResponseFinalizeBlock"), "app_hash", 5);
    let commit = find("ResponseCommit");
    field(commit, "retain_height", 3);
    assert_eq!(commit.field.len(), 1);
}

#[tokio::test]
async fn vendored_sources_and_licenses_match_pinned_checksums() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for line in include_str!("../SHA256SUMS").lines() {
        let (expected, path) = line.split_once("  ").unwrap();
        let actual = hex::encode(Sha256::digest(std::fs::read(root.join(path)).unwrap()));
        assert_eq!(actual, expected, "changed upstream source: {path}");
    }
}
