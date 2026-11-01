Design the integration layer of a Rust application framework, on top of the dependency-injection core and the transport layer you already designed (the attached `DESIGN.md` and `transports/DESIGN.md`). Both are settled and built; they compile, and the transport layer has conformance suites. Below is what the integration layer must be able to do and the constraints it lives under. Nothing here describes an existing implementation; design it from these requirements.

**The foundation.** An integration is a module of the DI core: it provides bindings, takes part in the startup phases (`wire`, `connect`, `listen`) and the shutdown sequence (drain, then close), and reads configuration the application gives it. It uses the transport layer's public contracts where it serves traffic (HTTP routes, gRPC services) and its own conformance-suite practice (hosts and links declare capabilities, a shared suite proves them, every scenario is shown to fail against a planted break). Extend the core's SPI where a requirement needs it, and say where.

**Constraints**
- Stable Rust 1.88, edition 2024, for crates that can hold it; a crate whose library needs more says so and states its own floor.
- Two levels of runtime independence apply, as in the transport layer: a runtime-free crate has no async runtime anywhere in its tree; a runtime-neutral crate may reach a runtime only through crates that are themselves neutral. Say which level each crate in the layer sits at and why.
- Runtimes are given, never found: an integration holds the runtime the application gives it and does not discover one through an ambient lookup. A library that does look one up (a pool spawning on whichever runtime is current) is entered through the application's runtime, or refused at startup when the application's runtime is not one the library supports.
- Every macro is sugar over a public value-level API.
- A permanent misconfiguration is refused at `wire`, naming what is wrong and where it came from. An unreachable dependency at `connect` is a different, retryable failure.
- Secrets are redacted by the core's existing redaction everywhere they could appear: errors, `Debug`, logs.
- Wire behaviour and file formats follow the governing specification or convention wherever one exists (Kubernetes probe semantics, `grpc.health.v1`, database URL schemes, W3C Trace Context); where a standard decides, the design does not invent. Where a candidate standard never finished (the IETF health-check response draft expired in 2022), say so and treat it as a shape, not a contract.
- Nothing on a runtime worker blocks: a blocking call (reading a file, a certificate store, a synchronous driver) is either done where nothing is being served yet, or moved off the worker, and the stall scenario applies to it.

**Capabilities**

Configuration
1. An application declares one typed configuration. An integration reads its own section by path, without knowing the application's type.
2. Sources are given, never found, and layer in the order added, each overriding the ones before: defaults, files (TOML, JSON, and YAML if a maintained serde implementation exists), a `.env` file, the environment, explicit values. Reading the process environment is something the application chooses explicitly; a test passes a map instead. Loading a `.env` file never writes into the process environment.
3. A missing, ill-typed or invalid value is refused at `wire`. The error names the field's path, the source that supplied the value (file and line, or variable name) and what was expected; every error is reported together, not the first alone. Rules a type system cannot express are added by a validation hook with the same reporting.
4. Secret values deserialise transparently and are redacted everywhere; a refusal names a secret's path, never its value.
5. Reloading while running and remote sources (a secrets manager, a configuration service) are named as deferred, with the reason.

Health
6. Liveness, readiness and startup probes with Kubernetes' meanings. Liveness never depends on integrations. Readiness is false until `listen` has finished, false from the moment the drain begins and before any transport refuses work, and false whenever a contributed check is down. Startup is true once `listen` has finished.
7. Health checks are named, async, return up or down with a redacted reason, run concurrently, and are each bounded: a check that does not answer in its bound is down, never up. A zero bound is refused; an unbounded check is an explicit opt-out.
8. An integration's check reaches readiness without the application registering it by hand, so a forgotten registration cannot leave readiness blind to a dead dependency. Say what the core needs to collect several contributions of one kind.
9. Served over HTTP (distinct paths per probe, status codes Kubernetes reads, a documented body, dependency detail off by default because it tells an outsider about internals) and over `grpc.health.v1` (including `Watch`) when gRPC is present. The core of health depends on no runtime; the services that serve it sit at the level of the hub that serves them.
10. A scenario on every host that serves health: readiness turns false at the drain before the first refusal.

Database and store integrations
11. Postgres, MySQL and SQLite through SQLx; SeaORM; Diesel; MongoDB; Redis. Prisma Client Rust is excluded: its repository was archived with a "no longer maintained" notice on 2025-03-12 and has had no change since.
12. Each is a module that provides its library's own pool or client as a binding, configured through the configuration layer. The application calls the library's own API on the library's own types; there is no query layer of ours over any of them.
13. The connect, retry, close and health policy is written once and shared; each library supplies how to connect, how to ping and how to close.
14. `wire` refuses a malformed URL or option, redacting any secret in it. `connect` retries with backoff under a total bound and fails the app's start with a retryable error distinct from misconfiguration. Connecting lazily, with readiness false until the first success, is an explicit opt-in.
15. Pools close after every transport has drained, because in-flight work may still use them, under a close bound. Say whether the core already runs close in reverse dependency order, and extend it if it does not.
16. An application can use more than one database of the same kind, each separately named and injectable.
17. Migrations never run implicitly. Running them at start is an explicit option, off by default; a failed migration fails the start as a misconfiguration, not as retryable. Where a library's migrator cannot guard against several replicas starting at once, the option is refused for that library with the reason.
18. The libraries are mostly tokio-based and are called from the user's handler code on whichever thread that runs on. Say how a module that needs tokio refuses an application running another runtime, and how a library that spawns at pool creation is made to spawn on the application's runtime. Where one library's version depends on another's (SeaORM on a specific SQLx), say how the pair is kept in step, and what happens when an application holds two.
19. Diesel is synchronous. Decide how its queries avoid blocking a worker (an async Diesel crate, a blocking pool the module owns, or a core extension) and justify the choice.
20. A conformance suite that every database module is held to, run against real containers (a file for SQLite). It covers: a malformed URL refused at `wire` with secrets redacted; an unreachable database failing `connect` at its bound as retryable; lazy connect keeping readiness false; the health check going down in an outage and recovering; the pool closing after the transports drain, with a query in flight at the drain completing; connecting not stalling a one-thread runtime; an unsupported runtime refused at `wire`; two named instances staying separate; migrations running only on request and once across concurrent replicas.
21. Transactions scoped to an execution, request-scoped providers, and per-test databases are named as deferred, with the reason.

Command line
22. `ulo new <name>` creates a project from templates embedded in the binary, with no network fetch: one per transport, plus an optional database choice. The generated project depends on the exact version of the CLI that generated it.
23. CI builds every template and combination, runs the generated project's tests, and fails a template whose tests run nothing. Generated code passes the generated-code lints.
24. `ulo generate` writes new files for a module, provider, controller, gateway or resolver and prints the line that registers each; whether it edits existing source is decided and justified (rewriting a user's file loses their formatting and comments).
25. `ulo dev` is unchanged by this layer; its Windows gap stays recorded.

Observability
26. Trace context propagates by W3C Trace Context (`traceparent`, `tracestate`) across every transport the framework serves and calls, through the execution's `.within`, so a span opened in one handler is the parent of the calls it makes. Spans are exported through OpenTelemetry. Say what the core needs, what is on by default, and how an application turns it off. This is the last piece of the layer.

**Out of scope, named:** a gap pass against NestJS and the wider Rust ecosystem (OpenAPI, SOAP and the rest) comes after the switch to the new API.

**What to deliver**
- The crate layout of the integration layer, the runtime level of each crate, and every extension to the core's SPI.
- Per integration: the user-facing API with a short example, what is refused and where (compile time, `wire`, `connect`, runtime), and the behaviour at each startup and shutdown phase.
- How a new database or store module plugs into the shared policy, written without macros.
- For each place a standard decides, which one, and what the design takes from it.
- An order of build, with the reason for each dependency between crates.
