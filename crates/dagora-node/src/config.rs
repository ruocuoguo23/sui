// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::{
    fs,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use consensus_config::{Authority, AuthorityName, Committee, NetworkKeyPair, ProtocolKeyPair};
use fastcrypto::{
    ed25519::{Ed25519KeyPair, Ed25519PrivateKey},
    hash::{HashFunction, Sha256},
    traits::{KeyPair, ToFromBytes},
};
use rand::{SeedableRng, rngs::StdRng};
use serde::{Deserialize, Serialize};

pub const CHAIN_ID: &str = "dagora-local";
pub const VALIDATORS: usize = 4;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NodeConfig {
    pub chain_id: String,
    pub epoch: u64,
    pub epoch_start_timestamp_ms: u64,
    pub validator_index: usize,
    pub committee_path: PathBuf,
    pub protocol_key_path: PathBuf,
    pub network_key_path: PathBuf,
    pub data_dir: PathBuf,
    pub consensus_port: u16,
    pub api_address: SocketAddr,
    pub abci_address: SocketAddr,
}

pub struct LoadedConfig {
    pub config: NodeConfig,
    pub committee: Committee,
    pub protocol_keypair: ProtocolKeyPair,
    pub network_keypair: NetworkKeyPair,
}

pub fn init(directory: &Path, port_base: u16) -> Result<()> {
    ensure!(
        port_base > 0 && port_base <= u16::MAX - 23,
        "port base must be between 1 and 65512"
    );
    if directory.exists() {
        ensure!(
            directory.read_dir()?.next().is_none(),
            "initialization directory must be empty; existing keys and chain data will not be overwritten"
        );
    } else {
        fs::create_dir_all(directory)?;
    }
    let directory = fs::canonicalize(directory)?;
    let mut authorities = Vec::with_capacity(VALIDATORS);
    let mut keypairs = Vec::with_capacity(VALIDATORS);
    let mut rng = StdRng::from_entropy();
    for index in 0..VALIDATORS {
        let protocol = Ed25519KeyPair::generate(&mut rng);
        let network = Ed25519KeyPair::generate(&mut rng);
        let protocol_key = ProtocolKeyPair::new(protocol.copy());
        let network_key = NetworkKeyPair::new(network.copy());
        let identity = Sha256::digest(protocol_key.public().to_bytes())
            .as_ref()
            .repeat(3);
        authorities.push(Authority {
            stake: 1,
            address: format!("/ip4/127.0.0.1/udp/{}", port_base + index as u16).parse()?,
            hostname: format!("dagora-{index}"),
            authority_name: AuthorityName::new(identity),
            protocol_key: protocol_key.public(),
            network_key: network_key.public(),
        });
        keypairs.push((protocol, network));
    }
    write_new(
        &directory.join("committee.json"),
        &serde_json::to_vec_pretty(&Committee::new(0, authorities))?,
        false,
    )?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis()
        .try_into()?;
    for (index, (protocol, network)) in keypairs.into_iter().enumerate() {
        let node_directory = directory.join(format!("node{index}"));
        fs::create_dir(&node_directory)?;
        let protocol_path = node_directory.join("protocol.key");
        let network_path = node_directory.join("network.key");
        write_new(
            &protocol_path,
            hex::encode(protocol.private().as_bytes()).as_bytes(),
            true,
        )?;
        write_new(
            &network_path,
            hex::encode(network.private().as_bytes()).as_bytes(),
            true,
        )?;
        let config = NodeConfig {
            chain_id: CHAIN_ID.into(),
            epoch: 0,
            epoch_start_timestamp_ms: timestamp,
            validator_index: index,
            committee_path: directory.join("committee.json"),
            protocol_key_path: protocol_path,
            network_key_path: network_path,
            data_dir: node_directory.join("data"),
            consensus_port: port_base + index as u16,
            api_address: ([127, 0, 0, 1], port_base + 10 + index as u16).into(),
            abci_address: ([127, 0, 0, 1], port_base + 20 + index as u16).into(),
        };
        write_new(
            &node_directory.join("config.json"),
            &serde_json::to_vec_pretty(&config)?,
            false,
        )?;
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8], secret: bool) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if secret { 0o600 } else { 0o644 });
    }
    #[cfg(not(unix))]
    let _ = secret;
    let mut file = options
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub fn load(path: &Path) -> Result<LoadedConfig> {
    let config: NodeConfig = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("reading config {}", path.display()))?,
    )?;
    ensure!(!config.chain_id.is_empty(), "chain_id cannot be empty");
    ensure!(config.epoch == 0, "v1 supports only fixed epoch 0");
    ensure!(
        config.validator_index < VALIDATORS,
        "validator index is outside the committee"
    );
    let committee: Committee = serde_json::from_slice(&fs::read(&config.committee_path)?)?;
    ensure!(
        committee.epoch() == 0 && committee.size() == VALIDATORS,
        "v1 requires a four-validator epoch-0 committee"
    );
    ensure!(
        committee
            .authorities()
            .all(|(_, authority)| authority.stake == 1),
        "v1 requires four equal voting weights"
    );
    let protocol = read_key(&config.protocol_key_path)?;
    let network = read_key(&config.network_key_path)?;
    let protocol_keypair = ProtocolKeyPair::new(protocol);
    let network_keypair = NetworkKeyPair::new(network);
    let authority = &committee.authorities_slice()[config.validator_index];
    ensure!(
        authority
            .address
            .udp_multiaddr_to_listen_address()
            .context("committee address must use IP/DNS + UDP encoding")?
            .port()
            == config.consensus_port,
        "consensus port does not match committee address"
    );
    ensure!(
        authority.protocol_key == protocol_keypair.public(),
        "protocol private key does not match configured validator"
    );
    ensure!(
        authority.network_key == network_keypair.public(),
        "network private key does not match configured validator"
    );
    let mut protocol_keys = std::collections::HashSet::new();
    let mut network_keys = std::collections::HashSet::new();
    let mut addresses = std::collections::HashSet::new();
    for (_, member) in committee.authorities() {
        ensure!(
            protocol_keys.insert(member.protocol_key.to_bytes().to_vec()),
            "duplicate committee protocol key"
        );
        ensure!(
            network_keys.insert(member.network_key.to_bytes()),
            "duplicate committee network key"
        );
        ensure!(
            addresses.insert(
                member
                    .address
                    .udp_multiaddr_to_listen_address()
                    .context("committee address must use IP/DNS + UDP encoding")?
            ),
            "duplicate committee network address"
        );
    }
    // Recompute thresholds rather than trust redundant fields in edited JSON.
    let committee = Committee::new(0, committee.authorities_slice().to_vec());
    Ok(LoadedConfig {
        config,
        committee,
        protocol_keypair,
        network_keypair,
    })
}

fn read_key(path: &Path) -> Result<Ed25519KeyPair> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            fs::metadata(path)?.permissions().mode() & 0o077 == 0,
            "private key {} must be accessible only to its owner",
            path.display()
        );
    }
    let bytes =
        hex::decode(fs::read_to_string(path)?.trim()).context("invalid private key encoding")?;
    let private = Ed25519PrivateKey::from_bytes(&bytes).context("invalid Ed25519 private key")?;
    Ok(Ed25519KeyPair::from(private))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn init_generates_distinct_matching_keys_and_refuses_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("cluster");
        init(&dir, 21000).unwrap();
        let mut public_keys = std::collections::HashSet::new();
        for index in 0..VALIDATORS {
            let loaded = load(&dir.join(format!("node{index}/config.json"))).unwrap();
            assert!(public_keys.insert(loaded.protocol_keypair.public().to_bytes().to_vec()));
            assert!(public_keys.insert(loaded.network_keypair.public().to_bytes().to_vec()));
            assert_eq!(loaded.committee.quorum_threshold(), 3);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(loaded.config.protocol_key_path)
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
        }
        assert!(init(&dir, 21000).is_err());
        let config_path = dir.join("node0/config.json");
        let mut config: NodeConfig =
            serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config.consensus_port += 1;
        fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        assert!(
            load(&config_path)
                .err()
                .unwrap()
                .to_string()
                .contains("consensus port does not match")
        );
    }

    #[tokio::test]
    async fn mismatched_keys_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        init(temp.path(), 21000).unwrap();
        fs::copy(
            temp.path().join("node1/protocol.key"),
            temp.path().join("node0/protocol.key"),
        )
        .unwrap();
        assert!(
            load(&temp.path().join("node0/config.json"))
                .err()
                .unwrap()
                .to_string()
                .contains("does not match")
        );
    }

    #[tokio::test]
    async fn duplicate_committee_identity_and_address_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        init(temp.path(), 21000).unwrap();
        let path = temp.path().join("committee.json");
        let original: Committee = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let mut members = original.authorities_slice().to_vec();
        members[1].protocol_key = members[0].protocol_key.clone();
        fs::write(
            &path,
            serde_json::to_vec(&Committee::new(0, members)).unwrap(),
        )
        .unwrap();
        let config = temp.path().join("node0/config.json");
        assert!(
            load(&config)
                .err()
                .unwrap()
                .to_string()
                .contains("duplicate committee protocol key")
        );
        let mut members = original.authorities_slice().to_vec();
        members[1].address = members[0].address.clone();
        fs::write(
            &path,
            serde_json::to_vec(&Committee::new(0, members)).unwrap(),
        )
        .unwrap();
        assert!(
            load(&config)
                .err()
                .unwrap()
                .to_string()
                .contains("duplicate committee network address")
        );
    }
}
