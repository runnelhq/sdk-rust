//! Transcripts of the protocol (testdata/protocol): the "→ " lines go to the plugin, its
//! answers must be the "← " lines, compared as JSON (the order of keys does not matter).
//! The transcripts are those of the Go SDK, so both SDKs speak the protocol alike. With
//! UPDATE=1 the answers are written anew; check the diff before committing.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value as Json;

/// Comparing, the test waits for as many answers as the transcript has (each within
/// ANSWER_TIMEOUT) and then EXTRA_QUIET for more; updating, until the plugin is quiet for
/// UPDATE_QUIET.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
const EXTRA_QUIET: Duration = Duration::from_millis(50);
const UPDATE_QUIET: Duration = Duration::from_millis(300);

#[test]
fn golden_transcripts() {
    let update = std::env::var("UPDATE").is_ok_and(|v| v == "1");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join("protocol");
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    assert!(!files.is_empty(), "no transcripts in {}", dir.display());
    let mut failures = Vec::new();
    for file in files {
        let want = std::fs::read_to_string(&file).unwrap().replace("\r\n", "\n");
        let got = play(&want, update);
        if update {
            std::fs::write(&file, &got).unwrap();
        } else if let Err(e) = compare(&got, &want) {
            failures.push(format!("{}: {e}\n--- got\n{got}--- want\n{want}", file.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "transcripts differ (UPDATE=1 cargo test --test golden, then check the diff):\n{}",
        failures.join("\n")
    );
}

fn compare(got: &str, want: &str) -> Result<(), String> {
    let g: Vec<&str> = got.lines().collect();
    let w: Vec<&str> = want.lines().collect();
    if g.len() != w.len() {
        return Err(format!("{} lines, want {}", g.len(), w.len()));
    }
    for (i, (a, b)) in g.iter().zip(&w).enumerate() {
        let equal = match (a.strip_prefix("← "), b.strip_prefix("← ")) {
            (Some(x), Some(y)) => serde_json::from_str::<Json>(x).ok() == serde_json::from_str::<Json>(y).ok(),
            _ => a == b,
        };
        if !equal {
            return Err(format!("line {}: got {a}, want {b}", i + 1));
        }
    }
    Ok(())
}

/// Sends the "→ " lines of a transcript and returns the transcript with the answers the
/// plugin gave; other lines (comments) stay as they are.
fn play(transcript: &str, update: bool) -> String {
    let (in_r, mut in_w) = std::io::pipe().unwrap();
    let (out_r, out_w) = std::io::pipe().unwrap();
    let plugin = common::golden();
    let serve = std::thread::spawn(move || plugin.serve(in_r, out_w, std::io::sink()));
    let (tx, answers) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out_r).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let lines: Vec<&str> = transcript.trim_end_matches('\n').split('\n').collect();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("← ") {
            continue; // replaced by the answers the plugin gives now
        }
        out.push_str(line);
        out.push('\n');
        let Some(request) = line.strip_prefix("→ ") else {
            continue;
        };
        writeln!(in_w, "{request}").unwrap();
        let expected = lines[i + 1..].iter().take_while(|l| l.starts_with("← ")).count();
        let mut push = |a: String| {
            out.push_str("← ");
            out.push_str(&a);
            out.push('\n');
        };
        if update {
            while let Ok(a) = answers.recv_timeout(UPDATE_QUIET) {
                push(a);
            }
            continue;
        }
        for _ in 0..expected {
            match answers.recv_timeout(ANSWER_TIMEOUT) {
                Ok(a) => push(a),
                Err(_) => break, // the comparison shows what is missing
            }
        }
        while let Ok(a) = answers.recv_timeout(EXTRA_QUIET) {
            push(a);
        }
    }
    drop(in_w);
    serve.join().unwrap().unwrap();
    out
}
