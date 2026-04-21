# nid

> A Rust CLI proxy that compresses AI coding agent shell output before it
> reaches the agent's context window. Target: 60–90% token reduction while
> preserving task-success fidelity.

**Status: v0.1.0 — early development.** Architecture in
[`docs/v1-architecture.md`](docs/v1-architecture.md).

## What it does

`nid` sits between your AI coding agent (Claude Code, Cursor, Codex CLI,
Gemini CLI, Copilot CLI, Windsurf, OpenCode, Aider) and the shell. It
intercepts every shell command the agent runs via each agent's native
PreTool hook, compresses the command's output, and returns a
structurally-preserved compact version to the agent's context.

```
┌────────────┐   "pytest -v"   ┌──────────┐   "nid pytest -v"   ┌─────┐
│   agent    │ ───────────────▶│ PreTool  │ ───────────────────▶│ nid │
│            │                 │   hook   │                     │     │
│            │◀─── compressed ─┤          │◀── wrapped output ──┤     │
└────────────┘  with footer    └──────────┘                     └─────┘
```

Compression is layered: a generic cleanup pass always runs, then exactly one
of {learned DSL profile, bundled hand-tuned profile, format-aware pass,
small-LLM fallback}, in priority order. Every invocation persists raw
output to a session store so the agent can fetch it verbatim via
`nid show <session-id>` when it needs to.

## Install

**From source (the only path available in v0.1.0):**

```bash
cargo install --path crates/nid-cli
```

Prebuilt binaries, Homebrew formula, and curl-to-shell installer are planned
for a subsequent release.

## Quick start

```bash
# Discover installed agents and print what onboard would do.
nid onboard --check

# Install hooks into your agents' configs (idempotent, writes backup).
nid onboard --non-interactive

# Run a command through nid manually (the hook will do this for you after
# onboard).
nid git status

# See token savings for the past sessions.
nid gain

# Read full raw output for a prior session.
nid show sess_abcdef0123

# Diagnostics: hook integrity, SQLite health, backend reachability.
nid doctor

# Roll it all back.
nid onboard --uninstall
```

## What's in v0.1.0

Build status: `cargo build --workspace` clean · 160 tests passing.

| Area | Status |
|---|---|
| Cargo workspace (8 crates) | ✓ |
| 11-table SQLite schema + migrations | ✓ |
| Content-addressed blob store + ref-counted GC | ✓ |
| Scheme R fingerprinting (paths/numbers/URLs/hex/quoted-strings collapse) | ✓ |
| Secret redaction (10 built-in patterns + high-entropy heuristic) | ✓ |
| DSL: 14 rule kinds + 6 invariant checks, grammar validator, pure-Rust interpreter | ✓ |
| 8 agent hook installers (Claude Code, Cursor, Codex, Gemini, Copilot, Windsurf, OpenCode, Aider) | ✓ |
| Byte-perfect uninstall via `onboard.backup.json` | ✓ |
| Idempotent hook rewrites (NID_RAW escape, builtins/tee/cat passthrough, pipeline whole-wrap, shadow prefix) | ✓ |
| Layer 1 generic cleanup (streaming, ANSI/CR/dedup/head-tail envelope) | ✓ |
| Layer 2 format detection (JSON/NDJSON/diff/stack/tabular/log/plain) | ✓ |
| 10 bundled Layer 3 profiles with byte-equal golden fixtures | ✓ |
| Layer 5 dispatch over persisted profiles | ✓ |
| Sample capture for unknown fingerprints | ✓ |
| Synthesis orchestrator + structural-diff floor | ✓ |
| Lock-in at N=5 / N=3 zero-variance, doubling re-refinement policy | ✓ |
| Tier 1 invariant checks + Tier 2 structural-subset check wired into hot path | ✓ |
| 6-signal bypass tracker (warmup window, rolling 100-run) | ✓ |
| Exit-code skew detection | ✓ |
| SIGTERM trap → partial output preserved | ✓ |
| Attestation footer: `[nid: profile fp/vX, fidelity N.NN, mode=..., raw via nid show sess_...]` | ✓ |

## What's intentionally not in v0.1.0

- **LLM refinement backends** — the `Backend` trait and synthesis
  orchestrator accept any async backend, and a `NoopBackend` ships today.
  The Anthropic / Ollama / `claude` CLI backends are wired as trait impls
  but not networked in v0.1.0. Structural-diff synthesis always runs as
  the guaranteed floor (plan §7.6).
- **Release-signing + auto-update** — `nid update --check` exists; the
  ed25519-signed GitHub release channel is planned for v0.2.
- **MCP server** — deferred to v1.1 per plan §9.
- **Remote profile registry / org key trust** — command surface exists but
  not networked.

## Layout

```
nid/
├── Cargo.toml                # workspace
├── docs/
│   ├── v1-architecture.md    # full plan (authoritative)
│   └── dsl-reference.md      # DSL rule reference with examples
├── profiles/                 # source TOML for bundled profiles
├── tests/fixtures/           # raw + expected-compressed pairs
└── crates/
    ├── nid-cli/              # binary
    ├── nid-core/             # Compressor trait, fingerprint, redact, Layer 1/2
    ├── nid-dsl/              # DSL AST, validator, interpreter, synthesizer
    ├── nid-storage/          # SQLite + blob store + per-table repos
    ├── nid-hooks/            # per-agent hook writers + onboard
    ├── nid-fidelity/         # invariants, bypass, exit-skew, structural
    ├── nid-synthesis/        # LLM backends + orchestrator
    └── nid-profiles/         # bundled Layer 3 TOML compiled in
```

## Development

```bash
# Build everything.
cargo build --workspace

# Run the full test suite.
cargo test --workspace

# Lint with the strictness of the release gate.
cargo clippy --all-targets -- -D warnings

# Formatting.
cargo fmt --check
```

## License

Apache-2.0 OR MIT.
