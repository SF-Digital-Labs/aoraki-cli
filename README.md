# Aoraki CLI

Heroku-style deploys straight to your own servers. Push code from your laptop straight to a server, run its build-and-deploy pipeline, stream the logs back, and report the deploy to the Aoraki console.

```
aoraki deploy staging      # push HEAD → box builds → k3s rolls out, logs stream live
aoraki logs staging -f     # follow app logs
aoraki status staging      # deployed sha vs local HEAD, pod readiness
```

Full product spec: `infrastructure/aoraki-cli-spec.md` in the manifest-internal-docs repo. Transport decision (direct git push to the box): ADR 006 in the same repo.

## How it works

- Each app on each box gets a bare git repo at `/data/git/<app>.git` with a post-receive hook (installed by `aoraki link`).
- `aoraki deploy` force-pushes the exact SHA over SSH; the hook checks it out (detached) into `/data/repos/<app>` and runs the app's existing `k8s/scripts/build-and-deploy*.sh` — deploy logic stays in the app repo, not in this binary.
- Build output streams back through the push. The build runs under `nohup` server-side, so a dropped connection never kills a deploy.
- SSH uses `ControlMaster` multiplexing: one handshake per session, every later command reuses the socket. Your `~/.ssh/config` works unchanged.
- Deploy history is recorded on the box (`/data/logs/aoraki-deploys/<app>.jsonl`, source of truth) and mirrored to Aoraki's `POST /api/deploys` (best-effort, queued locally when unreachable).
- GitHub is untouched: it stays your collaboration remote; `ucn`/`dcn` on the boxes remain the manual fallback.

## Install

One-liner (macOS Apple Silicon/Intel, Linux x64 — grabs the latest release):

```bash
curl -fsSL https://sf-digital-labs.github.io/aoraki-cli/install.sh | sh
```

Or download a binary from [Releases](https://github.com/SF-Digital-Labs/aoraki-cli/releases),
or build from source:

```bash
cargo build --release
cp target/release/aoraki /usr/local/bin/    # or anywhere on PATH
```

Docs & landing page: [sf-digital-labs.github.io/aoraki-cli](https://sf-digital-labs.github.io/aoraki-cli) (interim — product domain TBD, ADR 011)
(served from `docs/` via GitHub Pages; releases are built by
`.github/workflows/release.yml` on version tags).

## Setup

1. Add a `aoraki.toml` to your app repo root (see `examples/aoraki.toml`). In a monorepo, put one in each app's directory — the CLI walks upward from wherever you run it.
2. Run `aoraki login --url <aoraki-api-url>` — opens the console's CLI tokens page; paste a token and it lands in `~/.config/aoraki/config.toml` (0600). Tokens die after 90 days unused; any CLI call restarts the clock.
3. Run `aoraki link` — creates the bare repos + hooks on every configured server (idempotent; re-run any time to repair/upgrade hooks).
4. Run `aoraki doctor` — verifies the whole chain: SSH, hook version, workdir, docker, kubectl, disk, Aoraki remotes.
5. `aoraki deploy <env>`.

Server addresses not in `~/.ssh/config` and a default environment also live in the global config (see `examples/global-config.toml`).

## Remotes: several Aoraki consoles from one machine

Each console you deploy through is a named *remote* — company cloud, personal cloud, staging console:

```bash
aoraki login company  --url https://aoraki.manifest.network/api/v1
aoraki login personal --url https://aoraki.example.dev/api/v1
aoraki whoami                       # who each token maps to (also extends the 90-day window)
aoraki logout personal              # forget the local token (revoke in the console too if needed)
```

```toml
# ~/.config/aoraki/config.toml (written by `aoraki login`)
[remotes.company]
api_url = "https://aoraki.manifest.network/api/v1"
token = "cli_…"

[remotes.personal]
api_url = "https://aoraki.example.dev/api/v1"
token = "cli_…"

[defaults]
remote = "company"                    # used when an environment doesn't pin one
```

Deploy events go to one remote per environment: `remote = "personal"` in an
environment's section of `aoraki.toml`, else `[defaults].remote`, else the
sole configured remote.

## Commands

| Command | Purpose |
|---|---|
| `aoraki login [remote] [--url …]` | Connect to an Aoraki console (paste-a-token) |
| `aoraki logout [remote]` | Forget the stored token |
| `aoraki whoami [remote]` | Who each token maps to; probes the API |
| `aoraki link` | Set up bare repos + deploy hooks on the boxes (idempotent) |
| `aoraki deploy [env] [--ref <commit>]` | Push and deploy; production envs require typed confirmation |
| `aoraki logs [env] [-f] [--since 1h] [--previous]` | App logs from the pods |
| `aoraki status [env]` | Deployed sha vs local HEAD, pod readiness |
| `aoraki deploys [env]` | Recent deploy history from the box-local log |
| `aoraki run [env] -- <cmd>` | One-off pod with the deployed image (migrations, shells) |
| `aoraki restart [env]` | Rolling restart without a build |
| `aoraki open [env]` | Open the env's URL |
| `aoraki doctor [env]` | Preflight the whole deploy chain |
| `aoraki config [env]` | Configmap/secret key names (never values) |

Only committed code deploys — there is deliberately no way to push an uncommitted working tree. Everything running on a box is traceable to a commit.

## Not yet implemented (v1 roadmap)

- `aoraki rollback`
- `aoraki login` device flow (paste-a-token is the v1)
- `aoraki init` provisioning — new apps still follow playbook § 12
