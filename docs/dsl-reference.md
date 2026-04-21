# nid DSL reference (schema 1.0)

The nid compression DSL is TOML. It is validated against a fixed grammar
(plan §11.4) and interpreted in pure Rust — no code generation, no Wasm,
no eval. This reference lists every rule kind and every invariant check,
with an example for each.

A profile has four top-level sections:

```toml
[meta]            # required: fingerprint, version, schema; optional: format_claim, description
[[rules]]         # ordered; applied top-to-bottom
[[invariants]]    # run on compressed output at tier 1 fidelity
[[self_tests]]    # auto-populated by sample capture (you usually don't write these)
```

All regex patterns are compiled with the Rust `regex` crate (RE2 — no
catastrophic backtracking, no backreferences, no look-around).

## `[meta]`

| Key | Type | Required | Notes |
|---|---|---|---|
| `fingerprint` | string | ✓ | The Scheme R fingerprint for the command (see plan §6.6). |
| `version` | semver string | ✓ | Bumped per status-flip (patch: in-place; minor: split/merge; major: schema). |
| `schema` | `"1.0"` | ✓ | DSL schema version. v0.1.0 only speaks `"1.0"`. |
| `format_claim` | enum | optional | One of `plain`, `json`, `ndjson`, `diff`, `log`, `tabular`, `stack_trace`. Advises downstream fidelity checks. |
| `description` | string | optional | Human-readable prose. |

## Rules

Applied in declared order. Each rule runs over the current line set and
produces a new line set.

### `keep_lines`
Keep only input lines that match `match`.

```toml
[[rules]]
kind = "keep_lines"
match = '^(On branch |Your branch|Changes |Untracked |nothing to )'
```

### `drop_lines`
Drop input lines that match `match`.

```toml
[[rules]]
kind = "drop_lines"
match = '^\s*\(.*\)\s*$'  # drop lines that are only a parenthesized hint
```

### `collapse_repeated`
Collapse a run of `min+` consecutive lines matching `pattern` into one
`placeholder` line. `{count}` in the placeholder is substituted with the
collapsed run length.

```toml
[[rules]]
kind = "collapse_repeated"
pattern = "^\\.\\.\\."
placeholder = "[... {count} elided ...]"
min = 3
```

### `collapse_between`
Collapse everything between two fence patterns into a placeholder.

```toml
[[rules]]
kind = "collapse_between"
begin = "^BEGIN OUTPUT"
end = "^END OUTPUT"
placeholder = "[...omitted {count} lines...]"
```

### `head`
Keep the first `n` lines only.

```toml
[[rules]]
kind = "head"
n = 200
```

### `tail`
Keep the last `n` lines only.

```toml
[[rules]]
kind = "tail"
n = 50
```

### `head_after`
Keep the first `n` lines that follow the first line matching `after_match`.

```toml
[[rules]]
kind = "head_after"
n = 50
after_match = "^=== Test output ==="
```

### `tail_before`
Keep the last `n` lines immediately before the first line matching
`before_match`.

```toml
[[rules]]
kind = "tail_before"
n = 20
before_match = "^=== SUMMARY ==="
```

### `dedup`
Deduplicate adjacent identical lines.

```toml
[[rules]]
kind = "dedup"
```

### `strip_ansi`
Remove ANSI color + cursor control sequences.

```toml
[[rules]]
kind = "strip_ansi"
```

### `json_path_keep`
For a single-document JSON input, keep only the values at these bounded
paths. Paths are a small subset: `$`, `.key`, `[n]`. No wildcards, no
filters, no `..`.

```toml
[[rules]]
kind = "json_path_keep"
paths = ["$.items[0].id", "$.total"]
```

### `json_path_drop`
Drop values at the given paths.

```toml
[[rules]]
kind = "json_path_drop"
paths = ["$.verbose_trace", "$.debug"]
```

### `ndjson_filter`
For NDJSON input, keep only objects whose `field` has one of the
`keep_values`.

```toml
[[rules]]
kind = "ndjson_filter"
field = "level"
keep_values = ["error", "warn"]
```

### `state_machine`
Bounded sectional filter: each state declares an `enter` regex (the line
that transitions INTO the state) and `keep` / `drop` regex lists (what to
do with lines seen while in that state). Duplicate state names are
rejected.

```toml
[[rules]]
kind = "state_machine"
[[rules.states]]
name = "header"
enter = '^On branch '
keep  = ['^On branch ', '^Your branch']

[[rules.states]]
name = "changes"
enter = '^Changes '
keep  = ['^Changes ', '^\s+(modified|new file|deleted):']
```

### `truncate_to`
Cap total output to at most `bytes` bytes; append a single
`[... truncated to N bytes ...]` placeholder when the cap fires.

```toml
[[rules]]
kind = "truncate_to"
bytes = 65536
```

## Invariants

Tier 1 fidelity checks. Cheap regex/JSON-path probes that run after
compression. If any invariant fails, the profile is flagged for re-
synthesis; the output is still returned (invariants are observational in
the hot path, not gating).

### `last_line_matches`
```toml
[[invariants]]
name = "ExitCodeLinePreserved"
check = "last_line_matches"
pattern = '^exit: \d+'
```

### `first_line_matches`
```toml
[[invariants]]
name = "BranchLinePreserved"
check = "first_line_matches"
pattern = '^On branch '
```

### `all_matching_preserved`
Every line in *raw* matching `pattern` must also appear in *compressed*.

```toml
[[invariants]]
name = "ErrorLinesVerbatim"
check = "all_matching_preserved"
pattern = '(?i)(error|fatal|panic)'
```

### `count_matches_at_least`
Compressed must contain at least `count` lines matching `pattern`.

```toml
[[invariants]]
name = "AtLeastOneSummary"
check = "count_matches_at_least"
pattern = '^=+ [0-9]+ '
count = 1
```

### `json_path_exists`
For a JSON-claim output, check that the path resolves.

```toml
[[invariants]]
name = "TotalFieldPresent"
check = "json_path_exists"
path = "$.total"
```

### `exit_line_preserved`
If raw contains an "exit"-like indicator (matching
`(?i)^(exit[: ]|process exited|command failed|error code)`), compressed
must also contain it.

```toml
[[invariants]]
name = "ExitPreserved"
check = "exit_line_preserved"
```

## Per-run execution budget

The interpreter enforces:

| Budget | Value |
|---|---|
| max steps | 10,000,000 |
| max wall-clock | 2000 ms |
| max peak memory | 64 MB |

Exceeding any budget aborts the run and quarantines the profile.

## Forbidden (enforced by validator)

- Any I/O primitive
- Any subprocess primitive
- Dynamic eval
- Regex backreferences (the `regex` crate already refuses them)
- Unbounded recursion in `state_machine`

These cannot be expressed in the grammar; the validator exists to confirm
what the grammar already guarantees.
