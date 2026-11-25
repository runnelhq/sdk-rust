# Änderungen

## Unveröffentlicht

- Erste Fassung für Runnel ab S49c: Handshake, `describe` mit Schemas aus Rust-Structs (`#[derive(Config)]`, dieselben
  Schemas wie im Go-SDK und bei eingebauten Bausteinen), `action.execute`, Connection-Typen mit `connection.test`,
  `concurrency`, `$/cancelRequest`, Log über stderr, `panic` als Fehlermeldung.
- `testing::Client` für Tests ohne Runnel, `testing::Project` für Tests gegen Runnel (`RUNNEL_BIN`).
- Beispiel-Plugin `examples/hello` mit Action.
