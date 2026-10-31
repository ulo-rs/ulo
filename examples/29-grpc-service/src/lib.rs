use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use futures_util::{Stream, stream};
use ulo::{App, AppBuilder, BoxError, Guard, injectable, module, routes};
use ulo_grpc::{CallError, Classify, Detail, Details, ErrorKind, FieldViolation, Grpc, GrpcCx, Message};

pub mod pb {
    ulo_grpc::include_proto!("catalog.v1");
}

pub const TOKEN: &str = "Bearer open";

const ITEMS: [(&str, &str); 3] = [("a1", "anvil"), ("b2", "bellows"), ("c3", "crucible")];

/// Admits a call whose `authorization` metadata is [`TOKEN`]. The refusal is `Unauthorized`, which
/// gRPC renders as UNAUTHENTICATED; `Ok(false)` would render PERMISSION_DENIED.
#[injectable]
pub struct TokenGuard;

impl Guard<Grpc> for TokenGuard {
    async fn can_activate(&self, cx: &GrpcCx) -> Result<bool, BoxError> {
        match cx.metadata().get("authorization") {
            Some(TOKEN) => Ok(true),
            _ => Err(CallError::unauthorized("Bearer").into()),
        }
    }
}

#[derive(Debug)]
pub enum ItemError {
    EmptyId,
    Missing(String),
}

impl fmt::Display for ItemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ItemError::EmptyId => f.write_str("an item id is required"),
            ItemError::Missing(id) => write!(f, "no item `{id}`"),
        }
    }
}

impl Error for ItemError {}

impl Classify for ItemError {
    fn classify(&self) -> ErrorKind {
        match self {
            ItemError::EmptyId => ErrorKind::BadRequest,
            ItemError::Missing(_) => ErrorKind::NotFound,
        }
    }

    fn details(&self) -> Details {
        match self {
            ItemError::EmptyId => Details::new().with(Detail::FieldViolations(vec![FieldViolation::new("id", "must not be empty")])),
            ItemError::Missing(id) => Details::new().with(Detail::ErrorInfo {
                reason: "ITEM_NOT_FOUND".to_owned(),
                domain: "catalog.example".to_owned(),
                metadata: BTreeMap::from([("id".to_owned(), id.clone())]),
            }),
        }
    }
}

fn item(id: &str, name: &str) -> pb::Item {
    pb::Item { id: id.to_owned(), name: name.to_owned() }
}

#[injectable]
pub struct CatalogService;

#[routes]
#[guards(grpc = TokenGuard)]
impl CatalogService {
    #[ulo_grpc::method(pb::catalog::GetItem)]
    async fn get_item(&self, req: Message<pb::GetItemRequest>) -> Result<pb::Item, ItemError> {
        let id = req.0.id;
        if id.is_empty() {
            return Err(ItemError::EmptyId);
        }
        ITEMS.iter().find(|(known, _)| *known == id).map(|(id, name)| item(id, name)).ok_or(ItemError::Missing(id))
    }

    #[ulo_grpc::method(pb::catalog::ListItems)]
    async fn list_items(&self, req: Message<pb::ListItemsRequest>) -> impl Stream<Item = Result<pb::Item, ItemError>> {
        let limit = if req.0.limit == 0 { ITEMS.len() } else { req.0.limit as usize };
        stream::iter(ITEMS.iter().take(limit).map(|(id, name)| Ok(item(id, name))))
    }
}

#[module(controllers = [CatalogService], providers = [TokenGuard])]
pub struct AppModule;

/// The application, before a runtime is chosen: `main` and the tests each name theirs.
pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
