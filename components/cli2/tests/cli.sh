#!/usr/bin/env bats

# Start from a fresh state
rm -f ~/.findora/cli2_data.json

@test "key generation" {
  run $CLI2 key-gen alice
  echo ${lines[0]}
  [ "$status" -eq 0 ]
  [ "${lines[0]}" = 'New key pair added for `alice`' ]
}

