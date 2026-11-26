# sdk-rust – Rust-SDK für Runnel-Plugins

Mit diesem SDK schreibt man [Runnel](https://github.com/runnelhq/runnel)-Plugins in Rust. Ein Plugin ist ein eigenes
Programm, das Runnel als Kindprozess startet und über JSON-RPC auf stdin/stdout anspricht (ADR 0019 von Runnel). Das
SDK übernimmt das Protokoll; ein Plugin beschreibt nur seine Actions, Transforms und Connection-Typen als Rust-Structs.

Verbindlich ist das Protokoll in [`docs/dev/PLUGINS.md`](https://github.com/runnelhq/runnel/blob/main/docs/dev/PLUGINS.md)
von Runnel, nicht dieses SDK. Funktionsumfang und Verhalten folgen dem [Go-SDK](https://github.com/runnelhq/sdk-go);
beide SDKs beantworten dieselben Transkripte des Protokolls (`testdata/protocol/`).

| | |
|---|---|
| Crate | `runnel-plugin` (noch nicht auf crates.io; Einbinden über Git, siehe unten), dazu `runnel-plugin-derive` für `#[derive(Config)]` |
| Protokoll | Version 1: `initialize`, `describe`, `action.execute`, `connection.test`, Transforms als Strom (`transform.open/batch/end/emit/closed`, `$/ack`), `transform.init`, `transform.detectFields`, `$/cancelRequest`, `shutdown` |
| Runnel | Actions und Connection-Typen ab S49c, Transforms ab S50a, `init`/`detectFields` ab S50b |
| Rust | ab 1.87 (Edition 2024) |
| Lizenz | MIT; Abhängigkeiten im Plugin nur `serde` und `serde_json` (MIT oder Apache-2.0), das Derive-Makro braucht beim Bauen `syn`, `quote` und `proc-macro2` |

## Ein Plugin in fünf Minuten

```toml
# Cargo.toml
[dependencies]
runnel-plugin = { git = "https://github.com/runnelhq/sdk-rust", tag = "v0.2.0" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

```rust
use runnel_plugin::{Action, Config, Plugin};
use serde::Deserialize;

#[derive(Deserialize, Config, Default)]
#[serde(default)]
struct GreetConfig {
    /// Whom to greet
    #[runnel(required, example = "World")]
    name: String,
    /// Capital letters
    shout: bool,
}

fn main() {
    let mut p = Plugin::new("hello", "1.0.0"); // name und version wie in plugin.yaml
    p.action(Action::new("hello.greet", "Greet", "Returns a greeting for a name", |call, c: GreetConfig| {
        call.log.info(format_args!("greeting {}", c.name));
        Ok(serde_json::json!({ "message": format!("Hello, {}!", c.name) }))
    }));
    p.main();
}
```

Das vollständige Beispiel mit Tests – Action und Transform `hello.greet` – liegt in [`examples/hello`](examples/hello). Bauen und im Projekt ablegen:

```bash
cargo build --release -p hello
```
```bash
mkdir -p D:/flows/plugins/hello/bin && cp target/release/hello.exe D:/flows/plugins/hello/bin/ && cp examples/hello/plugin.yaml D:/flows/plugins/hello/
```
```bash
runnel plugins trust D:/flows
```

Im Workflow steht das Ergebnis unter `results.<id>.output`:

```yaml
actions:
  - id: greet
    type: hello.greet
    with:
      name: ${TARGET}
```

## Kategorie und Gruppe in der Palette

`.category(…)` ist optional. Ohne Kategorie zeigt der Editor die Bausteine eines Plugins in einer eigenen Gruppe
„Plugin: \<name\>“; mit einer Kategorie der eingebauten stehen sie in deren Gruppe. Eine unbekannte Kategorie ist ein
Programmierfehler (`panic` beim Start).

| | Kategorien |
|---|---|
| Actions | `category::GENERAL`, `EXECUTION`, `FILES`, `NETWORK`, `DATABASE` |
| Transforms | `transform_category::INPUT`, `TRANSFORM`, `FLOW`, `COMBINE`, `OUTPUT` und `category::EXECUTION` |

Eine Eingabe (ohne eingehende Links) braucht `transform_category::INPUT`.

## Konfiguration als Struct

Das JSON Schema der Konfiguration entsteht mit `#[derive(Config)]` aus dem Struct – mit denselben Regeln wie bei den
eingebauten Actions von Runnel (ADR 0014) und im Go-SDK. Runnel prüft die `with:`-Werte gegen das Schema
(Pflichtfelder, unbekannte Felder), löst Variablen, Ausdrücke und Connections auf und ruft erst dann das Plugin.

| Angabe | Bedeutung |
|---|---|
| Name des Feldes, `#[serde(rename = "…")]`, `#[serde(rename_all = "camelCase")]` | Name in der Datei |
| `/// Kommentar` oder `#[runnel(doc = "…")]` | Beschreibung für Editor und Werkzeuge (englisch, ein Satz) |
| `#[runnel(required)]` | Pflichtfeld |
| `#[runnel(default = "…")]`, `example = "…"`, `values = "a\|b"` | Vorgabe, Beispiel, erlaubte Werte (`enum`) |
| `#[runnel(format = "runnel-secret")]` | Passwort oder Token (nur solche Felder dürfen `${secret:…}` nutzen) |
| `format = "runnel-duration"`, `"runnel-text"`, `"runnel-field"`, `"runnel-target"` | Zeitspanne (`30s`), mehrzeiliger Text, Feld der Datensätze, Ziel eines Daten-Links |
| `#[runnel(group = "Advanced")]`, `only = "mode:b"` | Abschnitt im Formular; Feld gilt nur bei einem Wert eines anderen Feldes |
| `#[runnel(connection = "ldap")]` | Name einer Connection dieser Typen; das Feld ist ein Struct und bekommt die **aufgelöste** Connection |
| `#[runnel(skip)]`, `#[serde(skip)]` | nicht im Schema (`skip` wird trotzdem gelesen) |
| `#[serde(flatten)]` | Felder eines anderen `Config`-Structs übernehmen (z. B. `ConnectionInfo`) |

Typen: Texte → `string`, Ganzzahlen → `integer` (ohne Vorzeichen mit `minimum: 0`), Kommazahlen → `number`, `bool` →
`boolean`, `Vec<T>` → `array`, Maps mit Text-Schlüsseln → `object`, Structs mit `Config` → `object`,
`serde_json::Value` → beliebiger Wert, `Option<T>` → wie `T`. Fehler in den Angaben (unbekannte Option, Vorgabe passt
nicht zum Typ, `only` ohne Geschwisterfeld) sind Compile-Fehler oder `panic` beim Hinzufügen der Action.

- Runnel schickt Konfigurationen ohne optionale Felder oder mit `null`; das Struct braucht deshalb `#[serde(default)]`
  und `Default`.
- Texte in Feldern vom Typ Zahl oder `bool` wandelt das SDK um, denn ein Wert aus einer Variablen (`times: ${N}`)
  kommt als Text an. Passt ein Wert nicht, scheitert die Action mit einer Meldung wie
  `field "times": expected a whole number, got string` – ohne Rust-Typnamen.
- Ohne Felder: `NoConfig`.

## Referenz: Ergebnis, Fehler und Grenzen

Wie die eingebauten Bausteine (ADR 0014 von Runnel, E15b) kann ein Baustein beschreiben, was er liefert, wann er
scheitert und welche Grenzen gelten. Runnel zeigt das im Katalog des Editors; alles ist optional.

```rust
p.action(
    Action::new("iban.validate", "Validate IBAN", "Checks an IBAN", validate).reference(
        Reference::new()
            .output(OutputField::new("output.valid", "boolean", "Whether the IBAN is valid"))
            .output(OutputField::new("output.country", "string", "Country code of the IBAN"))
            .errors("Fails when iban is empty.")
            .limit("An IBAN has at most 34 characters."),
    ),
);
```

| Teil | Inhalt |
|---|---|
| `output(…)` | Actions: Teile von `result.output` mit Pfad (`output`, `output.valid`, `output[].dn`) und Typ aus `OUTPUT_TYPES`. Transforms: Felder, die sie setzen, mit Typ aus `RECORD_OUTPUT_TYPES` (`string`, `int`, … oder `any`); ein Name aus der Konfiguration als `<feld>`, z. B. `<target>` oder `<fields[].name>` |
| `records(…)` | nur Transforms: welche Datensätze sie über welche Links weitergeben |
| `errors(…)` | wann der Baustein scheitert, bei Transforms was auf den Fehler-Link geht |
| `limit(…)` | Grenzen für Größe, Anzahl und Zeit mit Standardwerten |

Texte englisch; `records`, `errors` und `limits` als Sätze mit Punkt, die Beschreibung einer Ausgabe ohne Punkt.
`testing::check_references(&plugin())` prüft das wie die Vertragstests von Runnel: `errors` für jeden Baustein,
`records` für jeden Transform, Typen, Pfade und Felder in `<…>`. Der Aufruf im Test ist freiwillig.

## Connection-Typen

```rust
#[derive(Deserialize, Config, Default)]
#[serde(default)]
struct LdapConnection {
    #[serde(flatten)]
    info: ConnectionInfo, // name und type der Connection
    /// Address, e.g. ldaps://dc1.example.com
    #[runnel(required)]
    url: String,
    /// Password
    #[runnel(format = "runnel-secret")]
    password: String,
}

p.connection_type(ConnectionType::new("ldap", "LDAP", "LDAP or Active Directory server").test(|ctx, c: LdapConnection| {
    // anmelden, nichts schreiben
    Ok(TestResult { ok: true, message: "signed in".into(), version: String::new() })
}));

#[derive(Deserialize, Config, Default)]
#[serde(default)]
struct SearchConfig {
    /// LDAP connection
    #[runnel(required, connection = "ldap")]
    connection: LdapConnection,
}
```

- Der Name ist der Namensraum oder beginnt mit `<namensraum>-`.
- `.test(…)` ist optional; damit bietet der Editor „Test connection“ an. Ein Fehler zählt als gescheiterter Test
  (`ok: false`), nicht als Fehlerantwort.
- Secrets kommen im Klartext an. Runnel maskiert sie in allem, was vom Plugin zurückkommt; trotzdem nicht loggen.

## Transforms

Ein Transform liest Batches mit `next` und schreibt Records mit `emit`; das SDK hält die Kredite in beide Richtungen
ein. Derselbe Typname darf zugleich Action sein.

```rust
p.transform(
    Transform::new("hello.greet", "Greet", "Adds a greeting to every record", |s, c: GreetFieldConfig| {
        while let Some(b) = s.next()? {
            let at = field_index(&b.fields, &c.field).ok_or("field not found")?;
            let mut out = b.fields.clone();
            out.push(Field::new("greeting", types::STRING));
            let (mut good, mut bad) = (Vec::new(), Vec::new());
            for mut row in b.rows {
                match row[at].as_str().filter(|n| !n.is_empty()).map(|n| format!("Hello, {n}!")) {
                    Some(g) => {
                        row.push(Value::String(g));
                        good.push(row);
                    }
                    None => bad.push(error_row(&row, "name is empty", &c.field)),
                }
            }
            s.emit(TO_MAIN, &out, &good)?;
            s.emit(TO_ERROR, &error_fields(&b.fields), &bad)?;
        }
        Ok(())
    })
    .record_errors() // darf einen Fehler-Link haben
    .parallel(),     // erlaubt copies
);
```

- **Ziele von `emit`:** `TO_MAIN` (alle Daten-Links ohne eigenes Ziel), `TO_ERROR` (Fehler-Link; Records mit
  `error_fields`/`error_row`, also `_error` und `_errorField`) oder die ID eines Transforms aus einem Feld mit
  `format = "runnel-target"`.
- **Eingaben** (`.category(transform_category::INPUT)`) rufen `next` nicht auf und schreiben nur mit `emit`.
- **Kredite:** Das SDK nimmt höchstens `STREAM_WINDOW` (4) Batches an, die `next` noch nicht geliefert hat, und
  schickt `$/ack`, sobald `next` einen Batch liefert; `emit` wartet, bis Runnel frühere Records weitergegeben hat.
  Eingehende Nachrichten liest die Schleife des SDK, deshalb hängt ein wartendes `emit` nie an einem nicht gelesenen
  `$/ack`. Schickt Runnel mehr Batches als vereinbart, endet der Strom mit Fehler, statt Records zu verwerfen.
- **Größe:** Records, die nicht in eine Nachricht (16 MiB) passen, schickt `emit` in mehreren `emit`; ein einzelner
  Record über 16 MiB ist ein Fehler des Transforms.
- **Werte** (`Value`) nach dem Typ des Feldes (`types::…`): `Int` (i64), `Float`, `Decimal` (Text, exakt), `String`,
  `Bool`, `Date` (`"2026-11-24"`), `Datetime` (RFC 3339 mit Offset), `Binary` (Bytes, Base64 auf der Leitung), `Json`,
  `Null`. Ein Wert, der nicht zum Typ passt, beendet den Strom mit einer Meldung wie
  `row 3 of a batch: field "amount": "x" is not a value of type decimal`.
- **Ein Fehler** aus der Funktion (oder ein `panic`) beendet den Strom mit `transform.closed` und Meldung; die Pipeline
  scheitert. Mehrere Ströme (auch `copies`) laufen in einem Prozess nebeneinander, je in einem eigenen Thread.
- **Feldkopf:** Das SDK schickt `fields` mit jedem `emit` (wie das Go-SDK, wegen Runnel vor runnel#228).

### Schema und „Felder ermitteln“

```rust
Transform::new("hello.greet", "Greet", "…", greet_records)
    .init(|ctx, input: &InitInput, c: GreetFieldConfig| {
        let mut res = InitResult::default();
        if !input.input.open && field_index(&input.input.fields, &c.field).is_none() {
            res.problems.push(Problem::error("field", format!("field {:?} is not in the input", c.field)));
        }
        let mut fields = input.input.fields.clone();
        fields.push(Field::new("greeting", types::STRING));
        res.output = Some(RecordSchema { fields, open: input.input.open });
        res
    })
```

- **`.init(…)`** (optional, `transform.init`) prüft die Konfiguration und nennt die Felder des Ausgangs
  (`InitResult` mit `output` und `problems` am Pfad in `with`, z. B. `field` oder `fields[2].name`). Runnel ruft es
  beim Prüfen (`validate`, Editor) und vor dem Lauf auf: ohne Secrets, Connections nur mit Namen, Variablen eventuell
  noch als `${…}`. Werte, die dann nicht zum Typ passen, bleiben bei ihrer Vorgabe. `init` soll prüfen, nicht
  verbinden. Ohne `output` ist das Schema hinter dem Transform offen.
- **`.detect_fields(…)`** (optional, `transform.detectFields`) liest für „Detect fields“ im Editor die ersten Records
  einer Eingabe (höchstens `limit`, ohne Angabe 1.000) und schlägt Felder mit Typ und Beispielwerten vor
  (`Detection`); Connections kommen dabei aufgelöst.

Im Test: `c.run_transform(typ, config, &batches)` (Kredite wie bei Runnel, Ergebnis mit `records(TO_MAIN)`),
`c.init(typ, config, &input)`, `c.detect_fields(typ, config, limit)`.

**Durchsatz** (fünf Felder, vier Kerne): mit Runnel (Beispiel-Plugin, 500.000 Records, ein Feld dazu) etwa
380.000 Records/s; `make bench` misst den Strom ohne Runnel mit dem Test-Client (etwa 450.000 Records/s).

## Was das SDK übernimmt

- **Start:** Ohne `RUNNEL_PLUGIN=1` gibt `main` einen Hinweis aus und endet mit Exit-Code 2.
- **Handshake:** `initialize` mit `protocolVersion: 1`, `encoding: json`, Name, Version und `concurrency` (Standard 4,
  `p.concurrency(n)`); `describe` mit den Schemas. Typen außerhalb des Namensraums, doppelte Typen und unbekannte
  Kategorien sind Programmierfehler (`panic` beim Hinzufügen, also im ersten Test).
- **Laufzeit:** ohne async. Die Schleife liest stdin (Zeilen bis 16 MiB, `\r\n` erlaubt) und startet je Anfrage und je
  Strom einen Thread; höchstens `concurrency` Anfragen arbeiten gleichzeitig, Ströme zählen nicht mit. Ein Schreiber schreibt je Nachricht eine Zeile auf stdout.
- **Abbruch:** `$/cancelRequest` setzt `call.is_cancelled()`; `call.cancel().sleep(d)` wartet und wacht beim Abbruch
  auf. Das SDK antwortet mit `-32800`. Endet stdin, werden alle laufenden Anfragen abgebrochen.
- **Fehler:** Ein `Err` der Action wird zur Meldung (`-32000`); ein `panic` ebenso – das Plugin läuft weiter, die
  Meldung mit Backtrace steht in einer Zeile im Log. Anfragen vor `initialize`: `-32002`, unbekannte Methoden:
  `-32601`, ungültiges JSON: `-32700`, ungültige ID (keine ganze Zahl im Bereich von int64 und kein Text): `-32600`. Eine Antwort über 16 MiB wird zur Fehlermeldung der
  Action, statt das Plugin zu beenden; eingehende Nachrichten über 16 MiB beenden es.
- **Log:** `call.log.debug/info/warn/error(…)` schreibt Zeilen mit Stufe auf stderr; Runnel zeigt sie im Log des
  Laufs. Mehrzeilige Meldungen werden eine Zeile (` | `).
- **stdout** gehört allein dem Protokoll: kein `println!` im Plugin (stattdessen das Log oder `eprintln!`).
- **Panics:** Das SDK fängt sie mit `catch_unwind` ab; das Plugin darf daher nicht mit `panic = "abort"` gebaut
  werden.

## Testen

Das Modul `runnel_plugin::testing` spricht das Protokoll wie Runnel, ohne Prozess:

```rust
#[test]
fn greet() {
    let c = runnel_plugin::testing::Client::start(&plugin());
    let out = c.execute("hello.greet", serde_json::json!({"name": "World"})).unwrap();
    assert_eq!(out["message"], "Hello, World!");
}
```

`start_execute` und `PendingCall::cancel` prüfen den Abbruch, `test_connection` den Test einer Connection, `log` und
`log_contains` die Zeilen auf stderr. Bricht das Plugin das Protokoll (z. B. etwas anderes als JSON-RPC auf stdout),
scheitert der Test beim Beenden des Clients.

Gegen ein echtes Runnel testet `testing::Project`: Es legt das gebaute Plugin in ein temporäres Projekt, gibt es mit
`runnel plugins trust --yes` frei (Freigaben ebenfalls im temporären Ordner) und startet `runnel validate` und
`runnel run`. Im Integrationstest eines Plugins ist das Programm `env!("CARGO_BIN_EXE_<name>")`. Solche Tests laufen
nur, wenn `RUNNEL_BIN` auf das Programm zeigt:

```bash
make runnel-test RUNNEL_BIN=../runnel/bin/runnel
```

## Entwickeln

| Zweck | Befehl |
|---|---|
| Tests | `make test` – dazu Transkripte des Protokolls in `testdata/protocol/` (dieselben wie im Go-SDK, als JSON verglichen) |
| Lint (rustfmt, Clippy für Linux- und Windows-Code) | `make lint` (`rustup target add x86_64-pc-windows-msvc`) |
| Zufallstest der Protokoll-Schleife | `make fuzz FUZZ_ITERATIONS=100000` (wöchentlich in der CI) |
| Transkripte neu schreiben, wenn sich das Protokoll bewusst ändert | `make golden`, danach den Diff in `testdata/protocol/` prüfen |
| Benchmark (Strom eines Transforms) | `make bench` |
| Lizenzen und Schwachstellen der Abhängigkeiten | `make deny` ([cargo-deny](https://github.com/EmbarkStudios/cargo-deny)) |
| Tests gegen Runnel (in der CI unter Linux und Windows, Runnel aus `main`) | `make runnel-test RUNNEL_BIN=…` |

Die Toolchain ist in `rust-toolchain.toml` festgelegt, Abhängigkeiten sind mit `=` gepinnt. Versionen nach SemVer
mit Tags (`v0.1.0`); bis `v1.0.0` kann sich die Rust-API noch ändern, das Protokoll nicht. Änderungen stehen in
[CHANGELOG.md](CHANGELOG.md).
