# Änderungen

## v0.2.0 (unveröffentlicht)

- **Referenz** (ADR 0014 von Runnel, E15b): `Reference` mit `outputs`, `records`, `errors` und `limits` an Actions
  und Transforms (`.reference(…)`); `describe` liefert die Schlüssel wie die eingebauten Bausteine, leere fehlen.
  Runnel zeigt sie im Katalog des Editors. Fehler, die Runnel nicht zeigen könnte (`records` an einer Action, Ausgabe
  ohne Namen oder mit unbekanntem Typ), sind Programmierfehler (`panic` beim Start).
- `testing::check_references` prüft die Texte mit den Regeln der Vertragstests von Runnel (wie
  `runneltest.CheckReference` im Go-SDK); der Aufruf ist freiwillig.
- IDs: Die Meldung für ungültige IDs lautet jetzt wie im Go-SDK
  `invalid id: an id is a whole number (int64) or a text`; ganze Zahlen über int64 (z. B. `9223372036854775808`) sind
  ungültig (Nachtrag 2026-11-26 zu ADR 0019).
- Transkripte in `testdata/protocol/` wieder identisch mit dem Go-SDK (Referenz, IDs, `null` für leere Listen).
- `testing::Project::command` startet `runnel` mit den Freigaben des Projekts, z. B. `runnel serve`.
- Beispiel-Plugin mit Referenz; der Test gegen Runnel liest sie aus `/api/v1/catalog`.

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
