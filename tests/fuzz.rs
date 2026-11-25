//! Random input for the protocol loop (without a fuzzing toolchain): lines of the
//! transcripts, changed at random, go to the plugin. It must neither panic nor hang, and
//! everything it writes must be JSON-RPC. RUNNEL_FUZZ_ITERATIONS sets the number of runs
//! (default 200; the weekly CI job runs many more), RUNNEL_FUZZ_SEED the start.

mod common;

use std::io::Cursor;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// xorshift64*: enough randomness for mutations, the same on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const TOKENS: &[&str] = &[
    "null",
    "true",
    "-1",
    "1e999",
    "\"\"",
    "{}",
    "[]",
    "\"x\"",
    "0",
    "9223372036854775808",
    "\"${A}\"",
    "\u{fffd}",
    "\\u0000",
    "[[1]]",
];

fn mutate(rng: &mut Rng, line: &str) -> String {
    let mut b = line.as_bytes().to_vec();
    for _ in 0..=rng.below(4) {
        if b.is_empty() {
            break;
        }
        let at = rng.below(b.len());
        match rng.below(4) {
            0 => {
                b.remove(at);
            }
            1 => b.insert(at, b"{}[]\":,0a\\\r"[rng.below(11)]),
            2 => {
                let t = TOKENS[rng.below(TOKENS.len())].as_bytes();
                b.splice(at..at, t.iter().copied());
            }
            _ => b.truncate(at),
        }
    }
    String::from_utf8_lossy(&b).into_owned()
}

#[test]
fn fuzz_serve() {
    let iterations: usize = std::env::var("RUNNEL_FUZZ_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let seed: u64 = std::env::var("RUNNEL_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0x5EED);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("protocol");
    let mut corpus = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let text = std::fs::read_to_string(e.unwrap().path()).unwrap();
        corpus.extend(text.lines().filter_map(|l| l.strip_prefix("→ ")).map(str::to_string));
    }
    let plugin = common::golden();
    let mut rng = Rng(seed | 1);
    for i in 0..iterations {
        let mut input = String::from("{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{}}\n");
        for _ in 0..=rng.below(6) {
            let line = &corpus[rng.below(corpus.len())];
            let line = if rng.below(3) == 0 {
                line.clone()
            } else {
                mutate(&mut rng, line)
            };
            input.push_str(&line);
            input.push('\n');
        }
        let out = Buf::default();
        let result = plugin.serve(Cursor::new(input.clone().into_bytes()), out.clone(), std::io::sink());
        assert!(result.is_ok(), "run {i}: serve failed: {result:?}\ninput:\n{input}");
        let text = String::from_utf8(out.0.lock().unwrap().clone()).expect("output is UTF-8");
        for line in text.lines() {
            let m: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("run {i}: not JSON ({e}): {line}\ninput:\n{input}"));
            assert_eq!(m["jsonrpc"], "2.0", "run {i}: {line}\ninput:\n{input}");
        }
    }
}
