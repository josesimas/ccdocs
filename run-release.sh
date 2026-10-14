#!/usr/bin/env sh
# Rebuild (release) and run ccdocs. Arguments are passed through, e.g.: ./run-release.sh --group 100kts --out out
cd "$(dirname "$0")" || exit 1
cargo build --release || exit 1
exec target/release/ccdocs "$@"
