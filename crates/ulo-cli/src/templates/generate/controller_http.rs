use serde::Serialize;
use ulo::{injectable, routes};
use ulo_http::Json;

#[derive(Debug, Serialize)]
pub struct @@pascal@@Listing {
    pub items: Vec<String>,
}

/// HTTP routes under `/@@route@@`. Bind it with `m.controller::<@@pascal@@Controller>()`.
#[injectable]
pub struct @@pascal@@Controller;

#[routes]
impl @@pascal@@Controller {
    #[ulo_http::get("/@@route@@")]
    fn list(&self) -> Json<@@pascal@@Listing> {
        Json(@@pascal@@Listing { items: Vec::new() })
    }
}

#[cfg(test)]
mod tests {
    use super::@@pascal@@Controller;

    #[test]
    fn lists_nothing_yet() {
        assert!(@@pascal@@Controller.list().0.items.is_empty());
    }
}
