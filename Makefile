# Rust-SDK für Runnel-Plugins. Ziele: make test | lint | fmt | fuzz | bench | deny | golden | runnel-test
FUZZ_ITERATIONS ?= 20000
# Runnel für die Tests gegen den Host, z. B. RUNNEL_BIN=../runnel/bin/runnel
RUNNEL_BIN      ?=
WINDOWS_TARGET  := x86_64-pc-windows-msvc

.PHONY: test lint fmt fuzz bench deny golden runnel-test

## test: alle Tests (Tests gegen Runnel nur mit RUNNEL_BIN)
test:
	RUNNEL_BIN=$(RUNNEL_BIN) cargo test --workspace --all-targets
	cargo test --workspace --doc

## lint: Formatierung und Clippy für Linux- und Windows-Code (rustup target add $(WINDOWS_TARGET))
lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo clippy --workspace --all-targets --target $(WINDOWS_TARGET) -- -D warnings

## fmt: Quelltext formatieren
fmt:
	cargo fmt --all

## fuzz: Protokoll-Schleife mit zufällig veränderten Nachrichten (FUZZ_ITERATIONS Läufe)
fuzz:
	RUNNEL_FUZZ_ITERATIONS=$(FUZZ_ITERATIONS) cargo test --release --test fuzz

## bench: Durchsatz eines Transforms über das Protokoll (ohne Runnel)
bench:
	cargo test --release --test bench -- --ignored --nocapture

## deny: Lizenzen, Schwachstellen und Quellen der Abhängigkeiten (cargo-deny)
deny:
	cargo deny check

## golden: Transkripte des Protokolls (testdata/protocol) neu schreiben; danach den Diff prüfen
golden:
	UPDATE=1 cargo test --test golden

## runnel-test: nur die Tests gegen Runnel (RUNNEL_BIN nötig)
runnel-test:
	@test -n "$(RUNNEL_BIN)" || (echo "set RUNNEL_BIN to the runnel program" && exit 2)
	RUNNEL_BIN=$(abspath $(RUNNEL_BIN)) cargo test --workspace --all-targets with_runnel
