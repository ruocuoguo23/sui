#!/usr/bin/env bash
# Copyright (c) Mysten Labs, Inc.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
cargo fmt --all --check
cargo xclippy --locked -- -D warnings
