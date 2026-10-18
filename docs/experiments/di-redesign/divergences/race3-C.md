# Divergences: race 3, area C (`ulo-config`)

Every place area C departs from `integrations/DESIGN.md`, `BUILD_PLAN_3.md` or the fourth and fifth
responses, or fills a shape they leave open, and the four points the plan hands to C. Each entry
gives what the design says (or that it is silent), what the code does, and why. All await the
user's sign-off.

No cargo was run. Library facts are read from the sources under `~/.cargo/registry/src` (`toml`
1.1.8, `jsonc-parser` 0.34.0, `saphyr` and `saphyr-parser` 0.2.2, `serde_core` 1.0.229). What only a
compile or a run can show is under "Not verified".

C wrote 15 files: every file of the plan's C table. `Cargo.toml` and `src/lib.rs` are unchanged from
the skeleton, which already matched the plan. No public signature changed and none was added. The
signatures that did change are `pub(crate)` internals the skeleton drafted: `NodeDeserializer`'s
fields and `DeError`'s fields in `de.rs`, `Loaded`'s fields in `config.rs`, and `Value` in `tree.rs`
(entries 6 to 8).

Replaced on `master`: nothing is carried. `ConfigModule<T: Config>`, `FromEnv`, `Validate`,
`ConfigService<T>`, `ConfigServiceFactory`, `Environment`, the `dotenv`-crate loading and
`#[derive(Config)]` are gone, as S-skel emptied the crate. The retired `NODE_ENV`/`APP_ENV`
environment switch has no counterpart: an application picks its files in `main`.

## The four points the plan hands to C

**P-C1. How a section's problems are prefixed with its path.** The design says the section read
prefixes them and does not say how. The deserializer starts at the section's path (`Section::read`
passes `path` as the root of the pass), so every path it records is absolute: `database.url`, never
`url`. A problem a store's `validate` writes is relative to the options, and prefixing it is the
caller's: D1's `under` in `ulo-db/src/module.rs` already does so through `Problems::iter` and
`Problems::at`. C adds no prefixing method to `Problems`.

**P-C2. How `ConfigModule` learns which leaves each section read consumed.** `Config` keeps one
`Mutex<BTreeSet<String>>` on the shared `Loaded`, so every clone of a `Config` sees the same set.
Each typed read collects the dotted path of every leaf its deserializer consumed on its passes
(`a.b`, `a.hosts[1]`, the format `Tree::leaves` uses) and adds them after the last pass. A leaf is
consumed when a typed request reads it. A key the type has no field for reaches `ignored_any`,
which consumes nothing, so it stays unread; a `HashMap` field or a `serde_json::Value` consumes
every leaf under it. `ConfigModule`'s init hook runs at `connect`, after every module has
registered and read its section, and compares `Tree::leaves()` with the set. It logs each leaf not
in the set at `warn` with its path and origin, or returns a `ConfigError` listing them under
`.deny_unread()`. The hook never prints a value.

**P-C3. How `Defaults`, built before the application's type is known, reaches the tree.** It does
not, and this is a divergence from "from `Default`". `Section::read` and `ConfigModule::new` are
bounded by `DeserializeOwned` alone, so the read cannot call `Default::default()`, and the
signatures are frozen. `Defaults` reads as a layer with no values and sets `Loaded::defaults`,
which nothing reads (`#[allow(dead_code)]`, with the reason beside it). A type that wants defaults
writes `#[serde(default)]` on the type or its fields; serde calls `Default` itself, and a section
absent from every source is read as an empty table so such a type loads. `Origin::Default` is never
produced for the same reason. Giving `Defaults` the meaning the design states needs `T: Default` on
`Section::read` and `ConfigModule::new`, or a typed `Defaults::of::<T>()`; that is a signature
change for the spine's owner to decide. Until then `.source(Defaults)` is accepted and inert.

**P-C4. How a merge key is told apart in `MarkedYaml`.** It is not told apart there. `saphyr`
resolves a scalar before the reader sees it and keeps no style, so a plain `<<` and a quoted `"<<"`
look alike. The R2 pre-scan already runs over `saphyr-parser`'s events, which carry the style, so
the same scan tracks whether the next child of a mapping is a key and records a problem naming the
line when a plain-style scalar `<<` arrives in key position. A quoted `"<<"` stays a legitimate
string key. `yaml::prescan` therefore reports merge keys beside the alias bound.

## Divergences and fills

1. **A variable name or a `.env` key becomes a lowercase path.** The design shows
   `Env::process().prefix("APP_").separator("__")` and does not say how a name maps. The code
   removes the prefix, splits at the separator, drops empty segments and lowercases each segment:
   `APP_HTTP__PORT` is `http.port`. Reason: Rust fields are lowercase and variable names are
   upper-case by convention, and figment, the library the design measures against, lowercases too.
   A name left empty by the prefix is ignored. `Values` paths are not lowercased and are split at
   `.` only (no `[n]` index).

2. **Environment and `.env` values are untyped text.** They enter the tree as `Value::Text`, and a
   typed read parses the text as the number, boolean or float the field asks for (`"8080"` as a
   `u16`, `"true"` as a `bool`). A failed parse is the same `expected u16, found a string` problem a
   file gives. Under `deserialize_any` (a `serde_json::Value`, `#[serde(flatten)]`,
   `#[serde(untagged)]`) the text stays a string, since the request does not say what it should
   be, so a number in a flattened struct cannot come from the environment. An array cannot come
   from the environment either.

3. **Source failures and typed problems are both reported.** The design says the `Err` carries
   "every source failure, every value missing, ill-typed or invalid". The code does that. A file
   that failed to parse also leaves its keys absent, so a required field it would have supplied is
   reported as missing beside the parse error. Suppressing the typed pass after a source failure
   would hide real problems in the other sources; the order, source failures first, lets a reader
   stop at the first line.

4. **Rules the design leaves open for the readers.**
   - JSON: a key written twice in one object is a problem naming the first line; an empty file is a
     problem; the top level must be an object.
   - YAML: a file with more than one document is a problem; an empty file is an empty tree; a top
     level that is not a mapping is a problem; a user tag is ignored and the node read as untagged;
     a key must be a scalar. `saphyr`'s loader already refuses a duplicate key.
   - TOML: a date or time is read as the text TOML wrote it.
   - TOML, JSON, YAML: an integer is held as `i128`, so every `u64` and `i64` survives; one past
     `i128` is a problem.
   - A UTF-8 byte-order mark at the start of a file is skipped (the first line's columns are then
     counted from the byte after it).
   - A parser's own error text is not reused when it quotes the source: `toml`'s `Display` prints
     the offending line, so the reader keeps `Error::message()` and the span alone.

5. **`.env` grammar fills.** The design's table fixes the forms. The parser also accepts spaces or
   tabs around `=` (the probe found `dotenvy` refusing `KEY = value`; refusing it was judged hostile
   for no gain), requires a key of letters, digits, `_`, `.` and `-`, refuses an unknown escape in
   double quotes and an unclosed quote as a problem naming the line, and accepts a comment after a
   closing quote only when whitespace precedes it. `KEY=a#b` keeps the `#`. Every problem
   carries the key where it has one and the line as its origin; the origin has no column, which
   `Origin`'s `Display` omits when the column is zero (entry 6).

6. **`Origin`'s `Display` omits a zero line or column.** `Origin::File { line: 0, .. }` prints the
   path alone and `column: 0` prints `path:line`. A `.env` value has a line and no column, and a
   failure of a file as a whole (not found, not UTF-8) has neither. The variants and fields are
   unchanged; the doc comment says what zero means. A failure inside a source carries
   `Origin::File { path, line: 0, column: 0 }` when the reader gave it no position, so the report
   still names the file.

7. **The tree holds `Value::Integer(i128)` and a `Value::Text` variant** where the skeleton drafted
   `Integer(i64)` and no `Text`. `Tree` gained `set(segments, node)` (used by `Env`, `DotEnv` and
   `Values`), and `tree.rs` gained `join_key` and `join_index` for the one path format the
   deserializer, `get` and `leaves` share. `Config` gained `pub(crate)` `mark_consumed` and
   `unread`. All are `pub(crate)`.

8. **`NodeDeserializer` and `DeError` are restructured.** The skeleton drafted
   `NodeDeserializer { node, path }`; it now holds the pass's context and a target that is either a
   node or an absent value. `DeError` gained a kind and a recorded flag. The skeleton's
   `forward_to_deserialize_any!` list is replaced by an explicit method for every `Deserializer`
   method, because the missing-field retry and the placeholders need each request's kind. Both
   types are `pub(crate)`.

9. **R3's rendering, as written.** `invalid_type` prints `expected {expected}, found {kind}`;
   `invalid_value` prints `invalid value: expected {expected}, found {kind}`; `unknown_variant`
   prints the list of variants and not the variant found; `Unexpected::Other`, which serde's own
   defaults fill with the value (`integer `123``), prints `an unsupported value`. A fixed-width
   integer out of range is caught before the visitor sees it, so it reads `expected u8, found an
   integer out of range` and the pass goes on. The design's example text for a range problem
   (`found 300`) is not reproduced, because that is the value. Under a path recorded by name as a
   secret's, a leaf type's own `Error::custom` message is replaced by `invalid value, found
   {kind}`; elsewhere it is printed as written (R3's stated limit).

10. **Missing fields, placeholders and the stated limits.** A mismatch the deserializer detects
    (type, range, an array where a table is asked for) is recorded and answered with a placeholder
    of the requested type, so the derive goes on. A missing field ends the derive's pass over its
    struct, so it is recorded, remembered, and the pass reruns with the field supplied until a pass
    finds nothing new (at most 2048 passes). Consequences, beyond P7's one leaf-type error per
    section: a missing field whose type is an enum with no placeholder (an externally tagged enum
    read from nothing) ends the discovery, so fields after it are not reported in that run; and a
    section whose root is a scalar read as a table reports the mismatch once and none of the
    missing fields below it.

11. **Secrets.** `Section` holds the texts from two records: the by-name record (`Secret` newtype
    name, the leaf's text and every scalar under it, booleans and empty strings skipped) and the
    collector's `CollectedSecrets::texts()`. The collector is `ulo::__private::collect_secrets`,
    installed around each pass. A repeat is harmless: the registry skips it, and the code sorts and
    deduplicates before building the `Secret<String>` list. `Section::register` calls `m.secret`
    for each text on success and on failure, so a secret in a section that failed to load is still
    scrubbed from later errors.

12. **`ConfigModule`.** `validate` runs only on a value that loaded; a failed load reports the load
    problems and not the hook's. A hook finding is a `ConfigError` headed `configuration is invalid`.
    A section's header reads `section `database` is invalid`, the whole configuration's
    `configuration is invalid`. `Problems`' own `Display` uses the second header.

13. **`Display` format.** `header (N problems)`, then one problem per line as `path at origin:
    message`, with the path or the origin left out when absent. No indentation is added: the
    renderer indents the continuation lines (PROBES_config.md §7).

14. **`Env::process()` skips a variable whose name or value is not UTF-8,** where `std::env::vars`
    would panic.

15. **YAML alias count (R2).** The scan counts every node built, aliases at their anchor's size,
    and refuses at the first alias after which the total passes 100,000, naming that alias's line.
    It resets at each document. The "events lack anchor IDs or positions" fallback is not written:
    the events carry both (`Event::Alias(id)`, the anchor ID on `Scalar`, `SequenceStart` and
    `MappingStart`, and a `Span` per event), read in `saphyr-parser` 0.2.2. Line and column come
    from the marker: lines count from 1 and columns from 0, so the code adds one to the column.

## Requests to other owners

None. Two notes for the coordinator:

- `Defaults` (P-C3) needs a signature decision from the spine's owner before it can mean anything;
  the code is ready to read a `Default` value into the tree once a type is in hand.
- D1's `under` helper prefixes `Store::validate`'s problems; `ulo-config` offers no method for it,
  and none is needed.

## Not verified

No compile was run, so every type, borrow and trait-resolution claim below is read, not shown.

- That `T::deserialize` resolves through the `DeserializeOwned` bound with `Deserialize` imported
  (`de.rs`, `section`).
- That `match` on a tuple holding a `&mut` borrow and a moved value compiles in `tree.rs::merge` as
  restructured, and that the bind-by-move guard in `de.rs::section` is accepted.
- That `macro_rules!` `int_method!` expands inside the `Deserializer` impl with `'de` unhygienically
  resolved (serde's own `forward_to_deserialize_any!` relies on the same).
- That `impl SpannedEventReceiver for Scan` does not overlap `saphyr-parser`'s blanket impl over
  `EventReceiver` (coherence on a local type).
- Behaviour never run: the alias bound against a nested alias bomb, the merge-key detection, the
  YAML 1.2 scalar resolution of `yes`/`0o52`, the `flatten` and `untagged` paths around a failing
  `Secret`, the missing-field rerun on a nested struct, and the multi-line report through the
  renderer. These are the premise checks in `BUILD_PLAN_3.md`, "After the race", item 3.
- That `Handle` and `ModuleHook`, returned by `try_value` and `on_init`, carry no `#[must_use]`
  that the workspace denies.
