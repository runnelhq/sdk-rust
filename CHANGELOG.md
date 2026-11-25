# Änderungen

## Unveröffentlicht

- Erste Fassung für Runnel ab S49c: Handshake, `describe` mit Schemas aus Rust-Structs (`#[derive(Config)]`, dieselben
  Schemas wie im Go-SDK und bei eingebauten Bausteinen), `action.execute`, Connection-Typen mit `connection.test`,
  `concurrency`, `$/cancelRequest`, Log über stderr, `panic` als Fehlermeldung.
- `testing::Client` für Tests ohne Runnel, `testing::Project` für Tests gegen Runnel (`RUNNEL_BIN`).
- Transforms als Strom (Runnel ab S50a): `Plugin::transform`, `Stream::next` und `Stream::emit` mit Krediten in beide
  Richtungen (höchstens 4 Batches voraus, `emit` wartet auf `$/ack`), Ziele `main`, `error` und `runnel-target`,
  Werte nach ADR 0007 (`Value`), zu große Ausgaben werden geteilt. `testing::Client::run_transform` mit Krediten wie
  Runnel. Benchmark `make bench`.
- `transform.init` und `transform.detectFields` (Runnel ab S50b): `Transform::init` und `Transform::detect_fields`;
  für `init` bleiben Werte, die noch `${…}` sind, bei ihrer Vorgabe. Die Transkripte in `testdata/protocol/` sind jetzt
  dieselben wie im Go-SDK.
- Beispiel-Plugin `examples/hello` mit Action und Transform (mit `init`); Test gegen Runnel mit Workflow, Pipeline und
  Prüfung.
