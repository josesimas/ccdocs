#!/usr/bin/env sh
# Rebuild (debug) and run ccdocs. Arguments are passed through, e.g.: ./run-debug.sh --group 100kts --out out
cd "$(dirname "$0")" || exit 1
cargo build || exit 1
exec target/debug/ccdocs "$@"
