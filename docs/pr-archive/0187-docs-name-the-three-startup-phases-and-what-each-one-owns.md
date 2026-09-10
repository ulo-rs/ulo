# #187 — docs: name the three startup phases and what each one owns

Merged 2026-08-24 into `master` from `docs/name-the-startup-phases`, commit [`44bd4dd`](https://github.com/ulo-rs/ulo/commit/44bd4dde65cdbd34aa92cf3579d3b896f9dcd3d9).

`ToniApplication` carried no type-level documentation, so the boundary between building an application and serving one was only inferable from reading the methods. It is now stated on the type:

- **`create`** resolves the module graph, constructs its providers, and runs their `on_module_init` hooks — where an integration confirms the **outbound** dependencies it declares will answer.
- **`bind`** runs `on_application_bootstrap`, then takes the **inbound** sockets this process serves on.
- **`run`** drives the serve loops until shutdown.

Outbound versus inbound is the line that holds. Describe-versus-acquire does not: `create` contacts the network deliberately, because ADR-0026 put the database startup check in `on_module_init`.

The doc also records why `bind` and `run` are separate, which is otherwise only visible as a consequence: a caller can read `BoundAdapters` before anything is served — the address the OS assigned for port 0, a readiness gate opened once the sockets are live, a socket handed to a replacement process. Fused, a bound address would be unobservable until the process was already serving.

`create_with` gains a sentence pointing at it, since that is where a reader starts.

### `discover_gateways` and `discover_rpc_controllers` stay in `bind`

They were a candidate to move, on the grounds that container reads are description rather than acquisition. Under the boundary as stated they belong where they are: discovery works out what to wire, immediately before wiring it, and is the input to registration rather than a separate concern. `bind` also runs the bootstrap hooks, which are neither description nor socket-taking, so a rule that admitted only socket work would misdescribe it.

Documentation only.
