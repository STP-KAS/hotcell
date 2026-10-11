> **Experimental. Not advice. Testnet-10 only.** tKAS and hotcell have no value. [DISCLAIMER.md](DISCLAIMER.md)

# hotcell

One shared, contended KCC-20 mint cell on Kaspa Testnet-10, made for drills. Each mint issues exactly 1 unit. Every minter spends the same covenant output, so mints collide. To get yours in, especially during a TN10 storm, your client has to track its in-flight spend, raise its fee, and replace its own transaction.

The token is not the point. The contended cell is.

## Why

Real covenant dapps will have hot shared state: a pool, an order book, a counter, a mint. Many users want to spend the same output at the same moment. Our TN10 storms tested plain payments and P2SH lanes under congestion. They never gave builders one standard covenant output to fight over. hotcell is that output.

It fits the [tn-master goal](https://github.com/STP-KAS/tn-master/blob/main/WHAT-WHY-HOW.md#goal): rehearse under real congestion, not made-up numbers. It also exercises the [advice to developers](https://github.com/STP-KAS/tn-master/blob/main/ADVICE-TO-DEVELOPERS.md):

- raise your fee when congestion rises, with a cap (items 1 and 3);
- make sure your fee setting is actually used (item 4);
- re-broadcast after a reconnect (item 5);
- track coins already in flight and chain on your own unconfirmed outputs (item 8);
- respect storage mass (item 9): one hotcell mint with a 1 tKAS carrier weighs about 73,000 grams, so at most 6 fit in one block.

Your hotcell balance just says your client landed that many covenant spends. Nothing more.

## What

| Field | Value |
| --- | --- |
| Name | `hotcell` (no ticker or decimals on chain; by convention the ticker is `HOTCELL` and decimals are 0) |
| Standard | KCC-20 (Last Call), app `KCC20PublicMint`, argent-lang/kcc20-reference `c8a08711`, contracts unchanged |
| Extension commitment | BLAKE3(`hotcell`) = `7c0b0bfc259103c0dfc9869dfa14ec23cda935978bec1dde6d837ea8313f5b8c` |
| Launch path | `launch::hotcell-v1` |
| Covenant id | `a549ffb3ae3c030ce888a514c731f6b3dc3aad469c28763ef60c482540d13aa7` |
| Supply | 1,000,000 units, all in one public minter at launch |
| Per-mint limit | 1 |
| Premine | none |
| Genesis group | one PublicMint and one TokenSeed |
| Minter deposit | 1 tKAS |
| Network | Testnet-10 only |

Mint rule (enforced by the covenant): anyone may call `mint` with a recipient state whose amount is exactly 1 (positive, at most the limit, at most what remains) and whose extension commitment is the one above. The minter's 1 tKAS deposit carries over. The caller pays the fee and the KAS on the new token output.

## What KCC-20 enforces and what it does not

Enforced by the covenant on L1:

- the 1,000,000 cap, the 1-per-mint limit, and the extension identity;
- a token amount is kept the same across transfers inside this covenant family;
- the deposit comes back only when remaining is 0 and the owner signs. With 1,000,000 units of 1 that will not happen, so treat the 1 tKAS deposit as locked.

Only convention, not enforced:

- the name, ticker and decimals. Only the BLAKE3 digest is on chain;
- that a mint happened "under congestion". A covenant cannot see the mempool or the fee market. The proof of congestion is the block time of your mint compared with a storm's published logs, not the token;
- who mints. There is no allowlist and no Sybil resistance. One key can mint every unit;
- that there is only one cell. The reference minter has a permissionless `split`: anyone may move up to half of the remaining allowance into a new minter they own. So the hot cell can be sharded. That is not a bug in this drill. Sharding hot state is the real fix, and splitting under contention is a legitimate move.

## Honest critique

- A token is not strictly needed. The useful object is a covenant output many people race to spend. A plain counter covenant would do the same job. KCC-20 was chosen because it is the standard, the reference compiles, and wallets and indexers will learn to read it.
- The balance proves nothing about skill or congestion. Do not rank people by it.
- Throughput is low by design. A single cell takes one spend per chain step, and storage mass limits it to about 6 mints per block with a 1 tKAS carrier. That is the drill, but it means hotcell is not a load generator.
- Griefing is possible. Anyone can flood the cell at a high fee and crowd out everyone else for as long as they pay. On TN10 that costs nothing real. On mainnet the same pattern would cost real money. That is the lesson, not a security guarantee.
- On TN10, fees are paid in worthless tKAS. hotcell shows the mechanics, not attack cost.
- KCC-20 is Last Call, not Final, and the reference is not audited. Mint and transfer rules may change before Final. Treat this family as disposable.
- It is not a faucet, a reward, a badge of honour or an airdrop. The [sixpack.wtf](https://sixpack.wtf) square already has its own TN10 faucet and in-page mint book. hotcell does not copy either: it is a real covenant on L1, and it pays nothing.

## How a developer uses it

1. Build argent-lang/kcc20-reference at `c8a087117735a1f87c5c6d115fcddeaf2562c784` with [tools/hotcell.rs](tools/hotcell.rs) added as a mode (see [tools/README.md](tools/README.md)). Or write your own client against the KCC-20 Program ABI.
2. Find the current minter output of the covenant above. It is the newest unspent output in that covenant family with the PublicMint state. Each mint moves it to output 0 of the mint transaction.
3. Build a mint: input 0 is the minter (`mint` entry, amount 1, your public key, owner scheme `0x00`, borrow scheme `0x00`), input 1 is your funding coin. Output 0 is the next minter (same 1 tKAS). Output 1 is your 1-unit KCC-20 output. Output 2 is your change.
4. Submit to a synced TN10 node. If someone else spent the minter first, your mint is a double spend. Rebuild on the new minter. If yours is still in the mempool and too slow, replace it with a higher-fee version that spends the same inputs (`submitTransactionReplacement`). Cap the fee.
5. During a storm, read the node's fee estimate every few seconds and follow the tn-master advice above.

## First run (11 Oct 2026, about 18:35 CEST)

Launched by Grok Bot from its own TN10 wallet through a synced local Testnet-10 node:

- Genesis: 1, accepted. Fee 0.08009 tKAS, storage mass 80,090.
- Mints: 5, each 1 unit, to the launcher's own key. Remaining after the run: 999,995.
- Replace-by-fee check: mint 5 was first sent at 100 sompi/gram, then replaced with the same inputs at 300 sompi/gram (fee 0.218715 tKAS). The replacement was accepted and the original never appeared on chain. So RBF works on a KCC-20 covenant spend on TN10.
- Mint shape: compute mass 9,115, storage mass 72,886 to 74,845 at a 1 tKAS carrier. Fee 0.0729 to 0.0748 tKAS at 100 sompi/gram.
- Fees paid: 0.59564 tKAS for genesis and mints. Locked or carried: 7 tKAS (1 minter deposit, 1 seed, 5 token carriers). Funding came from a 0.009978 tKAS consolidation of the wallet's own small change coins.
- Rejects: 0.

This was a small launch test, not a congestion drill. No storm was running.

Intentions are good; thought process is questionable. STP remains delusional. Si vis pacem, para bellum.
