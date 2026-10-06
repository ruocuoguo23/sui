# Sui source reference archive

These files are copied byte for byte from MystenLabs/sui release `mainnet-v1.80.1`, commit `782577ca40c9ef0b3f400ade8bc2cc2dcd0e2b23`. They are outside the Dagora workspace and retained for reading and later checkpoint, epoch and state sync ports. Original manifests and tests describe the upstream Sui build; this archive is not maintained as a buildable Sui distribution.

`manifest.json` records every original repository path, SHA-256 digest and byte length. Paths under this directory match upstream paths. Code is covered by [Apache-2.0](LICENSE); documentation licensing is in [LICENSE-docs](LICENSE-docs). Original copyright notices are preserved.

- [sui-core](crates/sui-core): checkpoint construction/certification, consensus handling and epoch recovery.
- [sui-types](crates/sui-types): checkpoint and committee types.
- [sui-node](crates/sui-node): epoch and component orchestration.
- [original simulator](crates/sui-simulator): source before removal of Sui execution dependencies.
- [state sync](crates/sui-network/src/state_sync): synchronization implementation and tests.
- [end-to-end reference tests](crates/sui-e2e-tests/tests): checkpoint, reconfiguration and committee scenarios.
- [consensus](docs/content/develop/sui-architecture/consensus.mdx), [epochs](docs/content/develop/sui-architecture/epochs.mdx), [checkpoint verification](docs/content/develop/sui-architecture/checkpoint-verification.mdx), [storage](docs/content/develop/sui-architecture/sui-storage.mdx).
- [Sui Lutris paper](doc/paper/sui-lutris.pdf).

The DAG commit algorithm remains in the active `consensus/core` crate. Later Dagora ports replace Sui effects, objects and system transactions with application blocks and app hashes.
