// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Unmodified CometBFT v0.38.22 ABCI protobuf and gRPC bindings.

pub const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("abci");

pub mod tendermint {
    pub mod abci {
        tonic::include_proto!("tendermint.abci");
    }
    pub mod crypto {
        tonic::include_proto!("tendermint.crypto");
    }
    pub mod types {
        tonic::include_proto!("tendermint.types");
    }
}
