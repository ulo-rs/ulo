// Routes written in another router's parameter and catch-all syntax, `:id` and `*rest`, which ulo
// would match as literal text.
// Refused at: each route literal, naming the `{id}` and `{*rest}` to write instead.
use ulo::{controller, handlers};

#[controller]
struct Files;

#[handlers]
impl Files {
    #[ulo_http::get("/raw/:id")]
    fn raw(&self) -> &'static str {
        "raw"
    }

    #[ulo_http::get("/files/*rest")]
    fn files(&self) -> &'static str {
        "files"
    }
}

fn main() {}
