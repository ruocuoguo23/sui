#!/usr/bin/env bash
# Copyright (c) Mysten Labs, Inc.
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
exec python3 "$(dirname "$0")/cluster.py" start "$@"
