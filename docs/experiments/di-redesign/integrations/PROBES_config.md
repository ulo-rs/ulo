# Probes: configuration (`ulo-config`)

Race 3, configuration premises from `RESPONSE.md` §2.1 and Q3, and `CAPABILITIES.md` capabilities 1 to 5. Every claim is marked **Probed** (code run, output below), **Read** (source or docs read) or **Unverified**.

Toolchain: cargo 1.98.1, macOS. Scratch crates live in `/private/tmp/claude-501/-Users-ifeanyi-Projects-Personal-open-source-toni-rs-toni-rs/a632688a-d566-4ff7-b727-8f28e1dd90c6/scratchpad/race3-probes-config/` (`p1` figment, `p2` origin-tree demo, `p2b` toml_edit/serde_spanned, `p3` JSON parsers, `p4`/`p4b` YAML, `p5` dotenvy, `p6` Secret, `p7` the repo's `ulo` renderer). Nothing in the repo was touched except this file.

Crate versions used (from each probe's `Cargo.lock`, latest on crates.io on 2026-10-18 unless noted):

| Crate | Version | Latest release (crates.io) | Recent downloads |
| --- | --- | --- | --- |
| figment | 0.10.19 (is the latest) | 2024-05-17 | 12.5M |
| toml | 1.1.8+spec-1.1.0 (figment itself pulls 0.8.23) | 2026-10-09 | 263M |
| toml_edit | 0.25.17 | | |
| serde_spanned | 1.1.2 | | |
| serde_json | 1.0.154 | 2026-10-11 | 367M |
| jsonc-parser | 0.34.0 | 2026-09-27 | 3.2M |
| json-spanned-value | 0.2.2 | 2020-10-10 | 94k |
| serde_json_borrow | 0.9.0 | 2025-09-30 | 155k |
| simd-json | 0.18.1 | 2026-08-23 | 6.1M |
| json5 | 1.3.2 | 2026-10-14 | 16.4M |
| serde_path_to_error | 0.1.20 | 2025-09-15 | 99M |
| serde_yaml | 0.9.34+deprecated | 2024-03-25 | 96.7M |
| serde_yml | 0.0.13 | 2026-05-27 | 5.2M |
| serde_norway | 0.9.42 | 2024-12-21 | 7.3M |
| serde-saphyr | 2.0.0 | 2026-10-13 | 8.3M |
| saphyr | 0.2.2 | 2026-10-15 | 0.95M |
| yaml-rust2 | 0.13.0 | 2026-09-11 | 18.9M |
| dotenvy | 0.15.7 | 2023-03-22 | 55.6M |

Release dates and download counts are from `https://crates.io/api/v1/crates/<name>` (Probed, fetched today).

## 1. figment: what its errors carry

Setup: `p1`, `figment = "=0.10.19"` with `toml`, `json`, `env`; target `struct Cfg { name: String, http: Http { port: u16, host: String } }`.

Command: `cargo run` in `p1`. Files: `app.toml` has `port = "eighty"` and `host = 5` (line 4 and 5); `app.json` the same shape pretty-printed over seven lines.

Raw `Display` output (Probed):

```
toml        invalid type: found signed int `5`, expected a string for key "default.http.host" in app.toml TOML file
json        invalid type: found unsigned int `5`, expected a string for key "default.http.host" in app.json JSON file
env         invalid type: found string "eighty", expected u16 for key "HTTP.PORT" in `APP_` environment variable(s)
serialized  invalid type: found string "p", expected u16 for key "default.http.port" in src/main.rs:17:42 p1::main::D
```

`Debug` (toml case, abridged): `Error { tag: Tag(Default, 1), profile: Some(Profile("default")), metadata: Some(Metadata { name: "TOML file", source: Some(File("/…/p1/app.toml")), provide_location: Some(Location { file: "src/main.rs", line: 12, column: 30 }), .. }), path: ["http", "host"], kind: InvalidType(Signed(5), "a string"), prev: None }`.

Findings:

- Key path: yes, as a dotted string with the profile prepended (`default.http.host`), and as `Error.path: Vec<String>`. For env the key shown is the post-transform one (`HTTP.PORT`), not the variable name `APP_HTTP_PORT`. (Probed)
- Source: file name for file providers (`in app.toml TOML file`), a description for env (``in `APP_` environment variable(s)``), which names the prefix, not the variable. A `Serialized` default reports the Rust source location of the call (`src/main.rs:17:42`) and the type name. (Probed)
- Line and column for a type error in a file: **no**. `Metadata` has `source` (a path) and `provide_location` (where the Rust code built the provider), and nothing per value. (Probed)
- Line and column for a file syntax error: yes, because the TOML and JSON parsers produce it: `TOML parse error at line 1, column 8 …` and `expected value at line 1 column 10 in app5.json JSON file`. (Probed)
- All problems or the first: **first only**. A file with `name = 1` and `port = "x"` (and a second file with both `port = "eighty"` and `host = 5`) reported one error each, `e.count()` was 1. The reported key is the first in key (map) order, not struct order: `host` before `port`, `http` before `name`. (Probed)

Verdict: **holds** for the designer's claim. figment names the path and the source, gives no line for values, reports one error. The probe confirms the reason for not building on it. Latest (0.10.19) is the version in `Cargo.lock`, so nothing newer exists to try. figment 0.10.19 also depends on `toml` 0.8.23, so using it beside `toml` 1.x would carry both.

Meaning for the design: the crate builds its own tree, as `RESPONSE.md` says. Premise 1 needs no change.

## 2. TOML: spans, and a working origin-tree demo

Probe `p2b` (Probed):

```
toml_edit item span: Some(19..23)
Spanned<u16>: value=8080 span=19..23
toml::de::Error span=Some(18..22)
TOML parse error at line 2, column 8
  |
2 | port = "no"
  |        ^^^^
invalid type: string "no", expected u16
```

Three paths to a span exist: `toml_edit::ImDocument::parse(..).get(k).span()`, `toml::Spanned<T>` as a field type (`serde_spanned::Spanned`), and `toml::de::DeTable::parse(src)` / `DeValue::parse`, which return a `Spanned<DeValue>` tree where every node, key and table carries a byte range (Read in `toml-1.1.8/src/de/parser/devalue.rs`, and used below). The span is a byte range into the source; a line and a column come from counting newlines before `span.start`. Note that `toml::from_str` on its own already reports line and column for a typed error but gives no field path and stops at the first error (Probed above).

The demo (`p2/src/tree.rs`, 235 lines including the JSON builder; `p2/src/main.rs`, 34 lines):

- `from_toml(file, src) -> Node` walks `DeTable`/`DeValue` and records `Origin::File { file, line, col }` on every leaf and table (about 30 lines).
- `Node` is `Leaf(Val, Origin) | Table | Array | Missing`; `merge(base, over)` layers two trees leaf by leaf (about 10 lines); `set(path, val, origin)` inserts an `Origin::Env(var)` leaf (about 8 lines).
- `De` implements `serde::Deserializer` over `&Node` with a shared `RefCell<Ctx>` that collects `Problem { path, origin, msg }` (about 120 lines for the deserializer, `Seq`, `Map` and the macro for integers).
- `extract::<T>(root, ctx)` is the driver; `unread(root, ctx)` lists the leaves no deserialiser consumed (about 12 lines), which is the "unread key" check in `RESPONSE.md` §2.1.

How it reports every error rather than the first: a type mismatch records a `Problem` and then hands the visitor a placeholder (`visit_i64(0)`, `visit_str("")`, `visit_bool(false)`) so the derive keeps going. A missing field is raised by the derive at the end of the struct through `Error::missing_field`, which the demo's error type overrides; the demo records the problem, remembers the field path in a patch set, returns a retry marker and reruns the whole pass, feeding a `Missing` placeholder node for each patched key. `Option` fields see `Missing` as `None` and `#[serde(default)]` fields are never raised as missing (the `workers` field in the run below).

Run (`cargo run` in `p2`; output verbatim, `Cfg`/`Http`/`Db` as in `main.rs`):

```
== good + unread
ok: Cfg { name: "x", http: Http { port: 8080, host: "h", bind: 127.0.0.1, workers: 0 }, db: Db { url: "pg://", pool: None }, retries: [1, 2], debug: false }
unread: db.typo_key (app.toml:10:12)
== bad
debug at app.toml:3:9: expected boolean, found string
http.port at app.toml:5:8: expected u16, found 70000 (out of range)
http.host at app.toml:4:1: missing required field
name at app.toml:1:8: expected string, found integer
retries[1] at app.toml:2:15: expected u8, found 300 (out of range)
retries[2] at app.toml:2:20: expected u8, found string
db at app.toml:1:1: missing required field
== custom FromStr failure (IpAddr) plus a later type error
db.url at app.toml:9:7: expected string, found integer
http.bind at app.toml:7:8: invalid IP address syntax
== env override with bad type
http.port at env APP_HTTP__PORT: expected u16, found string
== json file
http.port at app.json:6:13: expected u16, found string
http.host at app.json:5:11: missing required field
name at app.json:2:11: expected string, found integer
retries[1] at app.json:3:18: expected u8, found 300 (out of range)
db at app.json:1:1: missing required field
json syntax error: Unexpected close brace on line 1 column 7
syntax error: TOML parse error at line 1, column 5
```

Verdict: **holds**, with three limits found by running it (Probed):

1. An error raised by a leaf type's own `Deserialize` from a valid scalar (here `IpAddr` from `"not-an-ip"`, any `FromStr`-style newtype, a `Url`) has no placeholder value. The demo records it with its origin and aborts the enclosing struct, so the fields after it, and its siblings after it in key order, are not checked. In the `IpAddr` case above, `db.url` (visited first) is reported and the rest after `http.bind` is not. "Every error together" therefore holds for missing fields, wrong types and out-of-range numbers, and not for failures inside a leaf type's parser. Closing that needs a second validation pass or a per-leaf catch, and is not solved here. (Probed)
2. A table origin for a "missing field" is the table's header, which is the best available line; a missing top-level key gets `1:1`. (Probed)
3. The retry loop reruns the deserialization, so anything recorded during a pass (secret leaves, read marks) must be de-duplicated. The demo dedups problems on (path, message) and sets for the read marks; the secret list in section 6 is not de-duplicated. (Probed)

Meaning: the origin tree is about 200 lines of crate code for TOML plus 20 per further format, not a dependency. The crate needs a stable `Problem` type, not serde's error, and must treat leaf-parser failures as a documented single-error case.

## 3. JSON: value spans

Probe `p3` on a seven-line document with `"name": 7` and `"port": "eighty"` (Probed):

```
--- serde_json typed from_str
Display: invalid type: integer `7`, expected a string at line 2 column 11
line=2 col=11 class=Data
--- serde_json from Value (no positions)
Display: invalid type: string "eighty", expected u16
line=0 col=0
--- serde_path_to_error + serde_json
path=name inner=invalid type: integer `7`, expected a string at line 2 column 11 (inner line=2 col=11)
--- json-spanned-value
key "http" span 17..23 value span 25..51
key "name" span 4..10 value span 12..13
--- jsonc-parser AST ranges
prop name range=Range { start: 4, end: 13 } value range=Range { start: 12, end: 13 }
prop http range=Range { start: 17, end: 51 } value range=Range { start: 25, end: 51 }
--- json5 typed error
Display: expected string at line 2 column 11  debug: Error { inner: ErrorInner { content: Code(ExpectedString), position: Some(Position { line: 1, column: 10 }) } }
```

| Crate | Last release | Recent downloads | Line/col per value | Notes |
| --- | --- | --- | --- | --- |
| serde_json 1.0.154 | 2026-10-11 | 367M | Only through a typed `from_str`: data errors carry line and column of the end of the failing token (shown above). Through `Value`, nothing (`line=0`). Syntax errors always. | One error, no tree positions. |
| serde_path_to_error 0.1.20 | 2025-09-15 | 99M | Path only; with serde_json the position rides on the inner error. | First error only. |
| jsonc-parser 0.34.0 | 2026-09-27 | 3.2M | **Yes**: `parse_to_ast` gives a byte `Range` per value and property; line and column come from counting newlines. | Superset of JSON (comments, trailing commas); `ParseOptions { allow_comments: false, allow_trailing_commas: false, allow_loose_object_property_names: false, .. }` makes it strict (used in the demo). Maintained by dprint. |
| json-spanned-value 0.2.2 | **2020-10-10** | 94k | Yes (byte spans) | Unmaintained for six years. |
| serde_json_borrow 0.9.0 | 2025-09-30 | 155k | Not seen; Read: a borrowed `Value`, no spans. | Unverified in code. |
| simd-json 0.18.1 | 2026-08-23 | 6.1M | Not seen. Read: tape nodes, no public per-value offsets; syntax errors give an index. | Unverified in code; MSRV 1.88. |
| json5 1.3.2 | 2026-10-14 | 16.4M | Error position only (`Position { line, column }` on a syntax or typed error). | No value spans. |

The demo in section 2 includes `from_json` over `jsonc-parser` (about 20 lines) and produced the JSON lines in the run above with `file:line:col` per leaf (`app.json:6:13`, and so on). (Probed)

Verdict: **does not hold as stated, and better than the fallback**. The designer expected JSON to be file plus key path only if no maintained parser gives value spans. `jsonc-parser` is maintained (released three weeks ago, 3.2M recent downloads) and gives value ranges, so JSON gets the same line and column as TOML. Column semantics: `jsonc-parser` gives the start of the value (the demo reports the first character), while `serde_json`'s typed error gives the end of the token.

Meaning: `ulo-config` takes `jsonc-parser` as the JSON parser behind a `json` feature, configured strict. If the dependency is unwanted, the fallback (file and key path, no line) is still correct for `serde_json::Value`, but costs the precision the crate exists to give.

## 4. YAML

Advisory (Read, `https://raw.githubusercontent.com/rustsec/advisory-db/main/crates/serde_yml/RUSTSEC-2025-0068.md`):

```
id = "RUSTSEC-2025-0068"   package = "serde_yml"   date = "2025-09-11"   informational = "unsound"
aliases = ["GHSA-hhw4-xg65-fp2x"]   [versions] patched = []

# serde_yml crate is unsound and unmaintained
Using `serde_yml::ser::Serializer.emitter` can cause a segmentation fault, which is unsound.
The GitHub project for `serde_yml` was archived after unsoundness issues were raised.
If you rely on this crate, it is highly recommended switching to a maintained alternative.
Recommended alternatives: serde_norway (Maintained fork of serde_yaml, using unsafe-libyaml-norway);
serde_yaml_ng (…using unmaintained unsafe-libyaml). Incomplete pure Rust alternatives: serde_yaml2, yaml-peg.
```

Note that crates.io shows `serde_yml` 0.0.13 published 2026-05-27, after the advisory; the advisory still lists no patched version. `serde_yaml` is `0.9.34+deprecated` (2024-03-25, archived); the advisory-db lists RUSTSEC-2018-0005 under it, whose content I did not read (Unverified). (Read)

| Crate | Maintained | `unsafe` | MSRV | Location in errors |
| --- | --- | --- | --- | --- |
| serde_yaml | No (archived) | in libyaml port | 1.64 | n/a |
| serde_yml | No (archived, RUSTSEC-2025-0068) | yes | 1.85 | n/a |
| serde_norway 0.9.42 | Last release 2024-12-21 (ten months), 7.3M recent downloads | **64 `unsafe` hits in src** plus the `unsafe-libyaml-norway` C-port dependency (Probed by grep and `cargo tree`) | 1.71 | yes, `line`, `column`, `index` |
| serde-saphyr 2.0.0 | Yes (2026-10-13) | `#![forbid(unsafe_code)]` in `lib.rs` and `unsafe_code = "forbid"` in `Cargo.toml` (Probed); its tree includes `encoding_rs` and `simdutf8`, which are not unsafe-free | **1.89** | yes, `line N column M` plus a source snippet |
| saphyr 0.2.2 | Yes (2026-10-15) | 0 `unsafe` hits in src (Probed by grep); depends on `encoding_rs`, `hashlink` | 1.85 | per-node `span` (below) |
| yaml-rust2 0.13.0 | Yes (2026-09-11) | not checked | 1.85 | Unverified |

Type-error probe (`p4`, one document each; output abridged) (Probed):

```
serde-saphyr  error: line 3 column 9: invalid u16
               --> <input>:3:9
              3 |   port: eighty
                |         ^ invalid u16
serde_norway  http.port: invalid type: string "eighty", expected u16 at line 3 column 9   location=(line 3, col 9, index 22)
-- missing field --
serde-saphyr  error: line 3 column 3: missing field `host`
serde_norway  http: missing field `host` at line 3 column 3
-- syntax --
serde-saphyr  error: line 2 column 7: expected mapping start
serde_norway  http: invalid type: sequence, expected struct Http at line 2 column 7
```

Differences: `serde_norway` reports a **field path** (`http.port`) and a typed location (`e.location()`); `serde-saphyr` reports a location with a rendered snippet and no path in the message (`invalid u16`, without the found value). Both report one error. (Probed)

Positions without a serde crate: `saphyr::MarkedYaml::load_from_str` yields a tree where each node has `span.start.line()` and `col()` (0-based column). `p4b` output (Probed):

```
.name = Value(Integer(7)) at line 1 col 7
.http.port = Value(String("eighty")) at line 3 col 9
.http.hosts[0] = Value(String("a")) at line 4 col 11
.http.hosts[1] = Value(String("b")) at line 4 col 14
```

That is the same shape the TOML builder consumes, so the origin tree and its collecting deserializer from section 2 work for YAML with a roughly 20-line builder, and no serde YAML crate is needed at all. Not probed: anchors, aliases and merge keys (`<<`) under `MarkedYaml`, which the builder has to decide on; YAML 1.1 versus 1.2 scalar resolution (`yes`, `no`, `0o17`).

Recommendation for a `yaml` feature: parse with **`saphyr`** (`MarkedYaml`) into the origin tree. Reasons: positions per node, maintained, no `unsafe` in its source, MSRV 1.85 which fits the 1.88 workspace floor. `serde-saphyr` would force the YAML feature crate to 1.89 (above the workspace's 1.88), and `serde_norway` carries a C-port `unsafe` library and a ten-month release gap. If the tree approach is rejected for YAML, the fallback is `serde_norway` (path plus line, one error) with its floor and `unsafe` stated, not `serde-saphyr`.

## 5. `.env`: `dotenvy`

`dotenvy 0.15.7` (last release 2023-03-22; no newer release exists on crates.io). Probe `p5` over this file (Probed):

```
# a comment
PLAIN=value
SPACED = with spaces
export EXPORTED=yes
DQ="double \"quoted\" # not a comment\nline2"
SQ='single $PLAIN ${PLAIN}'
EXPAND=$PLAIN/${PLAIN}/x
INLINE=abc # trailing comment
EMPTY=
MULTI="line one
line two"
BAD LINE
AFTER=ok
```

Output:

```
before: PLAIN=Err(NotPresent)
"PLAIN" => "value"
ERR: Error parsing line: 'with spaces', error at line index: 5  (debug LineParse("with spaces", 5))
"EXPORTED" => "yes"
"DQ" => "double \"quoted\" # not a comment\nline2"        (Debug-escaped; the \n is a real newline)
"SQ" => "single $PLAIN ${PLAIN}"
"EXPAND" => "value/value/x"
"INLINE" => "abc"
"EMPTY" => ""
"MULTI" => "line one\nline two"
ERR: Error parsing line: 'BAD LINE', error at line index: 4  (debug LineParse("BAD LINE", 4))
"AFTER" => "ok"
after: PLAIN=Err(NotPresent) EXPORTED=Err(NotPresent) AFTER=Err(NotPresent)
process env has any key from file: false
-- expansion against process env (OUTER set in the process before the read)
Ok(("X", "from-process"))
Ok(("Y", "from-process-z"))
Ok(("Z", ""))
control after dotenvy::from_path (writes): PLAIN=Ok("value")
```

Findings:

- `from_path_iter` does **not** set the process environment: after reading, `std::env::var` returns `NotPresent` for every key, and no key of the file appears in `std::env::vars()`. The control, `from_path`, does set it. (Probed; holds)
- Quoting, comments, `export` prefix, multi-line double-quoted values and empty values behave as the common subset. `SQ` stays literal in single quotes. (Probed; holds)
- **Variable expansion is on and cannot be turned off**: `$PLAIN` and `${PLAIN}` expand from earlier keys of the file, and `$OUTER` expands from the **process environment** (`from-process`). This contradicts the design's "no variable expansion" and the capability that a `.env` read never involves the process environment unless the application chose it. A missing variable expands to the empty string, silently (`Z`). (Probed; does not hold)
- `KEY = value` with spaces around `=` is a parse error. A bad line yields an `Err` item and the iterator continues, so every bad line can be reported. The error carries the line text and an index **within the line**, not a line number. (Probed)

Verdict: the premise "the iterator reads without setting the environment" **holds**; the premise that it fits the design's common subset **partly** holds, because of expansion (reads the process environment) and the missing line numbers.

Meaning: either write the `.env` reader in the crate (the subset is about 40 lines: `#` comments, optional `export`, `KEY=VALUE`, single and double quotes, line numbers for origin), or take `dotenvy` and document that `$VAR` expands, against the capability. Writing it gives `file:line` origins (which `dotenvy` cannot) and removes the process-environment read. Recommended: own parser. I did not probe a third-party `.env` parser that disables expansion (Unverified).

## 6. The `Secret` mechanism

`Secret<T>(T)` with a hand-written `Deserialize` that calls `deserializer.deserialize_newtype_struct("Secret", visitor)`, whose visitor's `visit_newtype_struct` deserializes `T`. Probe `p6`, target `Cfg { name: String, api_key: Secret<String>, pin: Option<Secret<u16>>, tokens: Vec<Secret<String>> }` (Probed):

```
serde_json  : Ok(Cfg { name: "n", api_key: Secret(<redacted>), pin: Some(Secret(<redacted>)), tokens: [Secret(<redacted>), Secret(<redacted>)] })
toml        : Ok(…same…)
serde_saphyr: Ok(…same…)
serde_norway: Ok(…same…)
inner via toml: k-toml Some(4321) 2
```

All four deserializers decode the newtype transparently from a bare string, integer, optional and array element. (holds)

Custom deserializer recognising the name (`tree.rs`, `deserialize_newtype_struct`: when `name == "Secret"` and the node is a leaf, push `(path, origin)` into `Ctx.secrets`, then `visit_newtype_struct(self)`). Run on the TOML document of the probe (Probed):

```
secret leaf: api_key at app.toml:2:11
secret leaf: pin at app.toml:3:7
secret leaf: tokens[0] at app.toml:4:11
secret leaf: tokens[1] at app.toml:4:17
secret count = 4
```

Exactly the four secret leaves, and not `name`. It took 4 lines in the deserializer. (holds)

Three failures found (Probed):

1. **`#[serde(flatten)]` and `#[serde(untagged)]` hide the name.** Serde buffers such fields into its private `Content` type, and the buffered deserializer never sees `deserialize_newtype_struct("Secret", ..)` from the custom deserializer. Both cases deserialized successfully and recorded nothing:
   ```
   flatten result: Ok(Flat { name: "n", inner: Inner { api_key: Secret(<redacted>) } }); secrets recorded: []
   untagged result: Ok(Holder { u: A(Inner { api_key: Secret(<redacted>) }) }); secrets recorded: []
   ```
   The consequence is a secret that is not registered for redaction, with no error. The designer's "no thread-local" position does not cover these two attributes. Choices: document that a `Secret` must not sit under `flatten` or `untagged`; or keep a scoped thread-local collector that `Secret::deserialize` itself writes to (this catches every case, because `Secret::deserialize` runs on the buffered content too); or both.
2. **The crate's own messages can print the secret.** The demo's range message printed `expected u8, found 300 (out of range)` for a `Secret<u8>` leaf holding 300:
   ```
   secret out-of-range problems: ["expected u8, found 300 (out of range)"]
   ```
   serde's and the parsers' own messages do the same (`invalid type: string "eighty"`). The tree knows the secret paths before it renders a problem (the newtype name is seen first), so the crate must render only the kind and path for any problem under a secret leaf and drop the found value. Core redaction would catch it only if the value were registered first, and `redact.rs` states that on the `try_value` path "the registry has nothing to replace" because the secret sits inside the configuration that failed (Read, `crates/ulo/src/redact.rs:7-10`).
3. The collecting retry loop runs the deserialization more than once, so `Ctx.secrets` holds duplicates after a retry; registration with `m.secret(..)` must tolerate that (`SecretRegistry::register` already skips repeats, `redact.rs`, Read).

Also, Q3 of the third response asks whether the core's `Secret` already implements `Deserialize`. It does not: `crates/ulo/Cargo.toml` has no `serde` dependency or feature and `crates/ulo/src/redact.rs` has no `Deserialize` impl (Probed by grep; the `serde` feature on `ulo` from E1 is still to add). Note also that `Secret<T>` in the core derives `Clone, PartialEq, Eq` and registers through `register_secret`, which downcasts only `Secret<String>` (`crates/ulo/src/module/def.rs:410-414`, Read); a `Secret<u16>` is registered only through `m.secret(..)`.

## 7. The repo's renderer: can a multi-line configuration error survive?

Read:

- `ModuleDef::try_value` at `crates/ulo/src/module/def.rs:190-207`: on `Err`, `E: Into<BoxError>` is boxed whole into a `ValueFailure { key, error, location }` pushed on `node.failures`; the `Display` of `E` is untouched at this point.
- `wire` at `crates/ulo/src/graph/wire.rs:307-312` builds `WiringError::ValueFailed { module, key, error: redact(&graph.secrets, failure.error), at }`.
- `redact` at `crates/ulo/src/redact.rs:199-202` stores `scrub(secrets, chain_text(&*error))`. `chain_text` (`redact.rs:258-275`) is the error's `to_string()` plus each `source()` message not already contained in it, joined with `: `. `scrub` replaces registered secrets and strips URL userinfo. Neither truncates or touches newlines.
- The rendering at `crates/ulo/src/error/wiring.rs:445-449`: `format!("the value for `{}` in {} failed to build: {error}", ..)` is the head of `tree(..)`; `tree` (`wiring.rs:816-830`) writes the head with `f.write_str(&head)`, so its newlines pass through; `WiringErrors::fmt` (`wiring.rs:69-90`) then splits each rendered error with `lines()` and writes continuation lines indented by four spaces.

Run (`p7`, a scratch crate depending on `ulo` by path; a `Module` whose `register` calls `m.value(Secret::new("hunter2".to_string()))` and `m.try_value::<u32, _>(Err(ConfigErrors(..)))`; the `Display` of `ConfigErrors` writes a header line and then one problem per line; 65 problems, one 400 characters long) (Probed):

```
error: wiring failed with 1 error

  × the value for `u32` in Cfg failed to build: configuration is invalid (65 problems)
    http.port at app.toml:5:8: expected u16, found string
    api_key at app.toml:2:1: found [redacted] where a uuid was expected
    db.url at env DATABASE_URL: invalid URL pg://[redacted]@host/db
    retries[1] at app.json:3:18: expected u8, found 300 (out of range)
    long.key at app.toml:9:1: xxxxxxxx…(400 x, intact)
    many.0 at app.toml:0:1: missing required field
    …
    many.59 at app.toml:59:1: missing required field
    └─ recorded by `try_value` at src/main.rs:20
---- lines=68 bytes=3963   (then, with the extra problem, 69 lines)
contains hunter2: false; contains last problem: true; contains 400 x: true
Debug == Display: true
```

Findings (Probed):

- Newlines survive, every problem sits on its own line, indented four spaces; nothing is truncated at 65 problems and a 400-character line (the last problem and the whole long line are present).
- A registered `Secret<String>` and URL userinfo inside the multi-line text are replaced (`[redacted]`).
- The layout is serviceable: the continuation lines sit under the head, before the `└─ recorded by `try_value` at …` item, with no tree glyph of their own. The head also reads "the value for `u32` in Cfg failed to build": the key shown is the type of the value passed to `try_value`, so a config error should carry its own header line ("configuration is invalid (65 problems)") and the module will print as the module that read the section.
- `Debug` equals `Display` for `WiringErrors`.
- The `redact` text has the form `message: cause message` when `source()` is used, so for a list of problems it is better to put them in the top-level `Display`, as the probe did, than in `source()` (the join uses `: ` and drops a cause message already contained in the text).
- The downcast path still holds: `Redacted::downcast_ref::<ConfigErrors>()` reaches the structured list (Read, `redact.rs:24-27`), so a structured accessor can be offered beside the text.

Verdict: **holds**. The design's claim ("`Display` carries every problem, one per line with its path and origin") works with the current renderer without a change.

## Summary

| Premise | Verdict | Recommended choice | Confidence |
| --- | --- | --- | --- |
| figment gives a key path and source but no line for values, and reports only the first error | Holds (Probed on 0.10.19, the latest) | Do not build on figment, as designed | High |
| TOML spans let a custom deserializer report `file:line:col` per leaf, with all errors collected | Holds, with limits (Probed, about 200 lines) | Build the origin tree over `toml` 1.x `DeTable`/`DeValue` spans; collect type, range and missing-field problems by placeholder and retry; a failure inside a leaf's own parser (`IpAddr`, `Url`) stops the pass at the first | High that it works; Medium on the single-error limit being acceptable |
| JSON has no maintained value-span parser, so JSON is file plus path only | Does not hold: `jsonc-parser` 0.34.0 is maintained and gives ranges (Probed) | Use `jsonc-parser` in strict mode behind a `json` feature; same line and column as TOML. Keep file plus path as the fallback if the dependency is refused | High |
| `serde-saphyr` gives locations, else `serde_norway` | Partly: `serde-saphyr` gives line and column and a snippet but no path, and needs Rust 1.89, above the 1.88 floor; `serde_norway` gives path and line but carries `unsafe-libyaml-norway` and a ten-month gap (Probed) | Parse YAML with `saphyr` `MarkedYaml` (0 `unsafe` in src, MSRV 1.85, per-node line and column) into the same origin tree; no serde YAML crate. Fallback: `serde_norway` with its `unsafe` and floor stated | Medium (anchors, merge keys and YAML 1.1 scalars not probed) |
| `serde_yml` carries an advisory | Holds: RUSTSEC-2025-0068, unsound and unmaintained, no patched version (Read) | Never depend on it | High |
| `dotenvy` `from_path_iter` reads without setting the environment | Holds for the environment; fails for the design's subset: it expands `$VAR` from the file and from the process environment, cannot disable it, and gives no line numbers (Probed) | Write the roughly 40-line `.env` subset parser in the crate (comments, `export`, quotes, no expansion, line numbers) | High |
| `Secret<T>` via `deserialize_newtype_struct("Secret", ..)` is transparent | Holds on serde_json, toml, serde-saphyr and serde_norway (Probed) | As designed | High |
| The custom deserializer records exactly the secret leaves | Holds for plain structs, `Option`, `Vec`; **fails under `#[serde(flatten)]` and `#[serde(untagged)]`**, which register nothing and report nothing (Probed) | Document the two attributes as unsupported around a `Secret`, or add a scoped collector written by `Secret::deserialize` itself; render no found value for a secret path | High on the failure; Medium on which remedy |
| Core `Secret` already implements `Deserialize` | Does not (Probed by grep) | Add the `serde` feature on `ulo` (extension E1) | High |
| A multi-line `Display` survives `try_value` and the `WiringErrors` renderer | Holds: no truncation at 65 problems and a 400-character line, newlines kept and indented, secrets and userinfo scrubbed (Read and Probed, `p7`) | Put the problem list in the error's own `Display`, with a header line; offer the structured list through `downcast_ref` | High |

## Surprises

- `jsonc-parser` makes the JSON fallback in `RESPONSE.md` unnecessary.
- `dotenvy` expands variables from the process environment on the iterator path, with no switch, which breaks the "never read the process environment unless chosen" rule; and its errors have no line number.
- `serde(flatten)` and `serde(untagged)` silently defeat the name-based `Secret` capture.
- `serde-saphyr` 2.0.0 has an MSRV of 1.89 and `saphyr` 0.2.2 gives YAML positions without it.
- The crate's own error text would leak a secret's value (the out-of-range case) unless it consults the recorded secret paths.
