# ABCI source provenance

The `proto/` files are vendored without modifications. SHA256SUMS pins their
bytes and the included upstream licenses; builds do not download schemas.

- `proto/tendermint/**`: [CometBFT v0.38.22](https://github.com/cometbft/cometbft/tree/v0.38.22/proto), Apache-2.0 (`LICENSE-COMETBFT`).
- `proto/gogoproto/gogo.proto`: [cosmos/gogoproto v1.7.0](https://github.com/cosmos/gogoproto/blob/v1.7.0/gogoproto/gogo.proto), the dependency pinned in CometBFT's v0.38.22 go.mod. BSD-3-Clause (`LICENSE-GOGOPROTO`).
- `proto/google/protobuf/**`: [protobuf v3.21.12](https://github.com/protocolbuffers/protobuf/tree/v3.21.12/src/google/protobuf), standard Timestamp, Duration and Descriptor definitions. BSD-3-Clause (`LICENSE-PROTOBUF`).

The wire service is `tendermint.abci.ABCI` (ABCI 2.0). Go-only gogo annotations
are preserved; Rust prost types follow protobuf presence and timestamp rules.

The fixtures in `tests/fixtures/` were independently encoded using `libprotoc
35.1`, not the shared Rust encoder. Regenerate a fixture with:

```sh
protoc -I proto --encode=tendermint.abci.RequestFinalizeBlock proto/tendermint/abci/types.proto < tests/fixtures/finalize-request.textproto | xxd -p -c 100000
```

Use the matching message type for each fixture. These fixtures and descriptor
checks validate the pinned wire contract. A Rust client/server round trip does
not establish compatibility with an actual Cosmos SDK application; that needs a
separate integration test when the application version is selected.
