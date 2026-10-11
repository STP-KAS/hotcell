> **Experimental. Testnet-10 only.**

# hotcell builder

`hotcell.rs` is a mode for argent-lang/kcc20-reference `c8a087117735a1f87c5c6d115fcddeaf2562c784`, app `KCC20PublicMint`. The contracts are not changed. It builds one genesis (PublicMint plus TokenSeed under `launch::hotcell-v1`) and a short chain of 1-unit mints. It can also build a higher-fee replacement of the last mint with the same inputs. It signs locally and prints no key. Submitting is up to you.

## Build

1. Check out the reference at that commit.
2. Copy `hotcell.rs` to `src/bin/kcc20/hotcell.rs`.
3. In `Cargo.toml` `[dependencies]` add:

```toml
kaspa-addresses = { git = "https://github.com/kaspanet/rusty-kaspa", rev = "a41a333b08848f41bf737b72592e463a6011b8ac" }
borsh = "1.8.1"
```

4. In `src/bin/kcc20/main.rs` add `mod hotcell;` and this match arm before the catch-all:

```rust
Some("hotcell") => return hotcell::run(&artifact),
```

5. `cargo build --release --bin kcc20`. Rust 1.94.1, as pinned by the reference.

## Environment

| Variable | Meaning |
| --- | --- |
| `HC_KEY` | file with one 64-hex secret (keep it outside any repo) |
| `HC_PAYER_ADDRESS` | the `kaspatest:` P2PK address of that key; the tool refuses a mismatch or a mainnet address |
| `HC_FUNDING` | `txid=`, `index=`, `amount=`, `coinbase=`, `blockDaa=`, `script=` lines for one coin of that key, at least 10 tKAS |
| `HC_FEE_RATE` | sompi per gram, 1 to 10,000 |
| `HC_MINTS` | number of chained mints, 1 to 50 |
| `HC_BUMP_RATE` | optional; a higher rate builds a replacement of the last mint |
| `HC_OUT` | output folder: `txs.jsonl` (one signed tx per line, Borsh hex included) and `summary.json` |

Note: this tool launches a new family every time it runs. To mint into the existing hotcell family you need a mint-only mode that starts from the current minter output. That is not in this folder yet.

The fee is `max(storage, compute, ceil(transient / 2))` times the rate.
