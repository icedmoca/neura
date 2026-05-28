# Sharded library tests

Use `scripts/sharded_lib_tests.py` when `cargo test --lib` is too large for a local
terminal, agent harness, or CI job timeout.

The runner:

- discovers library tests with `cargo test --lib -- --list`
- shards deterministically by test index
- prebuilds the libtest binary once
- runs each selected test by exact name with `--exact`
- defaults `CARGO_INCREMENTAL=0` to avoid rustc incremental `verify_ich` ICEs seen
  on very large test builds

Examples:

```bash
# See what a shard would run.
scripts/sharded_lib_tests.py --shard-count 8 --shard-index 0 --dry-run

# Run one shard.
scripts/sharded_lib_tests.py --shard-count 8 --shard-index 0

# Run a focused subset, split into multiple shards.
scripts/sharded_lib_tests.py --filter memory --shard-count 4 --shard-index 0
```

To run the complete library suite, run every shard index from `0` to
`shard-count - 1`. For example, with 8 shards:

```bash
for i in $(seq 0 7); do
  scripts/sharded_lib_tests.py --shard-count 8 --shard-index "$i"
done
```
