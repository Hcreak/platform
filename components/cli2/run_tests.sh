set -euo pipefail

export FINDORA_HOME=$(mktemp -d)
echo "FINDORA_HOME: ${FINDORA_HOME}"

# Build the executable
cargo build

# Run the tests
cargo test

# Run clippy
cargo clippy

# Black box tests written in shell
bats tests/balances.sh
bats tests/transfers.sh
bats tests/simple-cli.sh
bats tests/advanced-cli.sh
bats tests/error-handling.sh
