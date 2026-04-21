# Changelog

All notable changes to this project will be documented in this file. Format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] — production-hardening pass (post-round-2 adversarial review)

**Critical fixes:**
- Layer 2 format-aware cleanup now actually runs when no profile matches.
- Auto-synthesis fires on the hot path when sample capture meets the
  lock-in threshold — the "learned Layer 5" story is real and wired.
- `nid update --from` has rotation-chain verification, sha256 integrity,
  and refuses self-signed tarballs when no release anchor is pinned.
- Shadow mode emits the REDACTED raw output (was leaking secrets).
- `nid show --raw-unredacted` is now a real policy lever: raw is stored
  AES-GCM-sealed (encrypted with a machine-local key at `<data>/key`),
  and the flag decrypts without re-applying redaction under interactive
  confirmation, with audit logging to `show_access.log`.
- DSL interpreter now has an execution budget (plan §11.4 / Appendix B):
  10M steps, 2000ms wallclock, 64MB peak memory. Overrun aborts to
  Layer-1 output and quarantines the offending profile.

**Security:**
- Release installs require `NID_RELEASE_ANCHOR_HEX` baked in at build
  time; without it, signed tarballs are refused unless
  `NID_RELEASE_ALLOW_UNANCHORED=1` is set explicitly.
- `security.redaction.deny_commands` opts a command into aggressive
  redaction (high-entropy sweep).
- `security.redaction.allow_commands` opts out of default redaction.
- `session.allow_raw_commands` forces raw persistence even if
  `preserve_raw=false` globally.
- `session.max_total_mb` (default 2048) caps per-invocation output
  capture with a truncation marker.
- Auto-synthesized profiles that abort their own training-sample budget
  are rejected.
- Profile purge now releases both dsl and rubric blobs.
- `nid profiles import --allow-unsigned` requires interactive confirmation
  (or `NID_UNTRUSTED_OK=1`).

**Correctness:**
- SQLite `busy_timeout=500ms` for multi-process hot-path writes.
- `fidelity_events` `bypass_signal` rows feed a rolling-window score
  (100-session, 3-run warmup); profiles exceeding threshold get
  quarantined.
- Exit-code skew recorded on every finalize when buckets have ≥50 samples.
- Per-fingerprint advisory lock guards auto-synthesis races.
- Opportunistic retention purge releases ALL blob refs (was capped at 256).
- `gain_daily` rollup populated on each finalize.
- `nid doctor` does real checks: SQLite round-trip, blob round-trip,
  TCP-probe Ollama, hook-SHA drift detection, JSON-parsed co-installed-
  hook count, perms, recent unredacted-access log summary.
- Ollama backend TCP-probes before returning Some.
- Hook handler reads `shadow.state` on every invocation.

**Release pipeline:**
- `.github/workflows/release.yml`: 5 targets (linux musl x86_64,
  linux gnu aarch64, darwin x86_64, darwin aarch64, windows x86_64).
- `nid-package` packer binary runs on the host, not the target.
- `nid-keygen` for initial signing-key generation.
- Hook response carries `additionalContext.nid` attestation block.

**Tests: 213 passing** (was 160 at 0.1.0, 188 at the round-1 verification).

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
