# nid — v1 Architecture Plan

> Generated from brainstorm session, 2026-04-21. Repo: https://github.com/newtro/nid

---

## 1. Vision

`nid` is a Rust CLI proxy that sits between an AI coding agent and the shell. It intercepts every shell command the agent runs via each agent's native PreTool hook, compresses the command's output, and returns a structurally-preserved compact version to the agent's context window. Target: 60–90% token reduction across common dev commands while keeping task-success fidelity high enough that the agent never notices the difference — except in the bill.

Three-letter name. One binary. Zero required runtime dependencies. Fully automatic — install, onboard, forget. Every new command it sees gets profiled automatically; profiles improve over time; fidelity is continuously validated; nothing ever asks the user for approval mid-session.

## 2. Problem Statement

AI coding agents (Claude Code, Cursor, Codex CLI, Gemini CLI, Copilot CLI, Windsurf, OpenCode, Cline, Aider) consume tokens linearly with the output of every shell command they run. `cargo build`, `pytest -v`, `docker logs`, `kubectl get pods`, `git log`, `terraform plan` routinely emit thousands of lines the agent will never use. Industry-wide this is a very large, very avoidable spend.

**Prior art we're iterating on:** RTK (rtk-ai/rtk, 19.5k★) proved the concept with ~100 hand-coded command strategies and a Rust single-binary + PreTool-hook architecture. edouard-claude/snip (Go, YAML filters) and abhisekjha/pith (Claude Code hooks with compression levels) are direct v0.x competitors launched in the weeks prior to nid.

**Where nid differentiates** (all six of the brief's moves, refined by this brainstorm):

1. **Layered compression with a 2-tier dispatch, not sequential pipeline.** Generic cleanup always runs; then exactly one of {learned profile, hand-tuned, format-detect, small-LLM catch-all} in priority order. No thresholds. No residual polish.
2. **DSL-based learning system (no Wasm).** On unknown commands, nid synthesizes a declarative TOML/JSON DSL via structural-diff + optional LLM refinement. Interpreted in pure Rust. Zero eval surface. Far simpler than Wasm while keeping the "sandboxed" security story intact.
3. **Continuous fidelity validation at four cost tiers.** Per-run invariant checks (free), per-run structural checks (cheap), sampled judge-model comparison (1% budgeted), behavioral bypass detection (free, indirect via agent activity).
4. **Per-agent PreTool hook coverage for 8 agents out-of-the-box.** Claude Code, Cursor, Codex CLI, Gemini CLI, Copilot CLI, Windsurf, OpenCode + Aider via config-file wrapper.
5. **Environment-adaptive synthesis** with a universal structural-diff floor so airgapped environments always get working profiles.
6. **Attestation-aware output** — every compressed tool result carries profile version + fidelity rating in a visible footer and (where the agent supports it) structured `additionalContext`.

Plus two adjustments this brainstorm made against the original brief:

- **MCP server deferred to v1.1** — v1 is Bash-hook-only. nid does not replace agents' native Read/Grep/Glob in v1. Known trade-off, shipped with eyes open.
- **Always-preserved raw** (user addition) — every invocation writes full raw output to the session store; compressed output carries a `raw_pointer`; agents can read raw via `nid show <id>`.

## 3. Non-Goals

- Not an agent harness. Not an IDE.
- Not replacing any agent's native tools in v1.
- No default telemetry. Opt-in only, clearly documented.
- No Windows native v1. WSL is the Windows story.

## 4. User-Facing Surface

### 4.1 CLI Commands

| Command | Purpose |
|---|---|
| `nid onboard` | Interactive install: detect agents + LLM backends, write hooks, verify. `--non-interactive`, `--check`, `--reconfigure`, `--uninstall`, `--uninstall --purge` |
| `nid <command…>` | Run `<command>` through the compression pipeline. This is what the hook rewrites to. |
| `nid show <session-id>` | Read raw archived output of a prior session. `--raw-unredacted` requires confirmation. |
| `nid sessions` | List recent sessions. Filters by fingerprint, time, exit code. |
| `nid profiles` | `list`, `inspect <fp>`, `pin <fp>`, `revoke <fp>`, `export <fp>`, `import <path>`, `sign <tarball>`, `rollback <fp>`, `purge <fp>` |
| `nid synthesize <cmd>` | Manually trigger profile synthesis for a command. `--force` to bypass cooldown. |
| `nid gain` | Token savings analytics, per-profile ROI, dollar cost conversions by model, synthesis-cost-vs-savings net. `--shadow` variant for Shadow Mode (§14). |
| `nid shadow enable` / `disable` / `commit` | Shadow Mode (§14). |
| `nid trust add <key>` / `revoke <key>` / `list` | Org profile-sharing trust keyring. |
| `nid doctor` | Diagnostics: hook integrity, DB health, blob store integrity, backend reachability. |
| `nid update` / `--check` / `--to <ver>` / `--channel` / `--dry-run` / `--from <tarball>` | Update from `github.com/newtro/nid/releases`. Ed25519-signed. Atomic replace. |
| `nid version` | Current + latest-known + channel. |
| `nid gc` | Force blob GC + session-retention purge. |

### 4.2 On-Disk Layout

```
~/.config/nid/
  config.toml             # user-editable TOML config
  onboard.backup.json     # original agent config backups for byte-perfect uninstall
  hooks/                  # generated per-agent hook scripts (referenced by agent configs)

~/.local/share/nid/       # XDG on Linux; ~/Library/Application Support/nid/ on macOS
  nid.sqlite              # WAL-mode SQLite: metadata, indexes, analytics
  nid.sqlite.backup.<ts>  # migration snapshots
  blobs/
    sha256-<hash>.dsl.zst # DSL profiles (TOML compressed)
    sha256-<hash>.sample.zst
    sha256-<hash>.raw.zst # raw session outputs
  sessions/               # recent session index (ephemeral cache; canonical data in SQLite+blobs)
  key                     # ed25519 local signing key (0600)
  release-key.pub         # rotatable release-verification public key
```

### 4.3 Hook Files Written per Agent

Each agent gets a hook that rewrites `<cmd>` → `nid <cmd>` (idempotent — skips if command already starts with `nid` or resolves to the nid binary). Per-agent hook location table:

| Agent | Hook location | Mechanism |
|---|---|---|
| Claude Code | `~/.claude/settings.json` (or project `.claude/settings.json`) | `PreToolUse` hook with `updatedInput.command`; emits `updatedInput` alone to survive bypassPermissions mode |
| Cursor | `~/.cursor/hooks.json` | `beforeShellExecution`; stdin/stdout JSON |
| Codex CLI | `~/.codex/hooks.json` | `PreToolUse` (Bash-only) |
| Gemini CLI | `~/.gemini/hooks.json` | `BeforeTool` |
| Copilot CLI | `.github/hooks/hooks.json` (per-project) or user-config equivalent | `preToolUse` with `modifiedArgs`/`updatedInput` |
| Windsurf | Cascade pre-hook config | Pre/post Cascade hooks |
| OpenCode | JS/TS plugin in user's plugin dir | `before_tool_call` |
| Aider | `.aider.conf.yml` / `AIDER_SHELL_COMMAND` env | Config-file wrapper (no hook API) |

> **Aider caveat:** unlike the other 8 agents, Aider has no per-invocation hook API in 2026. nid's Aider integration is config-file-based only, so rewrite fidelity depends on the user's Aider config remaining applied across sessions. Users who wipe `.aider.conf.yml` or launch Aider with overrides will silently bypass nid. `nid doctor` detects missing Aider config when Aider is installed.

> **Claude Code bypassPermissions quirk:** Claude Code has a known upstream bug where `updatedInput` is silently dropped when combined with `permissionDecision` under `bypassPermissions` mode. nid's hook handler always emits `updatedInput` *without* `permissionDecision` to work around this. Do not change that pattern without re-testing against the bypass-permissions path.

### 4.4 Hook Rewrite Rules

The per-agent hook writes out a small handler that applies these rules to every shell command it sees:

1. **Idempotent rewrite.** If the command already starts with `nid ` (or the resolved binary is the nid binary itself), pass through unchanged. Prevents `nid nid <cmd>` when two hooks are active or when an agent manually prepends `nid`.
2. **Whole-pipeline wrap.** A pipeline like `pytest | tee log.txt | grep FAIL` is wrapped as a whole: `nid 'pytest | tee log.txt | grep FAIL'`. nid sees the full command string and hands it to a subshell. We don't parse the pipeline and wrap each stage individually (too fragile).
3. **Passthrough list.** Commands that should *never* be wrapped:
   - Shell builtins: `cd`, `export`, `set`, `unset`, `alias`, `source`, `.`, `eval`
   - Pure-redirection commands: `tee`, `cat` used only to route bytes (detected by presence of `>`, `>>`, `|`, `<`)
   - Anything already matching rule 1 (starts with `nid`)
   - Anything the user has added to `hook.passthrough_patterns` in config
4. **`NID_RAW=1` escape hatch.** If the command starts with `NID_RAW=1 ` the hook unwraps back to raw and does not prepend nid. For an agent or user who needs raw output on a specific invocation without disabling nid globally. Appears as the weakest bypass signal in §8.2 — noted but not aggressively penalized.
5. **Hook-ordering metadata.** Hooks are registered with explicit ordering metadata where agents support it. `nid doctor` detects other hooks in the same slot that also rewrite commands and warns about last-writer-wins collisions.

## 5. Architecture at a Glance

```
┌──────────────────────────────────────────────────────────────────┐
│  Agent (Claude Code | Cursor | Codex | Gemini | Copilot | ...)   │
└────┬─────────────────────────────────────────────────────────┬───┘
     │ native Bash tool call: "pytest -v"                      │
     ▼                                                         │
┌──────────────────────────┐                                   │
│  Per-agent PreTool hook  │  (installed by `nid onboard`)    │
│  rewrites → "nid pytest -v"                                  │
└────┬─────────────────────┘                                   │
     ▼                                                         │
┌──────────────────────────────────────────────────────────────┘
│  nid <argv>
│  1. Fingerprint argv  (Scheme R: binary + canonicalized argv)
│  2. Dispatch:          Layer 5 learned? → Layer 3 hand-tuned? → Layer 2 format? → Layer 4 small-LLM
│  3. Spawn child, pipe stdout/stderr through:
│     ┌────────────┐  ┌──────────────────────┐  ┌──────────────┐
│     │ Layer 1    │→│ Dispatched Tier B    │→│ Attestation  │
│     │ generic    │  │ Layer (5/3/2/4)      │  │ footer+_meta │
│     │ cleanup    │  │ (streams chunks)     │  │              │
│     └────────────┘  └──────────────────────┘  └──────────────┘
│  4. Write raw to session store (with redaction pass)
│  5. Record gain/fidelity events in SQLite
│  6. Exit with child's exit code
▲
│  SIGTERM handler: flush buffered compression state, write marker, exit cleanly
```

## 6. Area 1 — Compression Pipeline

### 6.1 Two-Tier Hierarchical Dispatch

**Tier A (always runs, unconditional, free):**
- **Layer 1 — Generic cleanup:** dedup adjacent identical lines, strip ANSI escapes, strip carriage returns, optional head/tail truncation envelope. Streaming, byte-for-byte cheap.

**Tier B (exactly one runs, priority order):**
- **Layer 5 — Learned profile (DSL)** if one exists for this fingerprint.
- **Layer 3 — Hand-tuned strategy (DSL)** bundled with nid.
- **Layer 2 — Format auto-detect** (JSON, NDJSON, unified diff, tabular, stack trace, log).
- **Layer 4 — Small-LLM catch-all fallback** for unknown commands during the learning window.

No thresholds. No residual polish. "Compress as much as possible; what's compressed is compressed."

> **Two deliberate departures from the original brief** worth preserving:
> 1. **Priority order reverses the brief's implied L4→L5 sequence.** A learned profile, trained on real samples of this exact command, is strictly more accurate than a generic-LLM cleanup pass. Learned outranks small-LLM.
> 2. **Layer 4 is reinterpreted from "residual polish" to "bootstrap fallback."** Instead of running after another compressor as a cleanup pass, Layer 4 runs only when nothing else applies — as the fallback for unknown commands during the learning window before Layer 5 locks in. This is why there are no residual thresholds in v1.

### 6.2 Streaming + SIGTERM Semantics

- Pipeline is streaming-first internally. Each layer exposes `probe(&preview)` + streaming `compress(&mut input, &mut output)`. Layer 1 streams line-by-line. Tier B layers stream line-by-line or windowed depending on DSL declaration.
- nid doesn't impose a wall-clock budget — the agent's Bash timeout is the only authority. nid is transparent in timing.
- SIGTERM trap: flush in-flight compression state, write a terminal marker `--- [nid: interrupted] ---`, exit cleanly. Partial output is preserved for the agent.

### 6.3 Layer Interface (Compressor Trait)

```rust
trait Compressor {
    fn probe(&self, preview: &[u8], ctx: &Context) -> Applicability;
    fn compress(
        &self,
        input: &mut dyn Read,
        output: &mut dyn Write,
        ctx: &Context,
    ) -> CompressionResult;
}

enum Applicability { Applicable, Inapplicable, DegradedOnly }

struct CompressionResult {
    mode: CompressorMode,              // Full | Degraded | Passthrough
    kept_ranges: Vec<Range<usize>>,    // byte ranges preserved verbatim
    dropped_blocks: Vec<DroppedBlock>, // {range, placeholder: "[...1247 build lines...]"}
    invariants: Vec<InvariantResult>,  // cheap per-run claims (ExitCodeLinePreserved, ErrorLinesVerbatim, JsonValid, ...)
    format_claim: Option<FormatKind>,  // if set, output is parseable as this format
    self_fidelity: f32,                // 0.0-1.0 self-assessed
    raw_pointer: SessionRef,           // session id for raw-output retrieval
}
```

**Hard rule:** output is always a **line-preserving subset or structure-preserving filter** of input. Never a rewrite. Only Layer 4 (small-LLM fallback) may rewrite, and when it does it marks itself `Mode::Degraded`.

### 6.4 Always-Preserved Raw + Session Store

Every invocation persists raw output (post-redaction) to the session store. Compressed output carries a `raw_pointer` field and a visible footer: `[nid: full output preserved; nid show <session-id>]`. Config knob `session.preserve_raw: bool` (default `true`) with `session.retention_days`, `session.max_total_mb`, per-command deny/allow lists for privacy-sensitive commands.

Frequent `nid show` on the same profile is a **bypass signal** (§8.2) that triggers re-synthesis.

### 6.5 Binary/Data Separation

- Layer 3 hand-tuned strategies ship as **declarative TOML DSL** files bundled in the binary's release payload. New strategies add without a binary release.
- Layer 5 learned profiles ship as the same DSL format, synthesized + persisted as content-addressed blobs.
- Both implement the same Compressor trait via the DSL interpreter (§7.1).
- Layer 1 + Layer 2 primitives are native Rust (not DSL) — they're tight, bounded, and universal.
- Layer 4 small-LLM catch-all uses whatever LLM backend is configured; runs only for commands that haven't been profiled and don't match a format.

### 6.6 Command Fingerprinting (Scheme R)

Fingerprint = `binary_name` + canonicalized argv:
- Paths → `<path>` (all variants collapse together)
- Numbers → `<n>`
- URLs → `<scheme>://<host>` (scheme+host preserved; path/query collapsed)
- Quoted-string-positional args → `<str>`
- Flag names preserved verbatim (e.g., `--oneline`, `--json`, `-v`)
- Flag values usually collapsed, except for a small known list of "shape-defining" flag values (`--format=json` vs `--format=yaml` stay distinct)

**Progressive splitting:** start with one profile per fingerprint. When per-sample output-shape variance exceeds threshold, auto-split on the highest-variance differentiator (usually a flag). Split tree recorded in `profiles.parent_fp` + `profiles.split_on_flag`. Self-organizing; no hand-specified per-binary rules.

### 6.7 Profile Storage

- SQLite `profiles` row per profile version; blobs on disk at `~/.local/share/nid/blobs/sha256-<hash>.dsl.zst` (zstd-compressed TOML).
- Content-addressed → natural dedup, natural signing.
- Portable artifact: `.nidprofile` tarball = `{profile.toml, samples.zst, tests.toml, signature.sig}`, ed25519-signed.

## 7. Area 2 — Synthesis Workflow

### 7.1 DSL: What the Model Outputs

A declarative TOML document that nid interprets in pure Rust. No code generation, no Wasm, no eval surface.

**Minimal DSL grammar (v1 schema):**

```toml
# profile.toml
[meta]
fingerprint = "git-log-oneline-n"
version = "1.0.0"
format_claim = "plain"

[[rules]]
kind = "keep_lines"
match = "^[0-9a-f]{7,40} "  # commit-line pattern

[[rules]]
kind = "drop_lines"
match = "^\\s*$"            # blank lines

[[rules]]
kind = "collapse_repeated"
pattern = "^\\.\\.\\."       # collapse runs of ellipsis lines
placeholder = "[... {count} elided ...]"

[[rules]]
kind = "head"
n = 200
after_match = "^commit "    # keep first 200 lines after first commit

[[invariants]]
name = "ExitCodeLinePreserved"
check = "last_line_matches"
pattern = "^exit: \\d+"

[[invariants]]
name = "ErrorLinesVerbatim"
check = "all_matching_preserved"
pattern = "(?i)(error|fatal|panic)"
```

**Allowed rule kinds (v1):** `keep_lines`, `drop_lines`, `collapse_repeated`, `collapse_between`, `head`, `tail`, `head_after`, `tail_before`, `dedup`, `strip_ansi`, `json_path_keep`, `json_path_drop`, `ndjson_filter`, `state_machine` (bounded transitions, bounded depth), `truncate_to`.

**Allowed invariant checks:** `last_line_matches`, `first_line_matches`, `all_matching_preserved`, `count_matches_at_least`, `json_path_exists`, `exit_line_preserved`.

**Forbidden:** network, filesystem, subprocess, eval, backreferences (to prevent catastrophic regex backtracking — Rust `regex` forbids these natively anyway).

### 7.2 Synthesis Wiring

**Single canonical synthesizer + pluggable LLM backend + structural-diff as always-on pre-pass.**

1. **Structural-diff pre-pass (always runs):** given N raw samples, line-align them (`similar` crate), classify each line as `Constant` / `TemplatedConstant` / `Varying`, emit candidate DSL rules, identify invariants (lines present in all samples, error-patterns, exit-code line).
2. **LLM refinement (if backend available):** prompt model with samples + structural-diff candidate + invariants; ask for improved DSL; validate against self-tests.
3. **If LLM unreachable:** structural-diff output is the final profile.

**v1 LLM backend detection order:** `ANTHROPIC_API_KEY` → Ollama (default port) → `claude` CLI subprocess → structural-diff-only floor. (Parent-agent-MCP synthesis is a v1.1 add, pending §9's MCP server.)

### 7.3 Sample Collection & Lock-in

```toml
[synthesis]
samples_to_lock = 5                       # lock at N samples
fast_path_if_zero_variance = true         # lock at N=3 when all samples structurally identical
refine_on_sample_count_doubling = true    # re-refine at 5, 10, 20, 40, ...
min_samples_for_llm_refinement = 3
daily_budget_usd = 0.50
per_profile_refinement_cooldown_hours = 24
```

- N=1–4: no profile. Compression via Tier B fallback (L3 → L2 → L4). Samples accumulate in background.
- N=5 (or N=3 zero-variance): structural-diff synthesizes candidate + (optional) LLM refines → candidate must pass self-tests on all samples → **profile locks in as Layer 5.**
- N>5: keep accumulating; re-refine on (drift / sample-count doubling / `--force`).

### 7.4 Samples-as-Tests

Self-tests are **derived, not model-generated.** Each captured sample + its expected compressed output = one test. Candidate profiles must pass on all prior samples before replacing current. New samples auto-added to the test suite. No separate test-generation prompt.

### 7.5 LLM Refinement Prompt

```
You are refining a compression DSL for the command `<normalized-fingerprint>`.
N raw samples:
--- sample 1 ---
<raw>
--- sample 2 ---
<raw>
...

Current DSL (from structural-diff / prior LLM):
<current DSL as TOML>

Current invariants that MUST be preserved:
- ExitCodeLinePreserved
- ErrorLinesVerbatim
- <others>

Emit ONLY improved DSL as TOML. No prose. Rules:
- Preserve every listed invariant.
- Output must be a structural subset of input (keep/drop/collapse only — NO REWRITES).
- You may add new invariants you observe in the samples.
- Target: minimize compressed size while preserving every ERROR/FATAL line and exit indicator.
```

### 7.6 Failure Fallback

| Failure | Action |
|---|---|
| Invalid DSL parse | Keep current profile; log; exponential backoff |
| Self-tests fail | Keep current; log; backoff |
| LLM backend unreachable | Structural-diff profile stands |
| Repeated failures on same fingerprint | Quarantine refinement for 7 days |

### 7.7 Re-Synthesis Triggers

- Invariant violation detected by replay harness
- Rolling fidelity below threshold
- Schema version bump on `nid update`
- Sample count doubled since last refinement
- User `nid synthesize <cmd> --force`

## 8. Area 3 — Fidelity Measurement

### 8.1 Four-Tier Scoring

| Tier | Cost | Frequency | Detects |
|---|---|---|---|
| T1 — Invariant checks | Free (regex over compressed) | Every run | Profile dropped exit line, error lines, required invariants |
| T2 — Structural checks | Cheap (line-set subset) | Every run | Compressed contains lines not in raw (profile inventing content) |
| T3 — Judge-model comparison | $ (1% sample rate) | Sampled | Subtle semantic loss not caught by invariants |
| T4 — Behavioral signals | Free (observe agent) | Continuous | Agent re-fetched raw, ran duplicate command, hit `NID_RAW` |

### 8.2 Bypass Detection (6 Weighted Signals)

| Signal | Weight | Example |
|---|---|---|
| Raw re-fetch | +0.9 | agent ran `cat <file>` after nid read it |
| Explicit `nid show` / raw-escape | +0.7 | agent fetched raw deliberately |
| **Script-to-disk-then-run** | +0.6 | agent wrote a `.sh` / `.py` then ran it, producing identical output to a compressed command the hook would have rewritten (correlated by recent-file-creation timing + output-content hashing). Documented upstream as a limitation of every PreTool hook (the model can always sidestep a rewrite by creating and running a script); detecting it turns that limitation into a fidelity signal. |
| grep-after-read | +0.5 | agent ripgrepped content that should have been in compressed |
| Near-duplicate re-invocation within 30s | +0.4 | agent re-ran with slight flag change |
| `NID_RAW=1` explicit raw | +0.2 | agent preferred raw for this call |

Aggregate over rolling 100-run window per profile. Weighted-average > `fidelity.bypass_threshold` (default 0.3) → profile flagged for re-synthesis.

**Warmup window:** bypass scoring ignores the first 3 runs after a profile activates (`fidelity.bypass_warmup_runs`, default 3). Freshly locked-in profiles are noisy; the first few agent interactions shouldn't count against them while the agent calibrates.

### 8.3 Exit-Code Correlation

Every session records `(fingerprint, exit_code, raw_bytes, compressed_bytes)`. If a profile with >50 runs shows >2x difference in compressed-to-raw ratio between `exit_code = 0` and `exit_code != 0` (errors staying large while successes compress hard), the profile is underperforming on the failure case — the output agents most need. Triggers re-synthesis with stratified sampling (force samples from both buckets). Sudden exit-code-distribution shift (95% → 40% success) indicates environment change → drift → re-synthesize.

### 8.4 Judge-Model Economics

- Sample rate default **1%**, tunable 0–10% (`fidelity.judge_sample_rate`).
- Judge model = smallest available (Haiku-class default).
- **Targeted sampling:** 80% of budget to profiles with recent bypass signals or invariant failures; 20% random.
- Batch mode: judge queue flushed once daily; only invariant failures run synchronously.
- Shared `synthesis.daily_budget_usd` pool.

### 8.5 Attestation

- Visible footer on every compressed output: `[nid: profile <fp>/v<ver>, fidelity 0.94, mode=Full, raw via nid show <session-id>]`.
- Structured metadata via hook `additionalContext` field (agent-specific structure).
- Exit code = wrapped command's exit code; never masked.
- Hook for v1.1+ MCP `_meta.nid` block.

## 9. Area 4 — MCP Server (Deferred to v1.1)

**Not in v1.** Explicit decision: nid is a CLI tool first; replacing agent-native Read/Grep/Glob is additive future work.

**v1.1+ sketch** (so the architecture doesn't paint itself into a corner):
- MCP server mode: `nid mcp-serve` (stdio) or `nid mcp-serve --http` (streamable-HTTP per 2026 MCP roadmap).
- Tools: `nid_read`, `nid_grep`, `nid_glob`, `nid_ls`, `nid_show`, `nid_session_list`, `request_synthesis`.
- Signature-match each agent's native tools where they exist, so `Read → nid_read` is drop-in.
- `request_synthesis(fingerprint, samples, current_dsl?, invariants)` reverses synthesis: nid asks the parent agent's LLM to produce improved DSL. Free to the user since the agent session is already paying.
- `_meta.nid` structured attestation block on every tool result.
- Double-compression avoidance: hook rewrite is idempotent; MCP path uses direct Rust I/O (no subprocess); `nid_show` always returns raw.

The Compressor trait, profile format, fidelity model, data model, and security model all continue to work unchanged when MCP lands.

## 10. Area 5 — Onboard & Update

### 10.1 `nid onboard` — Five-Phase Flow

1. **Detect.** Platform, shell, installed agents (probed by binary on PATH + config-file presence), LLM backends (`ANTHROPIC_API_KEY`, Ollama daemon, `claude` CLI).
2. **Propose.** Print every file that will be written; show LLM primary + daily budget + raw-preserve + retention; require `[Y/n]` confirm. `--non-interactive` skips confirm and uses flag-supplied defaults.
3. **Apply.** Atomic writes. Backup every modified agent config to `~/.config/nid/onboard.backup.json` for byte-perfect uninstall.
4. **Verify.** Synthetic round-trip per agent — invoke each agent's hook handler with a mock payload, inspect the returned `updatedInput`. Cheap 1-2 token LLM call to confirm credentials. SQLite sanity (create/read/delete test row).
5. **Done.** Terse next-steps hint pointing at `nid gain`.

### 10.2 Modes

| Flag | Behavior |
|---|---|
| *(none)* | Full interactive flow |
| `--non-interactive` | No prompts, uses flag/env defaults |
| `--check` | Re-verify without writes — CI-friendly |
| `--reconfigure` | Re-detect, rewrite hooks, preserve data |
| `--uninstall` | Remove hooks, restore from backup; preserve data |
| `--uninstall --purge` | Remove hooks + data + config |
| `--agents <list>` | Restrict to specific agents |
| `--disable-synthesis` | Structural-diff only; no LLM calls |
| `--budget <usd>` | Override daily LLM budget |
| `--preserve-raw / --no-preserve-raw` | Override session-store default |

### 10.3 `nid update`

- Source: `https://github.com/newtro/nid/releases` (per user requirement).
- Ed25519-signed release artifacts. Release public key embedded in binary, rotatable via update.
- **Pre-pinned rotation record.** The binary ships with not just the current release public key but a signed rotation record: a list of "key-id → successor-key-id + rotation-signature". When a new release signed by an unknown key arrives, nid walks the rotation chain from its embedded current key forward through the rotation record to verify the new key. This lets key rotation happen without any out-of-band trust step and without users needing to re-download a fresh binary. A rotation record that breaks the chain is refused; user gets a clear error + instructions to install a fresh binary directly from the pinned release.
- Atomic replace: download to temp → verify signature → swap binary.
- Post-update: DB schema migration (auto-snapshot before), DSL schema migration (mechanical upgrade where possible; quarantine + re-synthesis otherwise), hook integrity re-check.
- Config preserved. User data never touched.
- Offline / airgapped: `nid update --from <tarball>`.
- Channels: `stable` (default), `beta`, `nightly`.
- `nid version` prints current + latest-known + channel.

## 11. Area 6 — Security Model

### 11.1 Threat Matrix + v1 Mitigations

| Threat | Mitigation |
|---|---|
| Malicious synthesized DSL | DSL grammar has no IO/exec; validator rejects off-grammar; self-tests must pass before activation |
| Malicious imported profile | Ed25519 signature verify; 5-tier trust (§11.2); default refuses unsigned |
| Secrets in raw session store | Pre-persistence redaction; 0700/0600 perms; optional AES-GCM encryption; `--no-preserve-raw`; per-command deny/allow lists |
| Hook tampering | SHA-256 of hook block recorded at install; `nid doctor` + `onboard --check` verify; mismatch warns + shows diff |
| nid binary supply chain | Ed25519-signed releases; key embedded; rotatable |
| Prompt injection via samples | DSL has no net/exec; bad DSL fails tests → quarantined; samples fed as data under strict grammar-constrained prompt |
| Regex DoS | Rust `regex` crate (RE2; no catastrophic backtracking); per-run step/time budget inside interpreter; runaway → quarantine |
| Session-dir perm leak | Enforced 0700/0600 at startup; warn on wider perms (shared `/tmp`-like paths) |

### 11.2 Five Trust Tiers

| Tier | Source | Behavior |
|---|---|---|
| T0 Local-synthesis | Created by this install | Highest trust; never transmitted |
| T1 Bundled | Shipped in nid release | Release-key signed; auto-installed |
| T2 Org-trusted | Signed by `nid trust add` key | Auto-import allowed |
| T3 Registry (v1.1+) | Community profile registry | Explicit `--approve` per profile |
| T4 Unknown signer | Unsigned or untrusted signer | Refused by default; `--allow-unsigned` + confirmation |

### 11.3 Redaction (Pre-Persistence)

Runs **before** raw output enters the session store. Built-in patterns:
- AWS access keys (`AKIA[0-9A-Z]{16}`)
- GitHub tokens (`ghp_…`, `github_pat_…`), GitLab tokens
- Stripe keys (`sk_live_…`, `sk_test_…`)
- JWTs (three-segment base64)
- SSH private keys (PEM `-----BEGIN … PRIVATE KEY-----` blocks)
- Bearer tokens in common headers
- High-entropy heuristic (Shannon entropy + length + alphabet constraints) for generic secret detection

Config:
```toml
[security.redaction]
extra_patterns = [...]
deny_commands = ["env", "printenv", "aws configure get", "kubectl get secret"]
allow_commands = []                  # opt specific commands out of redaction

[session]
preserve_raw = true
retention_days = 14
max_total_mb = 2048
deny_raw_commands = []               # commands whose raw is never persisted
allow_raw_commands = []              # override: force raw persist even if in deny list

[hook]
passthrough_patterns = [
    "^(cd|export|set|unset|alias|source|\\.|eval)\\b",
    "^tee\\b", "^cat\\b.*[<>|]",
]
```

`nid show` always redacts. `nid show --raw-unredacted` requires interactive confirmation + logs the access.

### 11.4 DSL Sandboxing (Spec)

- **Forbidden:** network, filesystem, subprocess, dynamic eval, regex backreferences.
- **Allowed:** regex match/extract, line filters, bounded state machines, JSON path extract (bounded depth), dedup/sort/head/tail/collapse/truncate with bounded memory.
- Per-run budget: max-steps, max-wallclock, max-memory. Exceeding any → runaway abort → profile quarantined.
- Enforced by single DSL validator at: synthesis-output time, import time, update-migration time.

### 11.5 Org Profile Sharing

- `nid profiles export <fp>` → signed `.nidprofile` tarball.
- `nid profiles sign <tarball> --key <path>` → re-sign with an org key.
- `nid trust add <key-path> --label "acme-corp"` → trust that key.
- Imports from trusted keys auto-approve; revoke cascades to all profiles signed by the key.
- `profile_import_events` table is the audit trail.

## 12. Area 7 — Data Model

### 12.1 SQLite Schema

Eleven tables. Full DDL in [Appendix A](#appendix-a--sqlite-ddl-v1).

| Table | Purpose |
|---|---|
| `meta` | Schema version, nid version, install time |
| `profiles` | One row per profile version; dispatch index |
| `blobs` | Content-addressed blob registry (ref-counted) |
| `samples` | Raw samples used for synthesis (redacted) |
| `sessions` | One row per nid invocation |
| `fidelity_events` | Invariant checks, judge scores, bypass signals, exit-skew events |
| `synthesis_events` | Synthesis attempts: outcome, cost, duration |
| `gain_daily` | Rollup of savings by day (redundant, populated from sessions) |
| `trust_keys` | Org profile-sharing trust keyring |
| `profile_import_events` | Audit log for profile imports |
| `agent_registry` | What nid installed where, for uninstall fidelity |

### 12.2 Versioning & Invalidation

- Profile version = SemVer. Major = DSL schema change; Minor = split/merge; Patch = in-place refinement.
- Invalidation = status flip (`active` → `superseded`), never a delete. History preserved for rollback/debug.
- `nid update` runs DB migrations (auto-snapshot before). DSL schema migrations: mechanical upgrade where possible; quarantine + flag for re-synthesis otherwise.
- **Lazy activation:** new profile inserted as `pending`. Promoted to `active` only after self-tests pass. Old `active` flipped to `superseded` atomically in the same transaction.
- `nid profiles rollback <fp>` flips most recent superseded back to active.

### 12.3 Blob GC + Retention

- `blobs.ref_count` maintained via triggers on `profiles`, `samples`, `sessions`.
- `nid gc` purges ref_count=0 blobs + session-raw blobs past `session.retention_days` (default 14) / `session.max_total_mb`.
- **Opportunistic nightly trigger.** The first `nid <cmd>` invocation of each calendar day runs a lightweight GC check (bounded wall-clock: 100ms). If enforcement work is needed, it's kicked off asynchronously so the hot path isn't penalized. Scheduled cron-style triggers aren't used — nid has no daemon — so "nightly" really means "first run after midnight local time." `nid gc` remains the manual path for immediate reclamation.

## 13. Bundled Starter Profiles (E5)

Ship nid with ~30 hand-tuned Layer 3 profiles for the commands agents encounter most often. Shipped as Tier 1 (bundled, release-key signed, auto-installed). Gives users immediate gain on day 1 before synthesis has any samples. Synthesis fills the long tail.

**Starter set (v1):**
- `git` — `status`, `log`, `log --oneline`, `diff`, `show`, `branch -a`
- `cargo` — `build`, `test`, `check`, `clippy`
- `npm` / `pnpm` / `yarn` — `install`, `run build`, `run test`, `audit`
- `pytest` — default + `-v` + `-x`
- `go test` (incl. `-json`)
- `docker` — `ps`, `images`, `logs`, `compose build`, `compose up`
- `kubectl` — `get pods`, `get svc`, `describe pod`, `logs`
- `terraform` — `plan`, `apply`, `validate`
- `az` — `account show`, `resource list`, `webapp log tail`
- `gcloud` — `projects list`, `compute instances list`
- `aws` — `s3 ls`, `ec2 describe-instances`
- `tsc`, `eslint`, `ruff`, `mypy`, `black --check`
- `rg` / `ripgrep`, `jq` large-output cases
- `curl -v`
- `psql` (table-format output)
- `make`
- `mvn package`, `gradle build`
- `gh` / `hub` (issues, pr list)

Profiles live at `~/.local/share/nid/blobs/sha256-<hash>.dsl.zst` after install and are indexed in `profiles` with `provenance='bundled'`. Updates via `nid update` can refresh them; users can `nid profiles revoke <fp>` to force the synthesis path for any command they'd rather learn locally.

## 14. Shadow Mode (E2)

**Purpose:** trust-ramp for skeptical or enterprise users before committing to compression.

**Flow:**
1. `nid shadow enable` — nid's hook starts rewriting commands to `nid --shadow <cmd>`.
2. In shadow mode nid runs the wrapped command, **passes raw output through to the agent unchanged**, but also captures raw + computes counterfactual compressed output + stores both in the session store with `sessions.mode = 'Shadow'`.
3. The agent sees zero behavior change — raw output, as if nid weren't installed.
4. After a representative window (user decides — 1 day, 1 week, 1 month), `nid gain --shadow` reports projected savings: "Over 412 commands across 8 days, compression would have saved 2.3M tokens (~$8.42 at Opus rates)."
5. When convinced: `nid shadow commit` flips the hook to produce compressed output.
6. `nid shadow disable` reverts to raw-only with no counterfactual collection (for users who just want nid off).

**Implementation footprint:** the pipeline logic is identical. The hook's rewrite target toggles between `nid <cmd>` and `nid --shadow <cmd>`. The pass-through-raw logic is a single conditional in the output writer. Estimated ~100 LOC delta.

## 15. Tech Stack (Verified Versions)

> All versions checked via web search 2026-04-21. Where exact current version couldn't be confirmed, the latest confirmed version is listed with a note.

| Purpose | Crate | Version | Notes |
|---|---|---|---|
| Rust edition | — | 2021 (2024 edition once stable) | Single-binary, cross-compile macOS x86_64/aarch64 + Linux x86_64-musl/aarch64-gnu |
| CLI parsing | `clap` | `4.4+` | Derive API |
| Async runtime | `tokio` | `1.35+` | Current-thread runtime for hook hot-path; multi-thread for synthesis |
| Error handling | `thiserror` + `anyhow` | `thiserror 2.x`, `anyhow 1.x` | Library uses thiserror; binary uses anyhow |
| Serde | `serde` + `serde_json` + `toml` | `serde 1.x`, `toml 0.8+` | Profile DSL, config, .nidprofile |
| SQLite | `rusqlite` | `0.32.1+` (bundled feature) | WAL mode; SQLite vanilla. Per-blob AES-GCM handles encryption (see below), not SQLCipher. |
| Regex | `regex` | latest | RE2-style, no catastrophic backtracking |
| File walking / glob | `ignore` + `globwalk` | latest | Respects `.gitignore` where relevant |
| Hashing | `sha2` | `0.10+` | Content addressing, hook integrity |
| Crypto (signatures) | `ed25519-dalek` | `2.2+` | Profile signing, release verification |
| Crypto (encryption, opt-in) | `aes-gcm` | latest | Optional encrypted session store. nid applies encryption at the blob layer (raw-output blobs sealed before persistence) rather than via SQLCipher — keeps SQLite vanilla and isolates the encryption surface to exactly the sensitive blobs. |
| Compression | `zstd` | `0.13+` | Sample + DSL + raw-output compression at rest |
| HTTP client | `reqwest` (with `rustls-tls`) | latest | Anthropic API, GitHub Releases, Ollama |
| Observability | `tracing` + `tracing-subscriber` | latest | Structured logs; opt-in OpenTelemetry export v1.1+ |
| XDG paths | `directories` | latest | Respect XDG on Linux, Application Support on macOS |
| Line alignment for structural-diff | `similar` | latest | Myers diff; fast |
| Process control | `tokio::process` + `nix` | — | SIGTERM trap, child signalling |

**Deliberately NOT in the stack (decisions from brainstorm):**
- ~~`wasmtime` / `wasmer`~~ — no Wasm runtime; DSL interpretation is pure Rust.
- ~~`mcp-sdk`~~ — no MCP server in v1.

## 16. Risks & Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| DSL insufficiently expressive for some commands | Synthesis produces bad profiles; Layer 4 fallback activates forever | DSL expanded in minor releases; quarantine + re-synthesis loop; Layer 4 remains a floor |
| Synthesis LLM unreliable (bad DSL, token spend) | Profile churn, cost overruns | Budget cap; backoff; quarantine; structural-diff as guaranteed floor |
| Hook-ordering conflicts across agents | Commands not rewritten, or rewritten wrong | Parallel hook execution is last-writer-wins on `updatedInput` — if another tool also rewrites the same command slot, order is non-deterministic. `nid doctor` detects co-installed command-rewriting hooks and warns; explicit hook-ordering metadata used where agents support it; idempotent rewrite rule prevents `nid nid <cmd>` when the order race happens to double-write. |
| False-positive secrets redaction hides real output | Agent fails task; fidelity invariant violated | Per-command allow-list; `--no-redaction` per-command; redaction patterns conservative |
| Bypass detection false positives (agent legitimately re-reads) | Unnecessary re-synthesis churn | Weighted + rolling window; tunable thresholds; ignore first 3 runs after profile activation |
| Users lose raw on disk-full | Session store grows unbounded | Retention + size caps enforced; opportunistic GC on first nid invocation of each day (bounded 100ms check); `nid gc` for immediate reclaim |
| Release key compromised | Malicious profiles / binaries | Key rotation via `nid update` from pre-pinned rotation record; `nid doctor --verify-keys` |

## 17. Milestones

Phased v1 implementation. Rough sequencing, not calendar-bound — user will adjust based on actual pace.

**Phase 1 — Foundation (~1.5 weeks)**
- Cargo workspace: `nid-cli`, `nid-core`, `nid-dsl`, `nid-storage`, `nid-hooks`.
- SQLite schema + migration runner.
- Blob store + content-addressed SHA-256 indexing.
- `nid onboard --check` skeleton (detect only, no writes).
- `nid version`, `nid doctor` skeletons.

**Phase 2 — Hot path (~2 weeks)**
- Per-agent hook writers (Claude Code first, then Cursor, Codex, Gemini CLI).
- Layer 1 generic cleanup (streaming).
- Layer 2 format detection (JSON/NDJSON/diff/log/tabular).
- Session store write path + pre-persistence redaction.
- `nid <cmd>` runnable end-to-end with Layer 1 + Layer 2 only.
- SIGTERM trap + flush.

**Phase 3 — DSL + Layer 3 (~2 weeks)**
- DSL grammar + interpreter (pure Rust).
- DSL validator (grammar + invariants).
- Bundle 10 highest-impact hand-tuned Layer 3 profiles (`git status`, `git log`, `git diff`, `cargo build`, `pytest`, `npm install`, `docker ps`, `kubectl get pods`, `rg`, `jq`).
- Compressor trait wiring; dispatch.
- `nid profiles list/inspect`.

**Phase 4 — Learning (~2 weeks)**
- Sample capture + store.
- Structural-diff synthesis (deterministic, no LLM).
- Layer 5 dispatch path.
- Profile lock-in at N=5 (and fast-path N=3 zero variance).
- Self-tests derived from samples.
- `nid synthesize <cmd> --force`.

**Phase 5 — Fidelity (~1.5 weeks)**
- Tier 1 invariant checks on every run.
- Tier 2 structural subset check on every run.
- Bypass signal detection + weighted rolling window.
- Exit-code correlation.
- Re-synthesis triggers wired.
- Attestation footer + hook `additionalContext`.

**Phase 6 — LLM refinement (~1 week)**
- ANTHROPIC_API_KEY, Ollama, claude CLI backends.
- Prompt assembly.
- Async refinement queue.
- Budget enforcement.
- Quarantine path.
- Tier 3 judge-model sampling.

**Phase 7 — Remaining bundled profiles (~1 week)**
- Finish the ~30 starter Layer 3 profiles.
- Import into release artifact.

**Phase 8 — Shadow mode + update + polish (~1.5 weeks)**
- `nid shadow` commands.
- `nid update` + release signing/verification.
- `nid gain` + `nid sessions` + `nid show`.
- Org profile sharing (`nid profiles export/import/sign`, `nid trust`).
- `nid onboard --uninstall` roundtrip testing.

**Phase 9 — Release (~1 week)**
- Homebrew formula, `cargo install`, curl-to-shell installer.
- GitHub Actions release workflow w/ signed artifacts.
- README + docs site.
- `nid bench` scaffolding (full E1 deferred to v1.1).

**Total:** ~13 weeks solo at reasonable pace. Matches brief's 4–6 months (with buffer for synthesis tuning, agent-hook edge cases, and real-world fidelity regressions).

## 18. Open Questions (Deferred)

Things this brainstorm explicitly left for later decision, either because they're non-v1 or because they need usage data before committing:

- **Parent-agent-MCP synthesis path.** Bundled with the v1.1 MCP server. Cheapest synthesis path if parent agent is willing; needs protocol work.
- **`nid bench` task-parity suite (E1).** Scaffolding in v1; full corpus + automation in v1.1.
- **Plugin system for user-written Layer 3 DSL profiles dropped in `~/.config/nid/plugins/`.** Technically works already (DSL is just data); the question is surface area for `nid profiles add-from <path>` UX.
- **Team-level gain dashboards.** Opt-in aggregation endpoint; TBD whether nid ships a reference server or just an export format.
- **OpenTelemetry emission of gain/fidelity metrics.** Straightforward; deferred until there's user demand.
- **Per-project `.nidrc`.** Already sketched in config; exact override semantics need real-world calibration.
- **Context-pressure-aware compression (E3).** Relies on per-agent context-remaining signals that aren't uniformly available; revisit after v1 ships and we can measure.

---

## Appendix A — SQLite DDL (v1)

```sql
-- Schema/migration metadata
CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

-- Profiles: core dispatch table
CREATE TABLE profiles (
  id                  INTEGER PRIMARY KEY,
  fingerprint         TEXT NOT NULL,
  version             TEXT NOT NULL,
  provenance          TEXT NOT NULL,         -- synthesized | hand_tuned | bundled | imported
  synthesis_source    TEXT,                  -- anthropic_api | ollama | claude_cli | structural_diff
  status              TEXT NOT NULL,         -- active | superseded | quarantined | pending
  dsl_blob_sha256     TEXT NOT NULL,
  rubric_blob_sha256  TEXT,
  parent_fp           TEXT,
  split_on_flag       TEXT,
  created_at          INTEGER NOT NULL,
  last_used_at        INTEGER,
  sample_count        INTEGER NOT NULL DEFAULT 0,
  fidelity_rolling    REAL,
  signature           BLOB,
  signer_key_id       TEXT,
  UNIQUE (fingerprint, version)
);
CREATE INDEX idx_profiles_fingerprint_active ON profiles(fingerprint) WHERE status = 'active';
CREATE INDEX idx_profiles_status ON profiles(status);

-- Content-addressed blob registry
CREATE TABLE blobs (
  sha256     TEXT PRIMARY KEY,
  kind       TEXT NOT NULL,          -- dsl | rubric | sample | compressed | raw | signature
  size       INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  ref_count  INTEGER NOT NULL DEFAULT 1
);

-- Samples used for synthesis (raw captured, redacted)
CREATE TABLE samples (
  id                  INTEGER PRIMARY KEY,
  fingerprint         TEXT NOT NULL,
  sample_blob_sha256  TEXT NOT NULL,
  exit_code           INTEGER NOT NULL,
  captured_at         INTEGER NOT NULL,
  shape_class         TEXT,
  FOREIGN KEY (sample_blob_sha256) REFERENCES blobs(sha256)
);
CREATE INDEX idx_samples_fp ON samples(fingerprint);

-- Sessions: per-invocation record
CREATE TABLE sessions (
  id                     TEXT PRIMARY KEY,    -- short random id; used in nid show
  fingerprint            TEXT NOT NULL,
  profile_id             INTEGER,             -- NULL if no profile (fallback path)
  command                TEXT NOT NULL,       -- canonicalized
  argv_raw               TEXT NOT NULL,       -- original argv
  cwd                    TEXT,
  parent_agent           TEXT,                -- claude_code | cursor | codex | ... | unknown
  started_at             INTEGER NOT NULL,
  ended_at               INTEGER,
  exit_code              INTEGER,
  raw_blob_sha256        TEXT,                -- NULL if preserve_raw=false
  compressed_blob_sha256 TEXT,
  raw_bytes              INTEGER,
  compressed_bytes       INTEGER,
  tokens_saved_est       INTEGER,
  model_estimator        TEXT,                -- tokenizer identity
  mode                   TEXT,                -- Full | Degraded | Passthrough | Shadow
  FOREIGN KEY (profile_id) REFERENCES profiles(id)
);
CREATE INDEX idx_sessions_fp_time ON sessions(fingerprint, started_at);
CREATE INDEX idx_sessions_time ON sessions(started_at);

-- Fidelity events: per-run invariants, judge scores, bypass signals, exit-skew
CREATE TABLE fidelity_events (
  id         INTEGER PRIMARY KEY,
  session_id TEXT,
  profile_id INTEGER NOT NULL,
  kind       TEXT NOT NULL,       -- invariant_pass | invariant_fail | structural_pass | structural_fail | judge_score | bypass_signal | exit_code_skew
  signal     TEXT,                -- bypass signal type or invariant name
  score      REAL,
  weight     REAL,
  detail     TEXT,                -- free-form JSON
  at         INTEGER NOT NULL,
  FOREIGN KEY (session_id) REFERENCES sessions(id),
  FOREIGN KEY (profile_id) REFERENCES profiles(id)
);
CREATE INDEX idx_fidelity_profile_time ON fidelity_events(profile_id, at);

-- Synthesis events: every attempt, cost, outcome
CREATE TABLE synthesis_events (
  id             INTEGER PRIMARY KEY,
  fingerprint    TEXT NOT NULL,
  backend        TEXT NOT NULL,   -- structural_diff | anthropic_api | ollama | claude_cli
  outcome        TEXT NOT NULL,   -- success | invalid_dsl | tests_failed | backend_error | quarantined
  new_profile_id INTEGER,
  duration_ms    INTEGER,
  cost_usd_est   REAL,
  error_detail   TEXT,
  at             INTEGER NOT NULL,
  FOREIGN KEY (new_profile_id) REFERENCES profiles(id)
);
CREATE INDEX idx_synthesis_fp_time ON synthesis_events(fingerprint, at);

-- Daily gain rollup (denormalized for fast `nid gain`)
CREATE TABLE gain_daily (
  date              TEXT PRIMARY KEY,  -- YYYY-MM-DD
  runs              INTEGER NOT NULL,
  raw_bytes         INTEGER NOT NULL,
  compressed_bytes  INTEGER NOT NULL,
  tokens_saved_est  INTEGER NOT NULL,
  usd_saved_est     REAL NOT NULL,
  synthesis_cost_usd REAL NOT NULL DEFAULT 0
);

-- Trust keyring
CREATE TABLE trust_keys (
  key_id     TEXT PRIMARY KEY,
  public_key BLOB NOT NULL,
  label      TEXT NOT NULL,
  added_at   INTEGER NOT NULL,
  revoked_at INTEGER
);

-- Profile import audit
CREATE TABLE profile_import_events (
  id            INTEGER PRIMARY KEY,
  profile_id    INTEGER,
  source_uri    TEXT,
  signer_key_id TEXT,
  outcome       TEXT NOT NULL,
  at            INTEGER NOT NULL,
  FOREIGN KEY (profile_id) REFERENCES profiles(id),
  FOREIGN KEY (signer_key_id) REFERENCES trust_keys(key_id)
);

-- Agent registry: for uninstall fidelity
CREATE TABLE agent_registry (
  agent            TEXT PRIMARY KEY,
  hook_path        TEXT NOT NULL,
  hook_sha256      TEXT NOT NULL,
  installed_at     INTEGER NOT NULL,
  original_backup  TEXT
);
```

## Appendix B — DSL Grammar (v1)

**Top-level document structure (TOML):**

```
profile := meta + rules* + invariants* + self_tests*

meta:
  fingerprint   : string (required)
  version       : semver string (required)
  format_claim  : enum { plain | json | ndjson | diff | log | tabular | stack_trace }
  schema        : "1.0" (required, matched to interpreter version)

rules (array, ordered; applied in order):
  kind          : enum {
                    keep_lines, drop_lines, collapse_repeated, collapse_between,
                    head, tail, head_after, tail_before,
                    dedup, strip_ansi,
                    json_path_keep, json_path_drop, ndjson_filter,
                    state_machine, truncate_to
                  }
  — each kind has its own required/optional fields (e.g., keep_lines takes `match: regex`)

invariants (array):
  name  : string (required)
  check : enum {
            last_line_matches, first_line_matches,
            all_matching_preserved, count_matches_at_least,
            json_path_exists, exit_line_preserved
          }
  pattern | path | count : check-specific parameters

self_tests (array; auto-populated by sample capture):
  sample_sha256 : string (pointer to blob)
  expected_compressed_sha256 : string
  expected_invariants : array of invariant names that must pass
```

**Forbidden (enforced by validator):**
- Any I/O primitive
- Any subprocess primitive
- Dynamic eval
- Regex with backreferences (the `regex` crate already rejects these)
- Unbounded recursion/iteration in state_machine

**Per-run execution budget (enforced by interpreter):**
- `max_steps: 10_000_000` (instruction budget)
- `max_wallclock_ms: 2000`
- `max_peak_memory_mb: 64`

## Appendix C — Example Profile: `git status`

```toml
[meta]
fingerprint = "git-status"
version = "1.0.0"
schema = "1.0"
format_claim = "plain"

# git status output has three sections:
# 1. "On branch X" / "Your branch is..." header
# 2. "Changes to be committed", "Changes not staged", "Untracked files" sections
# 3. Final "nothing to commit" or blank

[[rules]]
kind = "strip_ansi"

[[rules]]
kind = "keep_lines"
match = "^(On branch |Your branch|HEAD detached|Changes |Untracked |nothing to )"

[[rules]]
kind = "keep_lines"
match = "^\\s+(new file|modified|deleted|renamed|typechange):"

[[rules]]
kind = "keep_lines"
match = "^\\s+[a-zA-Z0-9_.-]+\\s*$"  # bare file names under untracked

[[rules]]
kind = "drop_lines"
match = "^\\s*\\(.*\\)\\s*$"  # hints like "(use 'git add <file>'...)"

[[rules]]
kind = "dedup"

[[invariants]]
name = "BranchLinePreserved"
check = "first_line_matches"
pattern = "^(On branch |HEAD detached|Your branch)"

[[invariants]]
name = "NothingOrChangesLinePreserved"
check = "count_matches_at_least"
pattern = "^(nothing to |Changes )"
count = 1
```

Compressed output of `git status` on a repo with 2 modified + 1 untracked under this profile looks like:

```
On branch main
Your branch is up to date with 'origin/main'.
Changes not staged for commit:
        modified:   src/foo.rs
        modified:   src/bar.rs
Untracked files:
        notes.md
no changes added to commit
[nid: profile git-status/v1.0.0, fidelity 0.98, mode=Full, raw via nid show sess_abc123]
```

Ratio vs. raw: typically 0.15–0.25. Task-fidelity: excellent — every piece of information an agent needs for "which files are dirty?" is preserved verbatim.

---

*End of plan.*
