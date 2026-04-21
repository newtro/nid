//! Pure-Rust DSL interpreter.
//!
//! Single entry point `apply_rules(input, &rules) -> CompressedOutput`, which
//! a Compressor-trait impl wraps to satisfy plan §6.3. Rules are applied
//! in declared order; each transforms a Vec<Line>, where a Line is either a
//! verbatim slice of input or a placeholder string.
//!
//! This is NOT a regex-heavy hot path: patterns are compiled once and reused.
//! All regex execution runs against the Rust `regex` crate (RE2, no
//! catastrophic backtracking).

use crate::ast::{Rule, RuleKind, StateDef};
use regex::Regex;
use std::sync::OnceLock;

/// A single line in the evolving compressed output.
#[derive(Debug, Clone)]
pub enum Line {
    /// Preserved verbatim from input at this byte offset / length.
    Verbatim { text: String },
    /// An elision placeholder emitted by a rule.
    Placeholder { text: String },
}

impl Line {
    pub fn as_str(&self) -> &str {
        match self {
            Line::Verbatim { text } | Line::Placeholder { text } => text,
        }
    }
    fn is_verbatim(&self) -> bool {
        matches!(self, Line::Verbatim { .. })
    }
}

#[derive(Debug, Clone)]
pub struct CompressedOutput {
    pub lines: Vec<Line>,
    pub bytes_in: usize,
    pub bytes_out: usize,
}

impl CompressedOutput {
    pub fn to_string(&self) -> String {
        let mut out = String::with_capacity(self.bytes_out);
        for l in &self.lines {
            out.push_str(l.as_str());
            out.push('\n');
        }
        // Drop final newline if input didn't have one — harmless in practice; we always add \n between lines.
        out
    }
}

/// Run the rule list against `input`. Returns the final compressed output.
pub fn apply_rules(input: &str, rules: &[Rule]) -> CompressedOutput {
    let bytes_in = input.len();
    // Initial line split: preserve each line as Verbatim.
    let mut lines: Vec<Line> = input
        .split_inclusive('\n')
        .map(|l| Line::Verbatim {
            text: l.trim_end_matches('\n').to_string(),
        })
        .collect();

    // If input had a trailing newline, split_inclusive leaves the last line empty.
    // We keep empty lines because rules like drop_lines may specifically target them.
    if let Some(last) = lines.last() {
        if last.as_str().is_empty() && !input.ends_with('\n') {
            lines.pop();
        }
    }

    for rule in rules {
        lines = apply_one(lines, &rule.kind);
    }

    let bytes_out: usize = lines.iter().map(|l| l.as_str().len() + 1).sum();
    CompressedOutput {
        lines,
        bytes_in,
        bytes_out,
    }
}

fn apply_one(lines: Vec<Line>, kind: &RuleKind) -> Vec<Line> {
    match kind {
        RuleKind::KeepLines { match_ } => {
            let re = compile(match_);
            lines
                .into_iter()
                .filter(|l| !l.is_verbatim() || re.is_match(l.as_str()))
                .collect()
        }
        RuleKind::DropLines { match_ } => {
            let re = compile(match_);
            lines
                .into_iter()
                .filter(|l| !(l.is_verbatim() && re.is_match(l.as_str())))
                .collect()
        }
        RuleKind::CollapseRepeated {
            pattern,
            placeholder,
            min,
        } => collapse_repeated(lines, pattern, placeholder, *min),
        RuleKind::CollapseBetween {
            begin,
            end,
            placeholder,
        } => collapse_between(lines, begin, end, placeholder),
        RuleKind::Head { n } => lines.into_iter().take(*n).collect(),
        RuleKind::Tail { n } => {
            let n = *n;
            let len = lines.len();
            lines.into_iter().skip(len.saturating_sub(n)).collect()
        }
        RuleKind::HeadAfter { n, after_match } => head_after(lines, *n, after_match),
        RuleKind::TailBefore { n, before_match } => tail_before(lines, *n, before_match),
        RuleKind::Dedup => dedup_adjacent(lines),
        RuleKind::StripAnsi => strip_ansi(lines),
        RuleKind::JsonPathKeep { paths } => json_path_keep(lines, paths),
        RuleKind::JsonPathDrop { paths } => json_path_drop(lines, paths),
        RuleKind::NdjsonFilter { field, keep_values } => ndjson_filter(lines, field, keep_values),
        RuleKind::StateMachine { states } => state_machine(lines, states),
        RuleKind::TruncateTo { bytes } => truncate_to(lines, *bytes),
    }
}

fn compile(src: &str) -> Regex {
    // Validator guarantees this compiles. If somehow we got here on an invalid
    // regex, fail closed with a match-nothing pattern.
    Regex::new(src).unwrap_or_else(|_| Regex::new("a\\A").unwrap())
}

fn dedup_adjacent(lines: Vec<Line>) -> Vec<Line> {
    let mut out = Vec::with_capacity(lines.len());
    let mut last: Option<String> = None;
    for l in lines {
        let s = l.as_str().to_string();
        if last.as_deref() == Some(s.as_str()) {
            continue;
        }
        last = Some(s);
        out.push(l);
    }
    out
}

fn strip_ansi(lines: Vec<Line>) -> Vec<Line> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap());
    lines
        .into_iter()
        .map(|l| match l {
            Line::Verbatim { text } => Line::Verbatim {
                text: re.replace_all(&text, "").into_owned(),
            },
            other => other,
        })
        .collect()
}

fn collapse_repeated(
    lines: Vec<Line>,
    pattern: &str,
    placeholder: &str,
    min: usize,
) -> Vec<Line> {
    let re = compile(pattern);
    let mut out = Vec::with_capacity(lines.len());
    let mut run_start: Option<usize> = None;

    let flush = |run_start: Option<usize>, i: usize, out: &mut Vec<Line>, src: &[Line]| {
        if let Some(start) = run_start {
            let count = i - start;
            if count >= min {
                out.push(Line::Placeholder {
                    text: placeholder.replace("{count}", &count.to_string()),
                });
            } else {
                for l in &src[start..i] {
                    out.push(l.clone());
                }
            }
        }
    };

    for (i, l) in lines.iter().enumerate() {
        let matches = l.is_verbatim() && re.is_match(l.as_str());
        if matches {
            if run_start.is_none() {
                run_start = Some(i);
            }
        } else {
            flush(run_start, i, &mut out, &lines);
            run_start = None;
            out.push(l.clone());
        }
    }
    flush(run_start, lines.len(), &mut out, &lines);
    out
}

fn collapse_between(
    lines: Vec<Line>,
    begin: &str,
    end: &str,
    placeholder: &str,
) -> Vec<Line> {
    let bre = compile(begin);
    let ere = compile(end);
    let mut out = Vec::with_capacity(lines.len());
    let mut inside = false;
    let mut inside_count = 0usize;
    for l in lines {
        if !inside {
            if l.is_verbatim() && bre.is_match(l.as_str()) {
                inside = true;
                inside_count = 0;
                out.push(l);
            } else {
                out.push(l);
            }
        } else {
            if l.is_verbatim() && ere.is_match(l.as_str()) {
                out.push(Line::Placeholder {
                    text: placeholder.replace("{count}", &inside_count.to_string()),
                });
                out.push(l);
                inside = false;
            } else {
                inside_count += 1;
                // skip (collapsed)
            }
        }
    }
    // If we ended inside, still emit the placeholder so output reflects the collapse.
    if inside && inside_count > 0 {
        out.push(Line::Placeholder {
            text: placeholder.replace("{count}", &inside_count.to_string()),
        });
    }
    out
}

fn head_after(lines: Vec<Line>, n: usize, after_match: &str) -> Vec<Line> {
    let re = compile(after_match);
    let mut out = Vec::with_capacity(lines.len());
    let mut found = false;
    let mut kept = 0usize;
    for l in lines {
        if !found {
            out.push(l.clone());
            if l.is_verbatim() && re.is_match(l.as_str()) {
                found = true;
            }
        } else if kept < n {
            out.push(l);
            kept += 1;
        } else {
            break;
        }
    }
    out
}

fn tail_before(lines: Vec<Line>, n: usize, before_match: &str) -> Vec<Line> {
    let re = compile(before_match);
    // Find first match; keep the `n` lines immediately before it.
    let mut idx = None;
    for (i, l) in lines.iter().enumerate() {
        if l.is_verbatim() && re.is_match(l.as_str()) {
            idx = Some(i);
            break;
        }
    }
    let Some(i) = idx else { return lines };
    let start = i.saturating_sub(n);
    let mut out = Vec::with_capacity(n + (lines.len() - i));
    for l in lines.iter().skip(start) {
        out.push(l.clone());
    }
    out
}

fn truncate_to(lines: Vec<Line>, bytes: usize) -> Vec<Line> {
    let mut out = Vec::new();
    let mut used = 0usize;
    for l in lines {
        let w = l.as_str().len() + 1;
        if used + w > bytes {
            out.push(Line::Placeholder {
                text: format!("[... truncated to {bytes} bytes ...]"),
            });
            break;
        }
        used += w;
        out.push(l);
    }
    out
}

fn json_path_keep(lines: Vec<Line>, paths: &[String]) -> Vec<Line> {
    // Treat the entire input as one JSON document. If it doesn't parse, leave
    // the input alone — degrading gracefully.
    let concat: String = lines
        .iter()
        .map(|l| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&concat) else {
        return lines;
    };
    let mut kept = serde_json::Map::new();
    for p in paths {
        if let Some(v) = json_walk(&value, p) {
            kept.insert(p.clone(), v.clone());
        }
    }
    let out = serde_json::Value::Object(kept);
    let text = serde_json::to_string_pretty(&out).unwrap_or(concat);
    text.lines()
        .map(|s| Line::Verbatim {
            text: s.to_string(),
        })
        .collect()
}

fn json_path_drop(lines: Vec<Line>, paths: &[String]) -> Vec<Line> {
    let concat: String = lines
        .iter()
        .map(|l| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&concat) else {
        return lines;
    };
    for p in paths {
        json_drop(&mut value, p);
    }
    let text = serde_json::to_string_pretty(&value).unwrap_or(concat);
    text.lines()
        .map(|s| Line::Verbatim {
            text: s.to_string(),
        })
        .collect()
}

fn ndjson_filter(lines: Vec<Line>, field: &str, keep_values: &[String]) -> Vec<Line> {
    lines
        .into_iter()
        .filter(|l| {
            if !l.is_verbatim() {
                return true;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(l.as_str()) else {
                return false;
            };
            match v.get(field).and_then(|f| f.as_str()) {
                Some(s) => keep_values.iter().any(|k| k == s),
                None => false,
            }
        })
        .collect()
}

fn state_machine(lines: Vec<Line>, states: &[StateDef]) -> Vec<Line> {
    // Compile all regexes up-front.
    struct Compiled {
        name: String,
        enter: Regex,
        keep: Vec<Regex>,
        drop: Vec<Regex>,
    }
    let compiled: Vec<Compiled> = states
        .iter()
        .map(|s| Compiled {
            name: s.name.clone(),
            enter: compile(&s.enter),
            keep: s.keep.iter().map(|r| compile(r)).collect(),
            drop: s.drop.iter().map(|r| compile(r)).collect(),
        })
        .collect();

    let mut active: Option<usize> = None;
    let mut out = Vec::with_capacity(lines.len());

    for l in lines {
        if !l.is_verbatim() {
            out.push(l);
            continue;
        }
        // Try to transition.
        for (i, c) in compiled.iter().enumerate() {
            if c.enter.is_match(l.as_str()) {
                active = Some(i);
                break;
            }
        }
        let keep = match active {
            None => true, // no active state — pass through
            Some(i) => {
                let c = &compiled[i];
                if c.drop.iter().any(|re| re.is_match(l.as_str())) {
                    false
                } else if !c.keep.is_empty() {
                    c.keep.iter().any(|re| re.is_match(l.as_str()))
                } else {
                    true
                }
            }
        };
        if keep {
            out.push(l);
        }
    }
    // silence unused warnings from `name`
    let _ = compiled.iter().map(|c| &c.name).count();
    out
}

/// Minimal JSONPath walk: `$`, `.key`, `[n]`.
fn json_walk<'a>(v: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut cur = v;
    let rest = path.strip_prefix('$').unwrap_or(path);
    let mut it = rest.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '.' => {
                let mut key = String::new();
                while let Some(&p) = it.peek() {
                    if p == '.' || p == '[' {
                        break;
                    }
                    key.push(p);
                    it.next();
                }
                cur = cur.get(&key)?;
            }
            '[' => {
                let mut num = String::new();
                while let Some(&p) = it.peek() {
                    if p == ']' {
                        it.next();
                        break;
                    }
                    num.push(p);
                    it.next();
                }
                let idx: usize = num.parse().ok()?;
                cur = cur.get(idx)?;
            }
            _ => return None,
        }
    }
    Some(cur)
}

fn json_drop(v: &mut serde_json::Value, path: &str) {
    // Walk parents to the last segment, then remove.
    let Some((parent_path, last)) = split_last_segment(path) else {
        return;
    };
    if parent_path == "$" {
        if let Some(obj) = v.as_object_mut() {
            obj.remove(&last);
        }
    } else {
        let parent = json_walk_mut(v, &parent_path);
        if let Some(p) = parent {
            if let Some(obj) = p.as_object_mut() {
                obj.remove(&last);
            }
        }
    }
}

fn split_last_segment(path: &str) -> Option<(String, String)> {
    let rest = path.strip_prefix('$').unwrap_or(path);
    let idx = rest.rfind('.')?;
    let parent = format!("${}", &rest[..idx]);
    let last = rest[idx + 1..].to_string();
    Some((parent, last))
}

fn json_walk_mut<'a>(v: &'a mut serde_json::Value, path: &str) -> Option<&'a mut serde_json::Value> {
    let mut cur: &mut serde_json::Value = v;
    let rest = path.strip_prefix('$').unwrap_or(path);
    let mut parts = vec![];
    let mut key = String::new();
    for c in rest.chars() {
        if c == '.' {
            if !key.is_empty() {
                parts.push(key.clone());
                key.clear();
            }
        } else if c == '[' || c == ']' {
            if !key.is_empty() {
                parts.push(key.clone());
                key.clear();
            }
        } else {
            key.push(c);
        }
    }
    if !key.is_empty() {
        parts.push(key);
    }
    for p in parts {
        if let Ok(idx) = p.parse::<usize>() {
            cur = cur.get_mut(idx)?;
        } else {
            cur = cur.get_mut(&p)?;
        }
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Rule, RuleKind};

    fn r(kind: RuleKind) -> Rule {
        Rule { kind }
    }

    #[test]
    fn keep_lines_basic() {
        let input = "A\nB\nC\n";
        let out = apply_rules(
            input,
            &[r(RuleKind::KeepLines {
                match_: "^B$".into(),
            })],
        );
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].as_str(), "B");
    }

    #[test]
    fn drop_lines_basic() {
        let input = "A\nB\nC\n";
        let out = apply_rules(
            input,
            &[r(RuleKind::DropLines {
                match_: "^B$".into(),
            })],
        );
        assert_eq!(
            out.lines.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
            vec!["A", "C"]
        );
    }

    #[test]
    fn collapse_repeated_collapses_runs() {
        let input = "ok\n...\n...\n...\ndone\n";
        let out = apply_rules(
            input,
            &[r(RuleKind::CollapseRepeated {
                pattern: r"^\.\.\.$".into(),
                placeholder: "[... {count} elided ...]".into(),
                min: 3,
            })],
        );
        let strs: Vec<&str> = out.lines.iter().map(|l| l.as_str()).collect();
        assert_eq!(strs, vec!["ok", "[... 3 elided ...]", "done"]);
    }

    #[test]
    fn collapse_repeated_leaves_short_runs() {
        let input = "ok\n...\n...\ndone\n";
        let out = apply_rules(
            input,
            &[r(RuleKind::CollapseRepeated {
                pattern: r"^\.\.\.$".into(),
                placeholder: "[...]".into(),
                min: 3,
            })],
        );
        let strs: Vec<&str> = out.lines.iter().map(|l| l.as_str()).collect();
        assert_eq!(strs, vec!["ok", "...", "...", "done"]);
    }

    #[test]
    fn head_and_tail() {
        let input = "1\n2\n3\n4\n5\n";
        let out = apply_rules(input, &[r(RuleKind::Head { n: 2 })]);
        assert_eq!(
            out.lines.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
            vec!["1", "2"]
        );
        let out = apply_rules(input, &[r(RuleKind::Tail { n: 2 })]);
        assert_eq!(
            out.lines.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
            vec!["4", "5"]
        );
    }

    #[test]
    fn dedup_adjacent_collapses_duplicates() {
        let input = "a\na\nb\nb\nb\nc\n";
        let out = apply_rules(input, &[r(RuleKind::Dedup)]);
        assert_eq!(
            out.lines.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn strip_ansi_removes_codes() {
        let input = "\x1b[31mred\x1b[0m text\n";
        let out = apply_rules(input, &[r(RuleKind::StripAnsi)]);
        assert_eq!(out.lines[0].as_str(), "red text");
    }

    #[test]
    fn truncate_to_caps_output() {
        let input = (0..100)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = apply_rules(&input, &[r(RuleKind::TruncateTo { bytes: 30 })]);
        let total: usize = out.lines.iter().map(|l| l.as_str().len() + 1).sum();
        assert!(total <= 30 + 64); // allow the placeholder line to exceed slightly
        assert!(out
            .lines
            .last()
            .map(|l| l.as_str().starts_with("[... truncated"))
            .unwrap_or(false));
    }

    #[test]
    fn head_after_keeps_first_n_after_anchor() {
        let input = "preamble\nstart\n1\n2\n3\n4\n";
        let out = apply_rules(
            &input,
            &[r(RuleKind::HeadAfter {
                n: 2,
                after_match: "^start$".into(),
            })],
        );
        let strs: Vec<&str> = out.lines.iter().map(|l| l.as_str()).collect();
        assert_eq!(strs, vec!["preamble", "start", "1", "2"]);
    }

    #[test]
    fn state_machine_filters_per_state() {
        let input = "On branch main\nChanges not staged for commit:\n\tmodified:   a\n\tnot-modified junk\nUntracked files:\n\tfoo\n";
        let out = apply_rules(
            input,
            &[r(RuleKind::StateMachine {
                states: vec![
                    StateDef {
                        name: "header".into(),
                        enter: r"^On branch".into(),
                        keep: vec![r"^On branch".into()],
                        drop: vec![],
                    },
                    StateDef {
                        name: "changes".into(),
                        enter: r"^Changes ".into(),
                        keep: vec![r"^Changes ".into(), r"^\s+(modified|new file|deleted):".into()],
                        drop: vec![],
                    },
                    StateDef {
                        name: "untracked".into(),
                        enter: r"^Untracked ".into(),
                        keep: vec![r"^Untracked ".into(), r"^\s+\S".into()],
                        drop: vec![],
                    },
                ],
            })],
        );
        let strs: Vec<&str> = out.lines.iter().map(|l| l.as_str()).collect();
        assert!(strs.contains(&"On branch main"));
        assert!(strs.contains(&"Changes not staged for commit:"));
        assert!(strs.iter().any(|s| s.contains("modified:")));
        assert!(!strs.iter().any(|s| s.contains("not-modified junk")));
    }

    #[test]
    fn ndjson_filter_keeps_matching() {
        let input = "{\"level\":\"info\",\"msg\":\"a\"}\n{\"level\":\"error\",\"msg\":\"b\"}\n";
        let out = apply_rules(
            input,
            &[r(RuleKind::NdjsonFilter {
                field: "level".into(),
                keep_values: vec!["error".into()],
            })],
        );
        assert_eq!(out.lines.len(), 1);
        assert!(out.lines[0].as_str().contains("\"b\""));
    }
}
