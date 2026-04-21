# Changelog

All notable changes to this project will be documented in this file. Format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] — initial architecture-complete snapshot

First end-to-end build against `docs/v1-architecture.md`. Not yet
release-ready; see README for what is and isn't in this cut.

### Added
- 8-crate Cargo workspace (`nid-cli`, `nid-core`, `nid-dsl`, `nid-storage`,
  `nid-hooks`, `nid-fidelity`, `nid-synthesis`, `nid-profiles`).
- 11-table SQLite schema (Appendix A) with forward-only migration runner.
- Content-addressed zstd blob store with ref-counted GC.
- Scheme R command fingerprinting (paths, numbers, URLs, hex, quoted
  strings collapse; shape-defining flag values preserved).
- Pre-persistence secret redaction: AWS, GitHub (classic + fine), GitLab,
  Stripe (live + test), JWT, SSH PEM blocks, Bearer-header, high-entropy
  heuristic.
- DSL: 14 rule kinds, 6 invariant checks, grammar validator (rejects regex
  backreferences, bad schemas, malformed regexes, empty state machines,
  duplicate state/invariant names, zero head/tail, bad collapse_repeated
  min, unsupported JSON-path constructs), pure-Rust streaming interpreter,
  structural-diff synthesizer as the "always-works" floor.
- 8-agent hook installer (Claude Code, Cursor, Codex CLI, Gemini CLI,
  Copilot CLI, Windsurf, OpenCode, Aider) with JSON-merge into real config
  files + byte-perfect `onboard.backup.json` for uninstall.
- Idempotent hook rewrites: `NID_RAW=1` escape, builtin/tee/cat pass-
  through, whole-pipeline wrap, `--shadow` prefix, `nid nid` de-duplication.
- Layer 1 generic cleanup (streaming dedup, strip ANSI, strip CR,
  head/tail envelope).
- Layer 2 format auto-detect (JSON, NDJSON, unified diff, stack trace,
  tabular, log, plain).
- 10 bundled Layer 3 profiles: `git status`, `git log`, `git diff`,
  `cargo build`, `pytest`, `npm install`, `docker ps`, `kubectl get pods`,
  `jq .`, `go test` — each with raw + expected-compressed fixture.
- Layer 5 dispatch: persisted profiles take priority over bundled.
- Sample capture for unknown fingerprints (cap 64 per fingerprint).
- `nid synthesize <cmd> [--force]`: structural-diff synthesis + self-test
  against every captured sample + status-flip promotion to `active`.
- Lock-in policy: N=5 default, N=3 zero-variance fast path, doubling
  re-refinement at 5/10/20/40/... samples.
- Tier 1 invariant checks + Tier 2 structural-subset check in the hot path;
  fidelity events persisted; fidelity score in the attestation footer.
- 6-signal bypass tracker with 3-run warmup + 100-run rolling window.
- Exit-code >2x skew detection.
- SIGTERM trap: partial output preserved + `--- [nid: interrupted] ---`
  terminal marker.
- CLI surface: `version`, `doctor`, `onboard` (with `--check`,
  `--non-interactive`, `--uninstall [--purge]`, `--agents`, `--budget`,
  `--disable-synthesis`), `show`, `sessions`, `profiles list/inspect`,
  `gain [--shadow]`, `shadow {enable,disable,commit}`, `synthesize
  [--force]`, `trust {add,revoke,list}`, `gc`, `update [--check, --from,
  --channel]`.
- Hidden `nid __hook <agent>` handler used by agent hook configs.
- Test suite: 160 tests across 8 crates, including golden-fixture harness,
  one-test-per-redaction-pattern coverage, one-test-per-forbidden-DSL-
  primitive, 8-agent payload handling, uninstall roundtrip, DSL validator,
  and synthesis end-to-end.

### Not yet
- Network LLM backends (Anthropic, Ollama, claude CLI stubs ship; wiring is
  planned for 0.2).
- Signed release channel + `nid update` over the network.
- MCP server (plan §9 — deferred to v1.1).
- The remaining ~20 bundled profiles to reach the ~30-profile v1 target.
- Homebrew formula, `cargo install` metadata tightening, curl-to-shell
  installer, GitHub Actions release workflow.
- Cross-compile targets pinned in CI (works locally; CI matrix TODO).
