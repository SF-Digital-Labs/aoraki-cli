# Upstream sync: keeping aoraki aligned with the codebases it deploys through

**Goal:** the CLI and the aoraki platform must stay in sync with the external
codebases they build on — the Fred provider daemon, the Manifest chain
(cosmos-sdk + the `liftedinit` modules), and the manifest SDK ecosystem —
without silently drifting as those projects evolve.

**Context:** the platform is Rust; the chain is Go with TypeScript bindings
generated from its protobuf definitions (telescope → `manifestjs` /
`manifest-sdk`). Aoraki re-implements a slice of that surface in Rust. This
document is the honest inventory of that slice, the risk each piece carries,
and the concrete mechanisms that keep it in sync.

---

## 1. Inventory: what is re-implemented, and the risk of each piece

The re-implemented surface is ~720 lines, all in `crates/core-chain`
(plus thin HTTP shims in the CLI). Piece by piece:

| Surface | Size | Replicates | Change likelihood | Failure mode & detection |
|---|---|---|---|---|
| Cosmos tx envelope (`TxBody`, `AuthInfo`, `SignDoc`, `TxRaw`, `MsgSend`, secp256k1 `PubKey`) | ~150 lines | cosmos-sdk wire format | **Near zero** — SIGN_MODE_DIRECT has been stable since cosmos-sdk 0.40 (2021); protobuf wire compatibility is an ecosystem guarantee | Chain rejects the tx at broadcast; loud, immediate |
| `liftedinit.billing.v1` messages (5 msgs, 43 fields) | ~100 lines | The chain's billing module — **actively developed** | **High** — new fields and validation land here first | Runtime: chain error on broadcast, recorded on the deployment row (insert-first rule). Protobuf forward-compat means *existing* fields keep encoding correctly; the risk is new *required* semantics, and unusable new features until we add them |
| ADR-036 bearer tokens (read + payload flavours) | ~80 lines | Fred's `internal/auth` + `internal/api` token JSON | **Medium-high** — per-operation token scoping is a known planned change upstream | Fred returns 401; every call site surfaces the failure. **Strongest-tested surface we have**: byte-identical output vs Fred's own `cmd/lease-token` binary is pinned as a regression test |
| Fred tenant REST client (`/v1/leases/*`) | ~150 lines | Fred's HTTP handlers | Medium | Tolerant `serde_json::Value` parsing degrades to missing fields, never panics; errors recorded, never silent |
| LCD REST queries (chain read paths) | ~130 lines | Chain query API (itself generated from the same protos) | Low-medium (chain version upgrades) | Read-only; tolerant parsing |
| Render Compute client (`core-render`) | separate crate | **Nothing** — no official SDK exists in any language; every consumer hand-rolls this HTTP+HMAC contract | n/a | Not a re-implementation risk regardless of language choice |

Two honest observations that size the problem correctly:

- The generated-bindings advantage covers **chain protos only**. The SDK's
  Fred HTTP layer (`packages/fred/src/http/` — auth, endpoints, provider
  calls) is hand-written TypeScript, exactly as ours is hand-written Rust.
  For that surface, staying in sync is an *organizational* problem (release
  coordination) for every consumer, in every language.
- Generated ecosystems have their own upgrade treadmill: an SDK minor
  release recently raised its Node engine floor past our build image —
  caught as build warnings. "Free upgrades" still means chasing releases;
  the difference is which tool reports the drift.

### Crate dependencies (the boring part, verified)

`core-chain` depends on `k256`, `bip39`, `bip32`, `prost`, `sha2`, `bech32` —
RustCrypto-ecosystem crates that the official `cosmrs` project itself builds
on. Low churn, additive releases, pinned to majors. The CLI is thinner still
(clap / serde / ureq). Supply-chain and release-chasing risk in the crates
themselves is minimal; **the risk lives in the contracts above, not the
dependencies.**

---

## 2. Mechanism 1 — generate the proto layer, stop hand-writing it

The correct response to "you won't get upgrades for free" is not a language
change — it is codegen from the same source of truth the TS bindings use.
`prost-build` generates Rust from protobuf exactly as telescope generates
TypeScript.

**Work plan (target: `crates/core-chain`, ~half a day):**

1. **Vendor the protos.** Copy from `manifest-ledger` at a pinned release
   tag into `crates/core-chain/proto/`:
   - `liftedinit/billing/v1/{tx,query,types}.proto`
   - `liftedinit/sku/v1/{tx,query,types}.proto`
   - their imports: `gogoproto/gogo.proto`, `cosmos_proto/cosmos.proto`,
     `amino/amino.proto`, `google/api/*.proto`, `cosmos/base/v1beta1/coin.proto`
2. **Record the pin.** `crates/core-chain/UPSTREAM.md` lists each vendored
   source, its repo, and the exact tag/commit. This file is the single place
   to answer "what version are we speaking?"
3. **Build-time codegen.** `build.rs` with `prost-build` compiles the
   vendored protos into a `generated` module. No network at build time —
   vendored files only, so builds stay hermetic.
4. **Envelope from the ecosystem crate.** Replace the hand-written cosmos
   envelope structs with the `cosmos-sdk-proto` crate (the same generated
   types `cosmrs` uses), keeping our thin signing/assembly wrappers.
5. **Delete `proto.rs` hand structs**; keep `wallet.rs` / `tx.rs` / `lcd.rs`
   / `fred.rs` as the hand-written *logic* layer over generated *types*.
6. **Upgrade procedure** (documented in UPSTREAM.md):
   `bump tag → re-vendor → cargo build → fix compile errors → run canaries`.
   Every upstream schema change becomes a compile error or a green build,
   never a silent runtime divergence.

What this deliberately does **not** cover: the ADR-036 token JSON and the
Fred REST shapes — those are not protobuf anywhere; they are covered by
Mechanism 2.

---

## 3. Mechanism 2 — CI canaries that detect drift before customers do

Two jobs, run on every PR touching `core-chain`/the CLI **and on a weekly
cron** (the cron matters: upstream moves without us committing anything).

**a) Token byte-parity vs the provider's own binary.**
The provider daemon ships `cmd/lease-token`, the reference implementation of
its bearer-token format. The canary:

1. Checks out the fred repo at the tag pinned in `UPSTREAM.md`.
2. `go build ./cmd/lease-token`.
3. Mints tokens (read flavour; payload flavour via a tiny fixture harness)
   with a fixed public test mnemonic, lease UUID, and timestamp.
4. Mints the same tokens through our Rust implementation (a
   `#[test]`-invoked helper — the static-fixture parity test we already
   have, upgraded to compare against the freshly built binary instead of a
   frozen string).
5. Asserts byte equality. A mismatch = the token contract moved; the CI
   failure message names the pinned tag vs upstream HEAD.

**b) Read-only contract test against testnet.**
A test (feature-gated `--features contract-tests`, network-touching, free):

- LCD: fetch node_info, one SKU, one provider, one lease, a credit account —
  assert every field our parsers depend on exists with the expected type
  (chain-upgrade shape drift).
- Fred: `GET /health` (200), and one authenticated `status` call against a
  known lease UUID asserting the error/response *shape* (auth accepted,
  structured JSON) — this exercises the full token path against the live
  provider without spending anything.

**c) Failure policy.** Canary red does not block feature PRs (upstream broke,
not us); it opens the upgrade procedure from Mechanism 1 + the sync review
below. The weekly cron is the early-warning radar.

---

## 4. Mechanism 3 — release coordination with upstream

Code can't detect a change that hasn't shipped; process covers the gap.

1. **Watch list.** Release notifications for: the fred repo, manifest-ledger,
   and manifest-mcp-mono (SDK). Each release triggers a short sync review:
   read the changelog against our inventory table (section 1), decide
   no-op / regenerate / code change, and record the decision.
2. **Contract-change notice (the ask upstream).** Request that changes to
   the **tenant API contract** — token format, endpoint paths,
   request/response shapes — be called out explicitly in fred's release
   notes under a "tenant API" heading. Every out-of-repo consumer (the TS
   SDK included) needs the same notice, so this is a general-good request,
   not special treatment. The known-planned per-operation token scoping is
   the first concrete case: we want it as scheduled work, not an incident.
3. **Pin discipline.** All upstream versions we speak are recorded in
   `UPSTREAM.md` files (core-chain for chain+fred pins; the frontend's
   package.json is already the pin for the TS SDK). No implicit "whatever
   was on main when we read it."
4. **Same discipline on the TS side.** The frontend pins
   `manifest-sdk`/`manifestjs` with caret ranges today; version bumps go
   through the same sync review (engine floors, breaking changes) — the
   recent Node-engine bump is the standing example.

---

## 5. Standing posture (already in place, keep it)

- **Tolerant parsing** everywhere we read upstream JSON — missing fields
  degrade, never panic.
- **Insert-first + recorded errors** — drift shows up as an honest error on
  a row with a timestamp, org, and author, never as silent corruption.
- **Scrubbed 5xx with reference ids** — when drift does hit a customer path,
  the customer sees a reference, and the log names the real upstream error.
