//! A constructor parameter or a struct field behind a `#[cfg]` that is off exists in the source
//! alone, for `#[injectable]` and `#[controller]` in both forms: its type is declared under the same
//! `cfg`, so a read, a declaration or a passing of it fails to compile, and the app wires without
//! the dependency. A `cfg` reached through `cfg_attr` gates as a bare one does, a field marked
//! `#[construct(default)]` is set only where it is compiled, and a tuple struct whose gated field
//! renumbers the fields after it is built by position. A parameter or field whose `cfg` is on,
//! `cfg(test)` in this file's build, is declared, read and passed, so a gate written as always off
//! leaves the call an argument short and the struct a field short.

use ulo::{App, Dep, Module, ModuleDef, ModuleIdentity, controller, handlers, injectable};

/// Declared under the `cfg` the parameters and fields carry, so no build of this file has it.
#[cfg(any())]
struct Absent;

/// Bound in every module below.
struct Present(u32);

/// Bound by [`Root`] and left out of [`Unbound`], which tells a declared dependency from one that
/// is not.
struct On(u32);

struct ByCtor {
    present: Dep<Present>,
    on: Dep<On>,
}

#[injectable]
impl ByCtor {
    fn new(
        #[cfg(any())] absent: Dep<Absent>,
        present: Dep<Present>,
        #[cfg_attr(test, cfg(any()))] through: Dep<Absent>,
        #[cfg(test)] on: Dep<On>,
    ) -> Self {
        ByCtor { present, on }
    }
}

#[injectable]
struct ByFields {
    #[cfg(any())]
    absent: Dep<Absent>,
    present: Dep<Present>,
    #[cfg_attr(test, cfg(any()))]
    through: Dep<Absent>,
    #[cfg(any())]
    #[construct(default)]
    defaulted_absent: Absent,
    #[cfg(test)]
    on: Dep<On>,
    #[cfg(test)]
    #[construct(default)]
    count: u32,
}

/// Field `1` is compiled out, so `Present` is field `1` and the default field `2` of what rustc
/// sees.
#[injectable]
struct ByPosition(
    #[cfg(any())] Dep<Absent>,
    Dep<Present>,
    #[cfg(any())]
    #[construct(default)]
    Absent,
    #[construct(default)] u32,
    #[cfg(test)] Dep<On>,
);

/// Only a defaulted field is gated, and it renumbers the fields after it as a gated dependency
/// does: `Present` is field `0` and the default field `1` of what rustc sees.
#[injectable]
struct ByDefaultPosition(
    #[cfg(any())]
    #[construct(default)]
    Absent,
    Dep<Present>,
    #[construct(default)] u32,
);

struct CtrlByCtor {
    on: Dep<On>,
}

#[controller]
impl CtrlByCtor {
    fn new(#[cfg(any())] absent: Dep<Absent>, #[cfg(test)] on: Dep<On>) -> Self {
        CtrlByCtor { on }
    }
}

#[handlers]
impl CtrlByCtor {}

#[controller]
struct CtrlByFields {
    #[cfg(any())]
    absent: Dep<Absent>,
    #[cfg(any())]
    #[construct(default)]
    defaulted_absent: Absent,
    #[cfg(test)]
    on: Dep<On>,
}

#[handlers]
impl CtrlByFields {}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(Present(7));
        m.value(On(9));
        m.provide::<ByCtor>();
        m.provide::<ByFields>();
        m.provide::<ByPosition>();
        m.provide::<ByDefaultPosition>();
        m.controller::<CtrlByCtor>();
        m.controller::<CtrlByFields>();
    }
}

/// [`Root`] without `On`, for one constructed type at a time.
struct Unbound(Register);

impl Module for Unbound {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(Present(7));
        (self.0)(m);
    }
}

/// A `Dep<Absent>` the wiring read would be a missing dependency: none is reported, and each value
/// is the one the `cfg` that is on reads.
#[tokio::test]
async fn a_constructed_type_whose_parameters_or_fields_a_cfg_compiles_out_wires_and_builds_without_them() {
    let app = App::builder(Root).runtime(ulo_runtime_tokio::Tokio::current()).wire().unwrap_or_else(|error| panic!("the app did not wire: {error}"));
    let app = app.connect().await.unwrap_or_else(|error| panic!("the app did not connect: {error}"));

    let by_ctor = app.get::<ByCtor>().await.unwrap_or_else(|error| panic!("ByCtor: {error}"));
    assert_eq!((by_ctor.present.0, by_ctor.on.0), (7, 9));
    let by_fields = app.get::<ByFields>().await.unwrap_or_else(|error| panic!("ByFields: {error}"));
    assert_eq!((by_fields.present.0, by_fields.on.0, by_fields.count), (7, 9, 0));
    let by_position = app.get::<ByPosition>().await.unwrap_or_else(|error| panic!("ByPosition: {error}"));
    assert_eq!((by_position.0.0, by_position.1, by_position.2.0), (7, 0, 9));
    let by_default_position = app.get::<ByDefaultPosition>().await.unwrap_or_else(|error| panic!("ByDefaultPosition: {error}"));
    assert_eq!((by_default_position.0.0, by_default_position.1), (7, 0));
    let ctrl_by_ctor = app.get::<CtrlByCtor>().await.unwrap_or_else(|error| panic!("CtrlByCtor: {error}"));
    assert_eq!(ctrl_by_ctor.on.0, 9);
    let ctrl_by_fields = app.get::<CtrlByFields>().await.unwrap_or_else(|error| panic!("CtrlByFields: {error}"));
    assert_eq!(ctrl_by_fields.on.0, 9);
}

/// What [`Unbound`] registers beside `Present`.
type Register = fn(&mut ModuleDef<'_>);

/// A parameter or field whose `cfg` is on is declared: with `On` unbound, wiring reports it.
#[test]
fn a_parameter_or_field_whose_cfg_is_on_is_declared() {
    let registrations: [(&str, Register); 5] = [
        ("ByCtor", |m| {
            m.provide::<ByCtor>();
        }),
        ("ByFields", |m| {
            m.provide::<ByFields>();
        }),
        ("ByPosition", |m| {
            m.provide::<ByPosition>();
        }),
        ("CtrlByCtor", |m| {
            m.controller::<CtrlByCtor>();
        }),
        ("CtrlByFields", |m| {
            m.controller::<CtrlByFields>();
        }),
    ];
    for (name, register) in registrations {
        let Err(error) = App::builder(Unbound(register)).wire() else {
            panic!("{name}: wired with `On` unbound, so its gated-on dependency was not declared");
        };
        let report = error.to_string();
        assert!(report.contains("missing dependency `On`"), "{name}: the report does not name `On`: {report}");
    }
}
