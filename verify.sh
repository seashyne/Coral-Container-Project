#!/bin/bash
set -e

# Verify we are in the coral directory
cd "$(dirname "$0")"

echo "Building project..."
cargo build

echo "Running verification with sudo..."
# Run the binary with sudo, verify PID 1 and hostname isolation
# The binary prints to stdout
OUTPUT=$(sudo ./target/debug/coral)

echo "$OUTPUT"

# Verify hostname change
if [[ "$OUTPUT" == *"Hostname: \"coral-container\""* ]]; then
    echo "Verification Passed: Hostname changed."
else
    echo "Verification Failed: Hostname not changed."
    exit 1
fi

# Verify PID 1
if [[ "$OUTPUT" == *"Child: PID: 1"* ]]; then
    echo "Verification Passed: Child PID is 1."
else
    echo "Verification Failed: Child PID is not 1."
    exit 1
fi

echo "All verifications passed."
