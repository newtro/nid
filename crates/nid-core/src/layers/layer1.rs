//! Layer 1 — generic cleanup (plan §6.1).
//!
//! Always runs, unconditionally, free. Streaming line-by-line.
//! - dedup adjacent identical lines
//! - strip ANSI escapes
//! - strip carriage returns
//! - optional head/tail truncation envelope

use crate::compressor::{Applicability, CompressionResult, Compressor, CompressorMode, FormatKind};
use crate::context::Context;
use crate::session::SessionRef;
use regex::Regex;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::OnceLock;

/// Envelope limits for Layer 1. `0` means "no truncation".
#[derive(Debug, Clone, Copy)]
pub struct Layer1Options {
    pub head: usize,
    pub tail: usize,
    pub dedup: bool,
    pub strip_ansi: bool,
    pub strip_cr: bool,
}

impl Default for Layer1Options {
    fn default() -> Self {
        Self {
            head: 0,
            tail: 0,
            dedup: true,
            strip_ansi: true,
            strip_cr: true,
        }
    }
}

pub struct Layer1Generic {
    opts: Layer1Options,
}

impl Layer1Generic {
    pub fn new(opts: Layer1Options) -> Self {
        Self { opts }
    }
}

impl Default for Layer1Generic {
    fn default() -> Self {
        Self::new(Layer1Options::default())
    }
}

fn ansi_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap())
}

impl Compressor for Layer1Generic {
    fn name(&self) -> &str {
        "layer1-generic"
    }

    fn probe(&self, _preview: &[u8], _ctx: &Context) -> Applicability {
        Applicability::Applicable
    }

    fn compress(
        &self,
        input: &mut dyn Read,
        output: &mut dyn Write,
        _ctx: &Context,
    ) -> anyhow::Result<CompressionResult> {
        let reader = BufReader::new(input);
        let mut last: Option<String> = None;
        let mut head_kept = 0usize;
        let mut bytes_in = 0usize;
        let mut bytes_out = 0usize;
        let mut tail_buf: std::collections::VecDeque<String> = std::collections::VecDeque::new();

        for line in reader.lines() {
            let mut line = line?;
            bytes_in += line.len() + 1;

            if self.opts.strip_cr {
                line = line.trim_end_matches('\r').to_string();
            }
            if self.opts.strip_ansi {
                line = ansi_re().replace_all(&line, "").into_owned();
            }
            if self.opts.dedup {
                if last.as_deref() == Some(line.as_str()) {
                    continue;
                }
                last = Some(line.clone());
            }

            // Apply head/tail envelope.
            if self.opts.head > 0 && head_kept < self.opts.head {
                writeln!(output, "{line}")?;
                bytes_out += line.len() + 1;
                head_kept += 1;
                continue;
            }
            if self.opts.tail > 0 {
                if tail_buf.len() == self.opts.tail {
                    tail_buf.pop_front();
                }
                tail_buf.push_back(line);
                continue;
            }
            if self.opts.head == 0 && self.opts.tail == 0 {
                writeln!(output, "{line}")?;
                bytes_out += line.len() + 1;
            }
        }

        // Flush tail buffer.
        if !tail_buf.is_empty() {
            if self.opts.head > 0 {
                writeln!(
                    output,
                    "--- [nid: middle elided, showing first {} and last {} lines] ---",
                    self.opts.head,
                    tail_buf.len()
                )?;
            }
            for l in tail_buf {
                writeln!(output, "{l}")?;
                bytes_out += l.len() + 1;
            }
        }

        Ok(CompressionResult {
            mode: CompressorMode::Full,
            kept_ranges: vec![],
            dropped_blocks: vec![],
            invariants: vec![],
            format_claim: Some(FormatKind::Plain),
            self_fidelity: 1.0,
            raw_pointer: SessionRef::new("".into()),
            bytes_written: bytes_out,
            bytes_read: bytes_in,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn ctx() -> Context {
        Context::new("fp", vec!["fake".into()])
    }

    #[test]
    fn dedups_adjacent() {
        let l1 = Layer1Generic::default();
        let mut input = Cursor::new(b"a\na\nb\nb\nb\nc\n".to_vec());
        let mut out: Vec<u8> = Vec::new();
        l1.compress(&mut input, &mut out, &ctx()).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "a\nb\nc\n");
    }

    #[test]
    fn strips_ansi() {
        let l1 = Layer1Generic::default();
        let mut input = Cursor::new(b"\x1b[31mred\x1b[0m text\n".to_vec());
        let mut out = Vec::new();
        l1.compress(&mut input, &mut out, &ctx()).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "red text\n");
    }

    #[test]
    fn strips_cr() {
        let l1 = Layer1Generic::default();
        let mut input = Cursor::new(b"line1\r\nline2\r\n".to_vec());
        let mut out = Vec::new();
        l1.compress(&mut input, &mut out, &ctx()).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "line1\nline2\n");
    }

    #[test]
    fn head_tail_envelope() {
        let opts = Layer1Options {
            head: 2,
            tail: 2,
            dedup: false,
            strip_ansi: false,
            strip_cr: false,
        };
        let l1 = Layer1Generic::new(opts);
        let input_bytes: Vec<u8> = (1..=10)
            .flat_map(|i| format!("line{i}\n").into_bytes())
            .collect();
        let mut input = Cursor::new(input_bytes);
        let mut out = Vec::new();
        l1.compress(&mut input, &mut out, &ctx()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("line1\n"));
        assert!(s.contains("line2\n"));
        assert!(s.contains("line9\n"));
        assert!(s.contains("line10\n"));
        assert!(!s.contains("line5\n"));
        assert!(s.contains("middle elided"));
    }
}
