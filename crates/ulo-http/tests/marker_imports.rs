//! `#[handlers]`, `#[controller]` and `#[injectable]` read and remove the attributes they carry,
//! and write beside the item a hidden import of each, through which an editor resolves it. The
//! attributes still mean what they meant: written several times on one impl, bare and under
//! `ulo` (`ulo::guards`, `::ulo::guards`), behind `cfg`, inside a fn body, on a generic
//! controller, and beside a module or attribute macro of the same name, each of them compiles and
//! each guard written runs. Any other qualifier is refused at compile time
//! (`ulo-codegen-tests/tests/ui/marker_qualifier_*.rs`).

mod o1_support;

use std::marker::PhantomData;
use std::sync::Mutex;

use http::StatusCode;
use o1_support::{Call, start};
use ulo::{BoxError, Construct, Guard, ModuleDef, controller, handlers, injectable};
use ulo_http::{Http, HttpCx};

/// Admits, and records its label in `SEEN` when it runs.
struct Mark(&'static str);

static SEEN: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

impl Guard<Http> for Mark {
    async fn can_activate(&self, _cx: &HttpCx) -> Result<bool, BoxError> {
        SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(self.0);
        Ok(true)
    }
}

/// The labels recorded so far that start with `prefix`, in order: each test reads its own.
fn seen(prefix: &str) -> Vec<&'static str> {
    SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().copied().filter(|l| l.starts_with(prefix)).collect()
}

struct Deny;

impl Guard<Http> for Deny {
    async fn can_activate(&self, _cx: &HttpCx) -> Result<bool, BoxError> {
        Ok(false)
    }
}

/// Modules named like the markers, beside impls that write them.
#[allow(dead_code)]
mod guards {}
#[allow(dead_code)]
mod meta {}

#[derive(Debug)]
struct Tag(&'static str);

#[controller]
struct Repeated;

#[handlers]
#[guards(value = Mark("repeated:impl"))]
#[meta(Tag("impl"))]
impl Repeated {
    #[guards(value = Mark("repeated:before"))]
    #[ulo_http::get("/repeated")]
    #[guards(value = Mark("repeated:after"))]
    #[meta(Tag("method"))]
    fn repeated(&self, cx: HttpCx) -> String {
        let info = cx.exec().handler().expect("the handler's info is set");
        info.meta_all::<Tag>().map(|tag| tag.0).collect::<Vec<_>>().join(",")
    }

    #[ulo::guards(value = Mark("repeated:qualified"))]
    #[ulo_http::get("/qualified")]
    #[ulo::meta(Tag("qualified"))]
    fn qualified(&self) -> &'static str {
        "qualified"
    }
}

#[controller]
struct Gated;

#[handlers]
impl Gated {
    #[::ulo::guards(value = Deny)]
    #[ulo_http::get("/absolute-path")]
    fn absolute_path(&self) -> &'static str {
        "unreachable"
    }

    #[cfg_attr(any(), guards(value = Deny))]
    #[ulo_http::get("/compiled-out")]
    fn compiled_out(&self) -> &'static str {
        "open"
    }

    #[cfg(any())]
    #[guards(value = Deny)]
    #[ulo_http::get("/absent")]
    fn absent(&self) -> &'static str {
        "absent"
    }
}

#[controller]
struct Generic<T: Send + Sync + 'static> {
    #[controller(default)]
    _of: PhantomData<T>,
}

#[handlers]
#[guards(value = Mark("generic:impl"))]
impl<T: Send + Sync + 'static> Generic<T> {
    #[ulo_http::get("/generic")]
    #[guards(value = Deny)]
    fn generic(&self) -> &'static str {
        "unreachable"
    }
}

/// A user's own attribute macro named `guards`, imported explicitly beside a `#[handlers]` impl that
/// writes the marker of that name.
mod own_guards {
    use ulo::{controller, handlers};
    pub use ulo::injectable as guards;

    use super::Deny;

    #[guards]
    pub struct Own;

    #[controller]
    pub struct Beside;

    #[handlers]
    impl Beside {
        #[ulo_http::get("/beside")]
        #[guards(value = Deny)]
        fn beside(&self) -> &'static str {
            "unreachable"
        }
    }
}

pub struct Built;

#[injectable]
impl Built {
    #[ulo::construct]
    pub fn build() -> Self {
        Built
    }
}

fn app(m: &mut ModuleDef<'_>) {
    m.controller::<Repeated>();
    m.controller::<Gated>();
    m.controller::<Generic<u8>>();
    m.controller::<own_guards::Beside>();
}

#[tokio::test]
async fn markers_written_several_times_before_and_after_the_transport_attribute_all_apply() {
    let app = start(app).await;

    let reply = app.send(Call::get("/repeated")).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.text(), "method,impl", "both tiers' metadata");
    assert_eq!(seen("repeated:"), ["repeated:impl", "repeated:before", "repeated:after"]);

    assert_eq!(app.send(Call::get("/qualified")).await.status, StatusCode::OK);
    assert_eq!(seen("repeated:q"), ["repeated:qualified"]);
    app.stop().await;
}

#[tokio::test]
async fn a_marker_under_an_absolute_ulo_path_guards_and_one_compiled_out_does_not() {
    let app = start(app).await;

    assert_eq!(app.send(Call::get("/absolute-path")).await.status, StatusCode::FORBIDDEN);
    let open = app.send(Call::get("/compiled-out")).await;
    assert_eq!((open.status, open.text()), (StatusCode::OK, "open".to_owned()));
    assert_eq!(app.send(Call::get("/absent")).await.status, StatusCode::NOT_FOUND);
    app.stop().await;
}

#[tokio::test]
async fn a_generic_controller_and_one_beside_a_users_guards_macro_are_guarded() {
    let app = start(app).await;

    assert_eq!(app.send(Call::get("/generic")).await.status, StatusCode::FORBIDDEN);
    assert_eq!(seen("generic:"), ["generic:impl"]);
    assert_eq!(app.send(Call::get("/beside")).await.status, StatusCode::FORBIDDEN);
    app.stop().await;
}

#[test]
fn the_users_own_guards_attribute_is_still_theirs() {
    fn constructs<T: Construct>() {}
    constructs::<own_guards::Own>();
    constructs::<Built>();
}

#[tokio::test]
async fn markers_inside_a_fn_body_apply() {
    #[controller]
    struct Local;

    #[handlers]
    #[guards(value = Mark("local:impl"))]
    impl Local {
        #[ulo_http::get("/local")]
        #[guards(value = Mark("local:method"))]
        fn local(&self) -> &'static str {
            "local"
        }
    }

    struct Made;

    #[injectable]
    impl Made {
        #[construct]
        fn make() -> Self {
            Made
        }
    }

    fn constructs<T: Construct>() {}
    constructs::<Made>();

    let app = start(|m| {
        m.controller::<Local>();
    })
    .await;
    assert_eq!(app.send(Call::get("/local")).await.status, StatusCode::OK);
    assert_eq!(seen("local:"), ["local:impl", "local:method"]);
    app.stop().await;
}
