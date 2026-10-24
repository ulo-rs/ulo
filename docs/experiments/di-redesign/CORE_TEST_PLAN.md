# Core test series: plan

Source: `integrations/RESPONSE.md`, ninth response. One test per capability in `CAPABILITIES.md`
(48), each shown failing against a planted break; every `WiringError` variant and every §12 refusal
gets a test at its listed point. Assert variants and fields, not message text, except §10.1's
short-name/collision rule and the help lines.

State at `188ac7ed`: `crates/ulo` has no tests of its own. Existing cover is incidental
(`ulo-runtime-tokio/tests/app.rs` for the clock, runtime, close and serve-death paths;
`ulo-db-conformance` for keyed modules and contribution through a store; `ulo-codegen-tests` for
gRPC enhancer tiers). `ulo-codegen-tests/tests/ui` holds only the gRPC-shape and `meta` compile-fail
cases: no §12 compile refusal is pinned.

## 0. Home (probe result)

`crates/ulo/tests/` works. `ulo-runtime-tokio` (and `tokio` with `rt`, `macros`) is a dev-dependency of
`crates/ulo`; `cargo test -p ulo --test probe` built `TestApp::of(Empty).runtime(Tokio::current())`,
connected and closed it. `Tokio` satisfying `TestApp::runtime(impl Runtime)` shows the test links
the one `ulo` the runtime crate was built against. See the report for the other checks.

Conventions for every file under `crates/ulo/tests/`:

- A shared `tests/common/mod.rs` (`mod common;` per file): `Empty`, a `module!`-free hand-written
  `Module` helper (`struct M(&'static str, fn(&mut ModuleDef))` with an identity from a name), `wire_err(builder) -> WiringErrors`
  that asserts `wire()` fails and returns the entries, `connect(builder)`, a `Probe` recording
  construction/hook order in an `Arc<Mutex<Vec<&'static str>>>`.
- Tests use the value-level API only (no macros; macros stay in `ulo-codegen-tests`).
- A planted break is a one-line change in `crates/ulo/src` that the test must fail under; each row names it. The
  builder agent applies it, runs the one test, records the failure, reverts. Planted breaks never stay.
- `#[tokio::test]` with `Tokio::current()` only where a hook, constructor or timer wait needs a
  runtime; wiring tests are plain `#[test]` over `wire()`.

## 1. Capabilities

Verdict column: pinned / partly / none. Judged by reading what the existing test asserts, not its name.
Area letters are the batch areas in section 5.

| # | Capability | DESIGN | Existing test | Verdict | Proposed test (file :: name) and assertion | Planted break |
|---|---|---|---|---|---|---|
| 1 | Declare a service the container builds | §4, §3.4 | `ulo-codegen-tests` build services via `#[injectable]` as fixtures; none asserts injection | **B2 built** | `bindings.rs :: a_construct_type_is_built_with_its_dependencies` (declared before its dependency's factory, so only the declared field edge orders the build); `injection.rs :: field_ctor_and_factory_sites_resolve_alike` for the field spelling | make `provide` skip dependency resolution |
| 2 | Bind a value under a key | §4 | none (`Values` in a template test only) | **B2 built** | `bindings.rs :: a_value_is_read_back_by_its_key` (each value under its own type; every read and a holder share the one `Arc`) | return a clone |
| 3 | Async factory, parameters are dependencies | §4 | none | **B2 built** | `bindings.rs :: a_factory_s_parameters_are_its_dependencies_and_its_output_the_binding`, `a_fallible_factory_binds_its_ok_type`, `a_fallible_factory_that_errs_fails_connect_with_the_binding_named` (`ConnectError::Construct`, `FailureReason::Errored`) | feed the factory a default instead of the resolved dep |
| 4 | Implementation under a trait, injected by trait | §4 | none | **B2 built** | `bindings.rs :: an_implementation_is_injected_by_its_trait` (`dyn Repo` exported alone: the concrete key is `NotFound` outside the module, both keys reach one object inside it) | bind under the concrete key |
| 5 | Several bindings of one type, told apart | §3.1, §4 | `ulo-db-conformance :: keyed::two_instances` (qualified, through a store) | **B2 built** | `bindings.rs :: qualified_bindings_of_one_type_are_separate` (`Dep<T, Primary>` vs `Dep<T, Replica>`; the unqualified read is `NotFound`) | collapse qualifier to `()` in the key |
| 6 | Alias | §4 | none | **B2 built** | `bindings.rs :: an_alias_reads_the_same_instance` (`Arc::ptr_eq` through both keys, the target built once) | alias builds a second instance |
| 7 | Collect contributions as a list, from several modules | §4, §7 | `ulo-db-conformance` (contribution through a store, one module) | **B2 built** | `bindings.rs :: contributions_from_two_modules_form_one_many` (import order, then the module's own), `a_contribution_is_a_value_a_type_or_a_factory_output`; the global-guards case stays in `enhancers.rs` | drop the second module's contributions |
| 8 | Third-party type, neither annotated nor trait-implementing | §4 | `ulo-db-*` bind pools (not asserted at core level) | **B2 built** | `bindings.rs :: a_foreign_type_is_bound_by_value_and_by_factory` (`Mutex<Vec<u8>>` by value, `HashMap` by factory) | require `Construct` in `value` |
| 9 | Keys are compiler-checked, no string keys | §3.1 | none | none | `ulo-codegen-tests/tests/ui/key_is_a_type_not_a_string.rs` (trybuild): `m.value::<&str>("x", ..)` / `get("name")` does not compile; plus `bindings.rs` need not exist. Check `Key` has no `From<&str>` (API-surface assertion in the ui test) | add a `From<&str> for Key` (the ui test must then compile and fail the suite) |
| 10 | Second single refused; single + collection mix refused | §4, §10.1 | none | **B1 built** | `wiring_bindings.rs :: duplicate_single_binding`, `duplicate_single_through_a_second_key` (`DuplicateBinding`); `single_beside_collection`, `a_collection_is_app_wide_so_a_single_in_another_module_mixes_with_it` (`KindMix`); controls `qualifiers_and_modules_keep_equal_types_apart`, `a_collection_of_many_contributions_and_a_differently_keyed_single_are_not_a_mix` | last binding wins silently |
| 11 | Fields, ctor params, factory params follow one rule | §5 | codegen fixtures only | **B2 built** | `injection.rs :: field_ctor_and_factory_sites_resolve_alike` (`Dependencies::field`, `::param` and a factory parameter, all declared before the dependency's factory) | resolve factory params through a different path |
| 12 | Site reads service, trait object, collection, or per-execution data | §5 | none | **B2 built** | `injection.rs :: a_site_reads_each_of_the_four_shapes` (`Dep<S>`, `Dep<dyn T>`, `Many<dyn T>`, `Ext<U>` in one execution-scoped factory) | `Many` resolves to the first element only |
| 13 | Optional dependencies | §5 | none | **B3 built (the run-time half)** | `injection.rs :: an_optional_dependency_is_none_when_unbound_and_some_when_bound`, `an_optional_extension_is_none_until_something_writes_it` | `Option` reported as `Missing` |
| 14 | Owned state beside dependencies | §5 | none | **B2 built** | `injection.rs :: a_constructor_sets_owned_state_beside_its_dependencies` (a transient `Construct` type: state derived from the dependency, an id of its own, the dependency shared) | share owned state through the cache |
| 15 | Fallible and async constructors report a startup error | §3.4, §10.2 | `ulo-runtime-tokio` none; `ulo-codegen-tests/clients.rs :: a_uri_that_does_not_parse_fails_wiring` (value path) | **B2 built** | `injection.rs :: a_failing_constructor_is_a_startup_error_naming_the_binding`; `bindings.rs :: a_fallible_factory_that_errs_fails_connect_with_the_binding_named`. `Panicked` and `TimedOut` stay with B4's `construct_errors.rs` | swallow the `Err` and bind a default |
| 16 | A site is read by type: alias or renamed import reads the same binding | §3.1 | none | **B2 built** | `injection.rs :: a_type_alias_and_a_renamed_import_read_the_same_key` (`type Aliased = left::Pool; use left::Pool as Renamed`, beside a `right::Pool` of the same short name) | key by `type_name` string |
| 17 | Every holder holds the same object; services need not be `Clone` | §3.3 | none | **B3 built** | `scopes.rs :: two_holders_share_one_non_clone_singleton` (`Arc::ptr_eq` through two holders and the lookup; a `Mutex` mutation and an init hook's effect visible to the other) | clone-on-resolve |
| 18 | Singleton: built once at startup, shared | §3.3, §6.1 | none | **B3 built** | `scopes.rs :: a_singleton_is_built_once_at_connect_and_shared`, `an_auto_provider_that_reads_no_execution_data_is_a_singleton`, `singletons_reading_a_transient_each_hold_their_own` | rebuild per get |
| 19 | Execution scope: once per execution, shared inside; WS connection not an execution | §6.1, §6.3 | `ulo-ws-hyper/tests/handshake.rs` (WS per-message, transport level) | **B3 built** (core half) | `scopes.rs :: an_execution_scoped_binding_is_built_once_per_execution`, `an_execution_scoped_binding_cannot_be_read_without_an_execution`, `execution_scoped_instances_are_released_when_the_last_handle_drops`; the WS-connection half stays in `ulo-ws-hyper` | share across executions |
| 20 | Transient: fresh at every site | §6.1 | none | **B3 built** | `scopes.rs :: a_transient_is_built_at_every_site`, `singletons_reading_a_transient_each_hold_their_own` | cache transients |
| 21 | Singleton on execution-scoped refused at startup | §6.2 | none | **B3 built** | `wiring_scopes.rs :: a_singleton_reading_execution_data_is_refused`, `a_singleton_reaching_execution_data_through_a_transient_is_refused_once`, `a_singleton_reading_an_execution_scoped_binding_is_refused`, `a_factory_singleton_is_refused_by_closure`, `an_auto_provider_reading_execution_data_is_refused_by_type_and_by_closure`, `a_controller_declared_singleton_that_reads_execution_data_is_refused`; control `bindings_built_in_an_execution_may_read_execution_data` | skip the needs-execution pass |
| 22 | Controller depending on execution-scoped is built per call | §6.2, §7 | none | **B3 built** | `scopes.rs :: a_controller_reading_execution_data_is_built_per_call`, `a_controller_reaching_execution_data_through_a_dependency_is_built_per_call`, `an_auto_controller_that_reads_no_execution_data_is_built_once`, `an_explicitly_per_execution_controller_is_built_per_call_though_it_reads_nothing` (called through `dispatch` over `AppHandle::mounted`) | build once |
| 23 | Execution carries cache, extension bag, cancellation, deadline; context injectable | §3.8, §6.3 | `ulo-codegen-tests/deadlines.rs` (gRPC deadline propagation) | **B3 built** | `executions.rs :: an_execution_scoped_service_reads_the_bag_and_the_execution_it_was_built_in`, `cancel_fires_the_signal_once_and_the_first_reason_stays`, `completing_an_execution_fires_no_cancellation`, `the_deadline_is_carried_and_fires_cancellation_on_the_apps_timer`, `without_a_timer_a_deadline_is_carried_and_never_fires`, `an_execution_opens_through_a_module_handle_with_that_modules_visibility` | deadline dropped from `ExecOptions` |
| 24 | Enhancers, gateways take part in the container, singleton or per execution | §7 | `ulo-codegen-tests/enhancers.rs` (gRPC) | partly | `enhancers.rs :: a_guard_reads_a_dep_and_is_per_execution_when_it_reads_one`: guard singleton built once; guard reading `Ext` built per call | build every enhancer as singleton |
| 25 | Role comes from the implemented trait, per transport | §7 | none | none | `enhancers.rs :: an_http_guard_and_an_rpc_guard_are_different_roles`: a type implementing only `Guard<A>` registers under `AnyGuard<A>` and not `AnyGuard<B>`; handler on B does not run it (test transports `A`, `B`) | register under every transport |
| 26 | Declared by type / value / closure; closure scope rule; global, controller, method order | §7 | `ulo-codegen-tests/enhancers.rs :: an_impl_guard_by_type...`, `method_and_impl_interceptors_...` (gRPC tiers only) | partly | `enhancers.rs :: the_three_spellings_stack_global_controller_method_in_written_order`; `a_closure_is_built_once_unless_it_reads_an_execution`; `an_explicit_scope_overrides_the_closure_inference` | reverse tier order |
| 27 | Each guard built only after the one before admits; nothing below built until all admit | §7 | none | none | `enhancers.rs :: guards_are_built_one_by_one_and_lower_stages_wait` with the `Probe`: the second guard's construction is absent when the first refuses; no interceptor or handler built | build all guards eagerly |
| 28 | A binding under a role key registers that role | §7 | none | none | `enhancers.rs :: a_contribution_under_a_role_key_registers_the_role` (B5); the refusals half is **B1 built** in `wiring_roles.rs` (`qualified_contribution_under_a_role_key`, `also_as_on_a_provided_type_under_a_role_key`, `also_as_on_a_value_under_a_role_key`, `also_as_on_a_factory_under_a_role_key`, `an_alias_under_a_role_key`, `the_role_is_per_transport`) | ignore role keys at freeze |
| 29 | Module declares imports/providers/controllers/exports; export is the only exit | §8.1, §8.2 | none | **B2 built** | `modules.rs :: an_unexported_binding_is_invisible_outside_its_module` (`WiringError::Missing`, `near: None`), `an_export_is_the_only_way_a_binding_leaves_its_module` | all bindings visible |
| 30 | Global modules reach every module | §8.2 | none | **B2 built** | `modules.rs :: a_global_module_s_exports_reach_modules_that_do_not_import_it`, `a_global_module_s_unexported_bindings_stay_private`; control `a_module_that_is_not_global_reaches_only_its_importers` | global exports ignored |
| 31 | for_root: different config two modules, same config deduped (diamond) | §8.3, §3.6 | none | **B2 built** | `modules.rs :: one_module_type_two_configs_is_two_modules` (`AmbiguousModule`, names `Db` and `Db #2`, two registrations and two builds), `a_diamond_import_with_equal_config_is_one_module` (one registration, one build, one pool shared by both importers) | identity ignores config |
| 32 | Keyed instances, each injected by key | §8.3 | `ulo-db-conformance :: keyed::two_instances`; `ulo-codegen-tests/clients.rs :: keyed_clients_of_one_service_each...` | **B2 built** | `modules.rs :: two_keyed_instances_are_injected_by_their_keys` (`Dep<T, Primary>` vs `Dep<T, Replica>`, `module_keyed`, `Db @ Replica`), `a_keyed_module_s_export_is_read_only_under_its_key` | one key shadows the other |
| 33 | Modules built at runtime by a function | §8.4 | none | **B2 built** | `modules.rs :: a_dynamic_module_registers_what_its_function_registers` (function runs once for an equal identity, exports honoured), `a_dynamic_module_is_identified_by_owner_and_config_and_prints_its_label` | function never called |
| 34 | Re-export what an import exports | §8.2 | none | **B2 built** | `modules.rs :: reexport_passes_an_import_s_export_on` (same instance; an unlisted export stops at the re-exporting module), `a_reexport_names_one_key_and_not_the_import_s_whole_surface`; refusals in `wiring_modules.rs` | reexport widens to all imports |
| 35 | Module identity deduplicates and prints a readable name in every error | §3.6, §10.1 | `ulo-db-conformance/tests/check_names.rs` (`S::NAME @ Key`, full names on collision; store level) | **B1 built** | `names.rs :: colliding_types_print_in_full_only_for_those_types`, `a_type_prints_short_when_nothing_in_the_report_collides_with_it`, `a_collision_is_decided_across_entries_and_leaves_the_qualifier_short`, `colliding_qualifiers_print_in_full_and_the_type_they_qualify_stays_short`, `a_keys_binding_kind_plays_no_part_in_a_collision`, `a_single_entry_displayed_on_its_own_applies_the_rule_to_its_own_names`, `modules_whose_names_print_alike_print_in_full`, `a_keyed_module_prints_its_qualifier_and_a_second_configuration_its_position` (text assertions, as allowed); pure: `type_name.rs`/`key.rs` `#[cfg(test)]` | print full paths always |
| 36 | Middleware configured per module, by route | §8, transports DESIGN | `ulo-codegen-tests/enhancers.rs :: a_scoped_pre_dispatch_entry_leaves_another_service_alone` (gRPC) | partly | question for the designer (Q2); owner is `ulo-http`/`ulo-transport`, not `crates/ulo`: `ulo-http/tests/middleware_scope.rs` | scope ignored |
| 37 | Module handle for current/named module, visibility-limited lookups | §8.5 | `ulo-rpc-conformance/src/cases/app.rs` takes a `ModuleRef` as a fixture | **B2 built** | `module_ref.rs :: a_module_ref_sees_only_what_its_module_sees`, `a_module_ref_reports_a_key_its_module_sees_twice` (`Ambiguous`), `a_module_lookup_by_type_is_not_found_or_ambiguous`, `a_module_ref_inside_an_execution_builds_per_execution_bindings_once` (`ExecutionRequired`, `with_execution`), `execute_on_a_module_ref_resolves_with_that_module_s_visibility`, `meta_is_what_the_module_wrote_and_nothing_for_another_module`, `an_injected_module_ref_is_the_enclosing_module`; `module_keyed` in `modules.rs` | ModuleRef resolves app-wide |
| 38 | Lazy loading after startup | §8.6 | none | none | `lazy.rs :: load_wires_and_connects_a_module_after_startup`; `LoadError::Refused` per `LoadRefusal` variant (Controllers, Middleware, Contribution, GlobalExport, Input); `Wiring`; `Connect`; `Closed` after close | load skips visibility checks |
| 39 | Async hooks: init then bootstrap, destroy/before-shutdown/shutdown with signal; module hooks | §9.1 | `ulo-runtime-tokio/tests/app.rs` uses `on_init` to spawn tasks | none | `hooks.rs :: init_runs_before_bootstrap_and_a_failure_fails_connect`; `close_runs_destroy_then_before_shutdown_then_shutdown_with_the_signal_name`; `a_module_hook_runs_with_the_module` | swap bootstrap and init |
| 40 | Startup hooks in dependency order, shutdown reverse | §9.2 | none | none | `hooks.rs :: hooks_run_in_dependency_order_and_shutdown_in_reverse` (chain A->B->C, plus a diamond) | sort ascending on shutdown |
| 41 | Hooks on execution-scoped/transient refused at compile time | §9.1, §12 | none | none | `ulo-codegen-tests/tests/ui/hook_on_execution_type.rs` and `hook_on_transient_type.rs` (trybuild) against `Construct<Scope: HookCapable>`; plus `wiring_scopes.rs :: HooksOnPerExecution` for an `Auto` type inferred per execution | remove the `HookCapable` supertrait |
| 42 | Readiness checks with retries and timeout; failed check fails startup; credentials redacted | §9.3 | `ulo-db-conformance :: connect::unreachable_fails_at_the_bound`, `create_loop`, `lock_wait` | partly | `readiness.rs :: a_check_retries_then_fails_connect` (attempt counter, frozen timer, `ConnectError` readiness case, `FailureReason`), `attempt_timeout`, `a_secret_in_the_error_is_redacted` (`Redacted` carries no secret; assert by value not text) | retry count off by one; skip redaction |
| 43 | Phases separate: build+connect, bind, serve | §9.4 | `ulo-runtime-tokio/tests/app.rs` (listen refusal, bind phase handing) | partly | `phases.rs :: phases_advance_wired_connected_bound_running_closed`: `phase()` at each point; no socket before `listen`; a server's `prepare` not called at `connect` | bind during connect |
| 44 | Every wiring error before any instance built, all in one pass | §10.1 | none | **B1 built** | `wiring_pass.rs :: all_errors_arrive_in_one_pass_and_nothing_is_built` (five faults across steps 1-4 and 6, five entries in step order, `Probe` empty), `errors_are_reported_in_step_order_whatever_the_registration_order`, `unrelated_missing_bindings_do_not_hide_each_other`, `a_missing_piece_skips_only_its_own_edges`, `a_graph_without_faults_wires_and_builds_nothing_either` | stop at first |
| 45 | Compile-time where feasible; nothing panics or exits; typed error | §10, §12 | `ulo-runtime-tokio/tests/app.rs :: a_transport_whose_serve_fails_closes_the_app` etc. (serve death) | partly | `startup_errors.rs :: no_failure_path_panics`: wiring, connect, listen failures each return `StartupError::{Wiring, Connect, Bind}` (matched, not text); a panicking constructor/hook is `FailureReason::Panicked` | `unwrap` in connect |
| 46 | Compile error points at the site, says what to write | §10.3 | `ulo-codegen-tests/tests/ui` (gRPC and meta only) | partly | `ui/*.stderr` for each compile row in section 4; assertion is the `.stderr` snapshot (the allowed text exception: help lines) | change the `on_unimplemented` note |
| 47 | Runtime lookups from app/context/module handle, typed errors | §6.3, §10.2 | `ulo-runtime-tokio/tests/app.rs` (`NotFound` for `Dep<dyn Runtime>` only) | **B3 built** | `lookup.rs :: an_unbound_key_and_an_unregistered_module_are_not_found`, `a_singleton_read_before_the_walk_built_it_is_not_ready`, `a_key_asked_for_as_another_type_is_wrong_type_and_builds_nothing`, `a_single_read_as_a_collection_and_a_collection_read_as_a_single_are_wrong_kind`, `an_execution_scoped_binding_needs_an_execution_from_the_app_and_a_module_handle`, `a_module_handle_sees_only_what_its_module_sees`, `a_key_two_exports_answer_is_ambiguous_from_a_module_that_sees_both`, `a_module_type_with_two_configurations_is_an_ambiguous_module`, `a_failing_or_panicking_build_inside_a_call_is_a_construct_error`, `a_singleton_lookup_from_destroying_on_is_closed`; `NotFound` for an extension or input in `executions.rs` | collapse variants |
| 48 | Overrides without editing production modules | §11 | `ulo-runtime-tokio/tests/app.rs :: overriding_the_timer/runtime_is_refused...` (refusals only) | **B3 built** | `testing.rs :: override_value_replaces_the_recipe_and_the_production_factory_never_runs`, `an_override_keeps_the_origin_modules_visibility`, `an_implementation_under_a_trait_is_overridden_by_the_trait_key`, `override_factory_reads_its_own_parameters`, `an_override_drops_the_replaced_recipes_dependencies`, `override_try_factory_builds_on_ok_and_fails_connect_on_err`, `override_many_replaces_a_whole_collection_and_qualified_targets_the_qualified_one`, `qualified_targets_the_qualified_binding_only`, `in_module_replaces_only_that_modules_binding_and_everywhere_replaces_both`, `in_module_of_and_in_module_keyed_name_one_instance`, `replace_module_swaps_an_import_for_a_superset_of_its_exports` | override changes the origin module |

Tally: pinned 0, partly 18 (5, 7, 8, 15, 19, 23, 24, 26, 32, 35, 36, 37, 42, 43, 45, 46, 47, 48), none 30.
No capability is judged pinned: the nearest ones (clock/runtime override refusal, serve death, keyed
store modules) assert one slice of a capability through a different layer.

## 2. Wiring errors: every `WiringError` variant (`crates/ulo/src/error/wiring.rs`)

All in `wiring_*.rs` under `crates/ulo/tests/`; each calls `wire()` and matches the variant and its fields (`ModuleName`, `KeyName`, locations as `Some`/count, not text). §10.1 step in brackets.
Planted break for all: skip that check in the named pass.

| Variant | Step | Test | Asserts |
|---|---|---|---|
| `ImportCycle { path }` | 1 | **B1 built** `wiring_modules.rs :: an_import_cycle_is_refused_with_its_path`, `a_three_module_cycle_and_a_module_importing_itself_are_refused`; control `a_diamond_of_imports_is_not_a_cycle` | path is `[A, B, A]` as `ModuleName`s; no `forwardRef` escape; 3-cycle too |
| `ReexportNotVisible` | 1 | **B1 built** `wiring_modules.rs :: reexport_of_a_key_no_import_exports` | module, key |
| `ReexportAmbiguous` | 1 | **B1 built** `wiring_modules.rs :: reexport_of_a_key_two_imports_export`; control `a_reexport_of_a_key_one_import_exports_is_accepted` | `sources` has both |
| `KeyedInput` | 1 | **B1 built** `wiring_modules.rs :: a_keyed_module_may_not_declare_an_input`; control `an_unkeyed_module_may_declare_an_input` | module, key |
| `ExportNotBound` | 1 | **B1 built** `wiring_modules.rs :: export_of_an_unbound_key`, `export_of_a_key_an_import_provides_says_to_reexport_it`, `export_of_a_key_spelled_like_a_bound_one_names_the_bound_key` | `imported`, `near` |
| `DuplicateBinding` | 2 | **B1 built** `wiring_bindings.rs :: duplicate_single_binding`, `duplicate_single_through_a_second_key` | key, module, two locations distinct |
| `KindMix` | 2 | **B1 built** `wiring_bindings.rs :: single_beside_collection`, `a_collection_is_app_wide_so_a_single_in_another_module_mixes_with_it` | both locations |
| `DanglingAlias` | 2 | **B1 built** `wiring_bindings.rs :: alias_to_nothing`; control `an_alias_to_a_binding_the_module_sees_is_accepted` | alias, target |
| `ValueFailed` | 2 | **B1 built** `wiring_bindings.rs :: try_value_err_via_module_def_and_via_contribute` (the secret's absence from `Display` is asserted beside `downcast_ref`, plan question 9) | `Redacted` error, no secret (by value) |
| `QualifiedRoleContribution` | 2 | **B1 built** `wiring_roles.rs :: qualified_contribution_under_a_role_key`; controls `an_unqualified_contribution_under_a_role_key_is_accepted`, `a_qualified_contribution_under_a_key_that_is_not_a_role_is_accepted` | key, module |
| `SingleRoleBinding` | 2 | **B1 built** `wiring_roles.rs :: also_as_on_a_provided_type_under_a_role_key`, `also_as_on_a_value_under_a_role_key`, `also_as_on_a_factory_under_a_role_key`, `an_alias_under_a_role_key`, `the_role_is_per_transport` | one variant each |
| `DuplicateReadiness` | 2 | **B1 built** `wiring_bindings.rs :: two_readiness_checks_for_one_key`; control `one_readiness_check_per_key_is_accepted` | two locations |
| `InputConflict` | 2 | **B3 built** `wiring_inputs.rs :: transport_vs_transport_different_seeders`, `transport_vs_module`, `transport_vs_binding`; `two_transports_naming_each_other_as_seeders_merge`, `a_module_declaration_the_transport_repeats_is_one_declaration`, `two_module_declarations_of_one_input_stay_a_duplicate_binding` | each `InputOrigin` side |
| `OverrideUnmatched` | 2 | **B3 built** `wiring_overrides.rs :: stale_override`, `a_qualified_override_over_a_keyed_module_names_the_module_to_scope_to` | key, `keyed` |
| `OverrideAmbiguous` | 2 | **B3 built** `wiring_overrides.rs :: override_matching_two_modules` (`.everywhere()` and `.in_module` succeed) | `matches` both |
| `OverrideModuleAmbiguous` | 2 | **B3 built** `wiring_overrides.rs :: in_module_over_two_instances`, `in_module_over_two_keyed_instances_is_ambiguous_and_in_module_keyed_names_one` (`in_module_of`, `in_module_keyed` succeed) | `candidates` |
| `OverrideKind` | 2 | **B3 built** `wiring_overrides.rs :: override_value_over_a_collection` (`expected` is the kind the key is bound as, `found` the kind the override would make it; designer to confirm) | expected/found `BindingKind` |
| `TimerOverride` | 2 | **B3 built** `wiring_overrides.rs :: overriding_the_timer_is_refused_at_wiring` | variant only |
| `RuntimeOverride` | 2 | **B3 built** `wiring_overrides.rs :: overriding_the_runtime_is_refused_at_wiring` | variant only |
| `ReplacementMissingExports` | 2 | **B3 built** `wiring_overrides.rs :: replacement_exports_a_subset` | `missing` |
| `ReplacementUnmatched` | 2 | **B3 built** `wiring_overrides.rs :: replace_a_module_nobody_imports` | original; sole entry for the first call |
| `DuplicateReplacement` | 2 | **B3 built** `wiring_overrides.rs :: second_replace_module_of_one_original`, `a_duplicate_over_an_unimported_original_is_reported_once_per_call_never_both` | first/second locations |
| `UnreadPrefix` | freeze | **B3 built** `wiring_controllers.rs :: at_prefix_on_a_controller_no_transport_reads`, `a_transport_is_listed_once_however_many_handlers_it_has`, `on_a_controller_with_no_handlers`; control `a_prefix_one_handler_reads_is_accepted_and_so_is_no_prefix` | prefix, controller |
| `Missing` | 3 | **B1 built** `wiring_visibility.rs :: missing_dependency`, `a_binding_that_is_not_exported_is_missing_outside_its_module`, `missing_dependency_whose_key_differs_only_by_send_and_sync_names_both_spellings`, `a_service_reading_the_timer_of_an_app_with_none_is_told_how_to_set_one` | key, consumer, module, near |
| `Ambiguous` | 3 | **B1 built** `wiring_visibility.rs :: two_imports_export_one_key`, `an_ambiguous_key_in_the_root_is_reported_though_nothing_reads_it` | `sources` both modules |
| `Cycle` | 4 | **B1 built** `wiring_cycles.rs :: a_dependency_cycle_prints_its_full_path`, `a_service_reading_itself_is_a_cycle`, `two_separate_tangles_are_two_entries`; control `a_diamond_of_dependencies_is_not_a_cycle`. Cycles through `Option`/`Many` edges: not tested until question 3 is ruled | path steps with modules |
| `ScopeViolation` | 5 | **B3 built** `wiring_scopes.rs :: a_singleton_reading_execution_data_is_refused`, `a_singleton_reaching_execution_data_through_a_transient_is_refused_once`, `a_factory_singleton_is_refused_by_closure`, `an_auto_provider_reading_execution_data_is_refused_by_type_and_by_closure` | binding, declared, path |
| `HooksOnPerExecution` | 5 | **B3 built** `wiring_scopes.rs :: hooks_on_an_auto_type_inferred_per_execution_are_refused` (closure hook, trait hook), `a_readiness_check_on_a_binding_inferred_per_execution_is_refused` | binding, module |
| `ClosureScopeViolation` | 5 | **B3 built** `wiring_scopes.rs :: an_explicit_singleton_closure_enhancer_reading_execution_data_is_refused_in_both_tiers` (role `guard` only) | transport as `TypeName` |
| `ClosureNeedsExecution` | 5 | **B3 built** `wiring_scopes.rs :: hook_readiness_module_hook_and_metadata_closures_may_not_read_execution_data`, `an_optional_read_of_execution_data_in_a_hook_closure_is_refused_too` | closure, path |
| `InputNotSeeded` | 5 | **B3 built** `wiring_inputs.rs :: input_read_on_a_path_from_a_transport_that_does_not_seed_it`, `a_handler_parameter_reading_an_unseeded_input_is_refused_with_one_step`; control `an_input_read_from_the_seeding_transport_or_optionally_is_accepted` | handler, transport, input, seeder, steps |
| `BoundWithoutTimer` | 6 | **B1 built** `wiring_timer.rs :: a_construction_timeout_on_an_app_with_no_timer`, `a_constructs_after_bound_on_an_app_with_no_timer`, `a_trait_hooks_after_bound_on_an_app_with_no_timer`, `a_readiness_timeout_and_attempt_timeout_on_an_app_with_no_timer`, `a_hook_timeout_and_a_module_hook_timeout_on_an_app_with_no_timer`; `default_and_unbounded_need_no_timer`, `the_same_bounds_wire_on_an_app_with_a_timer` accepted | item, location points at the call |
| `BackoffWithoutTimer` | 6 | **B1 built** `wiring_timer.rs :: backoff_reports_its_own_line`; control `a_zero_backoff_waits_for_nothing_and_needs_no_timer` | binding, location |
| `KnobWithoutTimer` | 6 | **B1 built** `wiring_timer.rs :: a_builder_knob_on_an_app_with_no_timer` | knob name |

Also: `WiringErrors` collects (5 entries in one pass, unrelated missing bindings do not hide each other,
a missing piece skips only its edges), and the collision printing pass:
`wiring_pass.rs :: all_errors_arrive_in_one_pass_and_nothing_is_built` and its three siblings, `names.rs :: colliding_types_print_in_full_only_for_those_types` and its siblings (all **B1 built**).

## 3. §12 refusal table: point and home

| Refusal | Point | Home / test |
|---|---|---|
| Field/param not `FromContainer` | compile | `ulo-codegen-tests/tests/ui/site_not_from_container.rs` |
| `Dep<T>` with `T` not `Send + Sync` | compile | `ui/dep_of_a_non_send_type.rs` |
| `Ext`/`ExecutionRef` in an explicit singleton | compile | `ui/ext_in_an_explicit_singleton.rs` |
| Hook on explicit execution/transient | compile | `ui/hook_on_execution_type.rs`, `hook_on_transient_type.rs` |
| Closure hook or `.ready` on execution/transient handle | compile | `crates/ulo/tests/ui` is not available (no trybuild in `ulo`); put in `ulo-codegen-tests/tests/ui/ready_on_an_execution_handle.rs` |
| Controller-level `value = expr` | compile | `ui/controller_level_value.rs` |
| `.qualified` on an enhancer contribution | compile | `ui/qualified_on_enhancer_contribution.rs` |
| `enhancer::<K>()` with non-role `K` | compile | `ui/enhancer_of_a_non_role_key.rs` |
| Second bound on one handle item (`.timeout` twice, `.unbounded()` beside `.timeout`/`.attempt_timeout`, either after `.unbounded()`) | compile | `ui/second_bound_on_one_item.rs` (four cases, four files) |
| Enhancer lacking its role trait, controller-level too | compile | `ui/enhancer_without_its_role_trait.rs`, `controller_enhancer_for_another_transport.rs` |
| Constructor not `Send` | compile | `ui/constructor_not_send.rs` |
| Two/zero constructors, bad attribute, malformed `into` | compile | `ui/two_constructors.rs`, `no_constructor.rs`, `bad_attribute.rs`, `malformed_into.rs` |
| `X as <role key>` in `providers` | compile | `ui/provider_as_a_role_key.rs` |
| Trait binding with non-implementing type | compile | `ui/trait_binding_non_implementor.rs` |
| `?` on a failed value expression | wire | `wiring_bindings.rs :: ValueFailed` |
| Missing; `dyn Repo` vs `dyn Repo + Send + Sync` | wire | `wiring_visibility.rs` |
| Two sources for one key | wire | `wiring_visibility.rs :: Ambiguous` |
| Duplicate single; mix | wire | `wiring_bindings.rs` |
| Qualified contribution / single binding under a role key | wire | `wiring_roles.rs` |
| Dependency cycle; module import cycle | wire | `wiring_cycles.rs`, `wiring_modules.rs` |
| `ExportNotBound` | wire | `wiring_modules.rs` |
| `.at(prefix)` on a controller that reads none | wire | `wiring_controllers.rs :: UnreadPrefix` |
| Singleton -> execution (also via transients) | wire | `wiring_scopes.rs` |
| `Auto` provider reading execution data (type and closure) | wire | `wiring_scopes.rs :: auto_provider_reads_execution_data` (hint names, asserted as the variant fields `declared`, `by_closure`) |
| Hook on an `Auto` type inferred per execution | wire | `wiring_scopes.rs :: HooksOnPerExecution` |
| Hook/readiness/module-hook/metadata closure reading `Ext` etc. | wire | `wiring_scopes.rs :: ClosureNeedsExecution` |
| Input not seeded by the handler's transport | wire | `wiring_inputs.rs` |
| Input declared by a keyed module | wire | `wiring_modules.rs :: KeyedInput` |
| Input declared twice / against a binding | wire | `wiring_inputs.rs :: InputConflict` |
| Override matches nothing / several; `in_module` over instances | wire | `wiring_overrides.rs` |
| `replace_module` unmatched; duplicate | wire | `wiring_overrides.rs` |
| `override_value::<dyn Timer/Runtime>` | wire | `wiring_overrides.rs` |
| A module binding `dyn Timer` | wire | `wiring_timer.rs :: a_module_binding_dyn_timer_is_a_second_source` (`Ambiguous`) |
| A wait on an app with no `Timer` | wire | `wiring_timer.rs` |
| Constructor/factory/readiness/hook fails, panics, times out | connect | `construct_errors.rs`, `readiness.rs`, `hooks.rs`: `StartupError::Connect`, each `ConnectError` case and each `FailureReason` (`Failed`, `Panicked`, `TimedOut`) |
| `ModuleRef::get` for a singleton not yet built | connect | `lookup.rs :: NotReady` (a hook looks up a later singleton) |
| Transport on an app with no `Runtime`, or `.timer` alone | listen | existing `ulo-runtime-tokio/tests/app.rs :: an_app_with_a_timer_alone_is_refused_when_it_binds_a_transport` and `an_app_without_a_clock_...` (judged: pinned for this row; `RuntimeMissing` via `downcast_ref`, before `prepare`: verify the "before any server's prepare" half, Q5) |
| Lazy module refusals (5 `LoadRefusal`s); `LoadError::Wiring`/`Connect`; `Closed` | load | `lazy.rs` |
| `execute`/`Execution::open` from Draining on -> `Closed`; `open_terminal` with `DrainToken` allowed | runtime | `executions.rs :: draining_refuses_new_executions_but_allows_a_terminal_one` |
| Singleton lookup from Destroying on -> `LookupError::Closed` | runtime | `lookup.rs`, `hooks.rs` (lookup inside a destroy hook) |
| `LookupError` variants | runtime | `lookup.rs` |
| Input not seeded in standalone / extension not written -> `NotFound` / `None` | runtime | `executions.rs` |
| Guard refusal -> `GuardRejected` through error handlers | runtime | `enhancers.rs :: a_guard_refusal_reaches_the_error_handlers_as_GuardRejected` |
| Panic in guard/interceptor/handler/error handler -> `PanicRecovered`; `is_panic` | runtime | `enhancers.rs :: a_panic_is_caught_at_its_stage` (four stages); `is_panic` through wrapping `Err`/`source()`, in `lookup.rs` for `LookupError::Construct { Panicked }` |
| Shutdown hook panics/times out/skipped/param read fails; transport `close` fails/panics/exceeds | close | `shutdown.rs :: ShutdownFailure::{Hook, Close}` with `FailureReason`; later steps still run (probe order). `serve_loop_death` is already pinned by `ulo-runtime-tokio/tests/app.rs`; add the ninth response's two-server scenario to `shutdown.rs` if it differs (the existing `a_transport_whose_serve_fails_closes_the_app` is one server; Q4) |

## 4. Pure in-crate tests (`#[cfg(test)]`)

- **B1 built** `type_name.rs` (`type_name::tests`, 10 tests): `TypeName::colliding` (equal short names, different paths; generic arguments cut; the same type twice; only the colliding names), `written`, `{:#}` full, closure segments, equality by `TypeId`.
- **B1 built** `key.rs` (`key::tests`, 10 tests): key equality (type, qualifier, alias, unsized `dyn`, `dyn X + Send + Sync` distinct from `dyn X`), hashing in a map, `requalified`, `qualifier`, `Display`/`text_in`, `role_spelling`.
- **B1 built** `timer.rs` (`timer::tests`, 8 tests): `resolve_bound` over `Default`, `Unbounded`, `After(d)` for each `BoundKind` with and without defaults, and the `timeout` helper (the future that finishes first wins; the sleep wins; work completing in the poll the sleep fires in counts as done).
  Not built: `Bound::After(ZERO)` refused by a zero rule. No such rule exists in `crates/ulo/src` or in `DESIGN.md` (`resolve_bound` passes a zero duration through), so there is nothing to pin; see question 11.
  The `timeout` helper in `timer.rs` is `pub(crate)` and unused by the lib (dead-code warning); it is exercised here only by its own unit tests.

## 5. Build batches

Sizes in tests (approximate). Each batch: one Sonnet build agent, then an Opus check. A batch's own
`tests/common/mod.rs` additions go in batch 1 only.

| Batch | Areas | Files | Size | Depends on |
|---|---|---|---|---|
| B1 Foundations and wiring errors | harness (`common`), TypeName/key/Bound pure tests, wiring errors steps 1-4 and 6 (modules, bindings, roles, visibility, cycles, timer), `wiring_pass` | `common/mod.rs`, `wiring_modules.rs`, `wiring_bindings.rs`, `wiring_roles.rs`, `wiring_visibility.rs`, `wiring_cycles.rs`, `wiring_timer.rs`, `wiring_pass.rs`, `names.rs`, in-crate pure tests | ~34 tests (variants: ImportCycle, Reexport x2, KeyedInput, ExportNotBound, DuplicateBinding, KindMix, DanglingAlias, ValueFailed, role x2, DuplicateReadiness, Missing, Ambiguous, Cycle, timer x3; caps 10, 35, 44) | none |
| B2 Bindings, injection, modules | caps 1-9, 11-17, 29-34, 37 | `bindings.rs`, `injection.rs`, `modules.rs`, `module_ref.rs` | ~34 | B1 (harness) |
| B3 Scopes, executions, overrides | caps 18-23, 47, 48; wiring step 5 and overrides (`ScopeViolation`, `HooksOnPerExecution`, `ClosureScopeViolation`, `ClosureNeedsExecution`, `InputNotSeeded`, `InputConflict`, `UnreadPrefix`, override variants x9) | `scopes.rs`, `executions.rs`, `lookup.rs`, `testing.rs`, `wiring_scopes.rs`, `wiring_inputs.rs`, `wiring_overrides.rs`, `wiring_controllers.rs` | ~36 | B1 |
| B4 Lifecycle, phases, lazy, shutdown | caps 15, 38-40, 42, 43, 45; connect/close/load/draining rows of §12; two-server serve death | `construct_errors.rs`, `hooks.rs`, `readiness.rs`, `phases.rs`, `lazy.rs`, `shutdown.rs`, `startup_errors.rs` | ~30; needs `Tokio` (timers: use a frozen `Timer` as `ulo-runtime-tokio/tests/app.rs` does) | B1 |
| B5 Enhancer pipeline | caps 24-28 (a test `Transport` marker pair, one `Controller` fixture; no HTTP) and the runtime rows (`GuardRejected`, panic stages) | `enhancers.rs`, `common/transport.rs` | ~14; fixture-heavy, run alone | B1, B3 (per-execution) |
| B6 Compile-time refusals | caps 9, 41, 46 and every compile row of §3 | `ulo-codegen-tests/tests/ui/*.rs` + `.stderr` (about 22 files); `compile_fail.rs` globs `tests/ui/*.rs` already | ~22 ui cases; run one at a time (`TRYBUILD=overwrite` only on first creation, then review stderr) | none (independent of B1-B5). **Status 2026-10-20: built, 28 new `ui/` cases, each shown red under a planted break; the controller-level `value = expr` row is not refused (F453), so it has no case.** |
| O1 Owner-crate gaps, HTTP | SWITCH_INVENTORY G9-G15 | `ulo-http/tests/` (routing, SSE, extractors, handler forms, errors, metadata, CORS, tower, module middleware = cap 36) | see section 6 | B1 style only |
| O2 Health, gRPC/RPC, WS | G17, G19-G21, G25 | `ulo-health-http/tests`, `ulo-health-grpc/tests`, `ulo-grpc/tests` or `ulo-codegen-tests`, `ulo-rpc/tests`, `ulo-ws/tests` | see section 6 | none |
| O3 Config, GraphQL, tracing | G16, G18, G24 | `ulo-config/tests`, `ulo-graphql*/tests`, `ulo-otel/tests` | see section 6 | none |

Core batches are roughly 30-36 tests each (B6 22 compile cases). B2, B3, B4 can run in parallel with
each other after B1 lands (separate files); B5 follows B3. B6 can start at once. Cargo runs one at a
time regardless (shared gotcha: one `cargo test` at a time).

### B1 status

Built: `tests/common/mod.rs` (harness, documented in its header), `names.rs`, `wiring_modules.rs`, `wiring_bindings.rs`, `wiring_roles.rs`, `wiring_visibility.rs`, `wiring_cycles.rs`, `wiring_timer.rs`, `wiring_pass.rs`, and the `#[cfg(test)]` modules in `type_name.rs`, `key.rs` and `timer.rs`. Each test file's header names the capabilities it pins. The planted-break table is in the B1 report; every break was planted in a scratch copy of the crate, never in the working tree.
Not built, with the reason: cycles through `Option`/`Many` edges (question 3); `Bound::After(ZERO)` (no rule, question 11); the `ulo-runtime-tokio` clock/runtime-override tests stay where they are (B3 adds the variant-level ones).

### B2 status

Built: `tests/bindings.rs` (11 tests), `tests/injection.rs` (8), `tests/modules.rs` (13), `tests/module_ref.rs` (7); 39 in all. `tests/common/mod.rs` gained four appended helpers (`connects`, `connect_err`, `read_q`, `read_many`) under a marker comment; nothing above it changed. Each file's header names the capabilities it pins. Every test was shown red under a planted break in a scratch copy of the crate (never in the working tree); the table is in the B2 report.
Placement: capability 17 is built in `injection.rs`, not `scopes.rs`; capability 15 is built only as far as a `Failed` constructor and a `try_` factory's `Err` (`Panicked` and `TimedOut` stay with B4). Capability 9 is a compile-time property and stays in B6. Capability 29's controllers need a transport fixture and are left to B5.
Not built: none of the B2 rows' questions for the designer apply (question 1 on capability 9 is B6's).

### B3 status

Built: `wiring_scopes.rs` (15), `wiring_inputs.rs` (9), `wiring_overrides.rs` (13), `wiring_controllers.rs` (4), `scopes.rs` (12), `executions.rs` (9), `lookup.rs` (10), `testing.rs` (11): 83 tests, against the plan's ~36, because each variant row of sections 2 and 3 for these areas got its own test. `tests/common/mod.rs` gained, appended below a marker comment: `Count`, `reader!`, `controller!`, `connected`, `connected_with`, `connected_on_tokio`, `close`. Each test file's header names its capabilities.

Planted breaks: 69, each applied to a scratch copy of `ulo`, `ulo-macros`, `ulo-handler-codegen` and `ulo-runtime-tokio` with its own target dir, source restored and compared byte-for-byte after each. Red counts (tests failing out of the suite, not only the intended one): S1 5, S2 1, S3 18, S4 1, S5 1, S6 1, S7 1, S8 2, S9 1, S10 1, S11 1, S12 3, S13 1, S14 5, S15 15, S16 3, S17 8, S18 5, S19 1, S20 1, S21 1 (wiring step 5 and run-time scopes); E1 3, E2 1, E3 2, E4 1, E5 1, E6 4, E7 42, E8 1, E9 1 (executions); L1 2, L2 1, L3 1, L4 1, L5 1, L6 1, L7 2, L8 1, L9 1, L10 5, L11 1 (lookups); T1 3, T2 1, T3 2, T4 2, T5 1, T6 1, T7 1, T8 1, T9 2, T10 2, T11 1, T12 2, T13 1, T14 5, T15 1, T16 1 (overrides); C1 3, C2 2, C3 1 (`UnreadPrefix`); I1 1, I2 3, I3 1, I4 1, I5 1, I6 1 (`InputConflict`). The break for each id is the one-line change in the driver's list, named in its label.

Survivors before strengthening: T15 (the break was equivalent: it forced the removed count to 1 but still removed the contributions; replaced by a break that counts without removing, which `override_many_replaces_a_whole_collection_and_qualified_targets_the_qualified_one` fails) and T16 (`override_factory` stopped recording its parameters and no test noticed; added `wiring_overrides.rs :: an_override_factory_s_parameters_are_checked_by_the_wiring_pass`, red under the break). No other break left its tests green.

Not built: `open_terminal` during the drain (the `DrainToken` is built only inside a `Server`'s `drain`, so it needs a server fixture; B4); the `interceptor` and `error handler` role strings of `ClosureScopeViolation` (only `guard`); run-time scope of closure enhancers (cap 24, B5). The WebSocket-connection half of cap 19 stays in `ulo-ws-hyper`. `OverrideKind`'s `expected`/`found` reading is pinned as documented in its row; designer to confirm. No defect found.

## 6. Owner-crate gaps (SWITCH_INVENTORY section 1)

Assigned to the owner's `tests/`, never `crates/ulo`. G1-G8 are the core capabilities above (G1 -> B3, G2 -> B1/B3, G3 -> B2, G4 -> B2, G5 -> B3, G6 -> B4, G7 -> B5, G8 -> B3); G22 -> B4 `startup_errors.rs`; G23 -> B4 (existing single-server cover plus the two-server scenario).

| Gap | Owner crate tests/ | Tests (variant/field level) |
|---|---|---|
| G9 HTTP routing | `ulo-http/tests/routing.rs` | trailing slash insignificant (same handler for `/a` and `/a/`), `{name}` parameter, duplicate route up to slashes refused at wiring/bind with a typed error, `{*rest}` |
| G10 CORS, tower, module middleware, rewrite | `ulo-http/tests/cors.rs`, `tower_bridge.rs`, `middleware.rs` | credentials forces origin echo, allow-list, max-age, non-preflight decoration, tower layer runs pre-dispatch, per-module middleware by route with `exclude`, URI rewrite changes the match (also capability 36) |
| G11 SSE | `ulo-http/tests/sse.rs` | headers; `data`/`id`/`event`/`retry`/comment framing; multi-line data splitting on CR/LF/CRLF; empty data writes `data: `; `LastEventId`; `end_event`; `keep_alive` |
| G12 HTTP extractors, `Valid` | `ulo-http/tests/extract.rs`; `Valid` on RPC/WS in `ulo-rpc/tests`, `ulo-ws/tests` | `Path`, `Query`, `Json`, `Form`, `Multipart`, `BodyStream`, `Host`, `Header` success and the failure variant; `Valid<P>` failure shape per transport |
| G13 Handler forms | `ulo-http/tests/handlers.rs` (macro forms in `ulo-codegen-tests`) | sync/async, no `&self`, alias of `Result` reaches the chain, `IntoReply` forms |
| G14 Error model | `ulo-transport/tests/errors.rs`, `ulo-http/tests/errors.rs` | `Classify` derive kind per variant, kind -> HTTP status table, chain claim vs fall-through, reshape |
| G15 Declared metadata on HTTP and RPC | `ulo-http/tests/meta.rs`, `ulo-rpc/tests/meta.rs` | handler and impl metadata, handler wins per type |
| G16 Config | `ulo-config/tests/layers.rs` | layered sources precedence (defaults < file < `.env` < env < values), section by path, aggregated errors name path and source, secrets redacted (partly exists: `secret_leak`) |
| G17 Health | `ulo-health-http/tests/probes.rs` | probe paths, 200/503, body, detail off by default, readiness false at drain before the first refusal |
| G18 GraphQL | `ulo-graphql-http/tests/mount.rs`, `ulo-graphql-ws/tests/subscriptions.rs`, per engine in `ulo-graphql-async-graphql/tests`, `ulo-graphql-juniper/tests` | mount path, playground HTML, module identity (same schema two paths mounts both; collision refused), graphql-transport-ws |
| G19 gRPC | `ulo-codegen-tests/tests/` (needs generated protos) | TLS serving (`ulo-test-certs`), `grpc-status-details-bin` payload, in-flight bound answers `ResourceExhausted`, service not injectable |
| G20 RPC | `ulo-rpc/tests/` | controller cannot be injected, `RpcClientModule` injection, declared metadata, `Valid` payload |
| G21 Multi-transport | `ulo-codegen-tests` or `ulo-http-hyper/tests` | one controller on two transports; one app serving four transports; one `close` drains all |
| G24 Tracing | `ulo-otel/tests/propagation.rs`, `ulo-otel-otlp/tests/wire.rs` | `traceparent` across transports, span per call, reserved-header refusal per transport (`reserved_header`), explicit OTLP header wins over env |
| G25 WS execution scope | `ulo-ws-hyper/tests/` | per-message scope, connect-guard bag reaches `on_connect` |
| G26 Examples | not tests: the example catalogue step after the series | out of scope here |

Batches: O1 = G9-G15 (HTTP, ~25 tests), O2 = G17, G19-G21, G25 (~18), O3 = G16, G18, G24 (~16).

## 7. Questions for the designer

1. Capability 9 ("no free-form string keys"): `qualified::<Q>()` and `for_root_named`-style
   names are types, but `ModuleName`/`DynamicModule::builder(id)` take a string and `module_keyed`
   keys are types. Is the test "no API takes a string where a key is expected" (ui test on `get`
   and `value`), or does `DynamicModule`'s string id count as a key? The plan assumes the former.
2. Capability 36 (per-module middleware by route) lives in `ulo-http`/`ulo-transport`, not
   `crates/ulo`: `crates/ulo/src` mentions middleware only in `app/load.rs` (the lazy refusal),
   `module/def.rs`, `module/meta.rs`. Confirm the owner crate and whether "by route" with
   exclusions is still a capability or became `PreDispatch` scoping.
3. §10.1 step 4: are cycles through `Option<Dep<_>>` or `Many<_>` edges cycles? The text says
   "resolved edges"; no rule for optional edges. The plan tests the plain cycle only until ruled.
4. `serve_loop_death`: `ulo-runtime-tokio/tests/app.rs` already has `a_transport_whose_serve_fails_closes_the_app`,
   `..._returns_while_the_app_runs_closes_the_app` and `two_serves_ending_on_one_poll_...`, and
   the lib implements it. The ninth response's two-server scenario (one fails mid-run, the other drains and closes, hooks run, `serve` answers `Err` naming the transport) is partly covered; confirm whether those tests count or the scenario is wanted in the core series as written.
5. §12 "before any server's `prepare`" for `RuntimeMissing`: `app.rs` tests refusal but the
   ordering half is not visibly asserted. Needs a server fixture recording `prepare`.
6. Capability 19: "a WebSocket connection is not an execution" is a transport property; the core
   cannot test it. Plan puts it in `ulo-ws-hyper`. Confirm.
7. Capability 41 and the `.ready(..)`/closure-hook compile rows are trybuild tests; the plan puts
   them in `ulo-codegen-tests` because `ulo` has no trybuild dev-dependency. Confirm, or allow
   `trybuild` as a dev-dependency of `ulo` (the `ui/` directory would then live in `crates/ulo/tests/ui`).
8. Capability 15/45: `FailureReason::{Failed, Panicked, TimedOut}` names are taken from
   DESIGN §12 prose; the builder agent must read `error/mod.rs` for the actual case names before asserting.
9. Capability 42 "credentials redacted": is the assertion on `Redacted`'s content by value (the
   registered secret absent) or only that the error is a `Redacted`? The ruling forbids message
   text assertions; plan asserts the secret string is absent from `Display`, an explicit exception to ask about.
10. `timer.rs::timeout` is `pub(crate)` and `cargo test` reports it unused (dead-code warning, with
    others in `redact.rs`). If it is not used by the library, it may be a planted-break target or
    dead code; the lib builds with 17 warnings.
11. `Bound::After(Duration::ZERO)`: the plan expected a "zero rule" that refuses it at wiring or prepare time. Neither `DESIGN.md` nor `crates/ulo/src` has one: `resolve_bound` returns `After { after: ZERO, .. }` and a zero timeout then expires at once. Is a zero bound legal (an immediate expiry) or to be refused? No test written until ruled.
12. `SingleRoleBinding` is spelled "`provide`, `value`, `also_as` or an alias" in section 12, but `provide`/`value` register a sized type under its own key, so a role key (always a `dyn`) can only be reached through `also_as` (on a `provide`, a `value` or a factory) or an alias. B1 pins those four. Confirm the section 12 wording should say so.
