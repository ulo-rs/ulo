use std::collections::HashMap;
use std::sync::Mutex;

use ulo::{App, AppBuilder, BoxError, Dep, injectable, module, routes};
use ulo_http::{BodyLimit, Bytes, CallError, Created, ErrorKind, KB, Multipart, Path};
use ulo_http_hyper::Server;

/// What a body may weigh unless its route says otherwise.
pub const SERVER_LIMIT: u64 = KB;
/// The tighter limit on `/avatars`.
pub const AVATAR_LIMIT: u64 = 256;

/// The server this example runs on; `main` and the tests bind it to their own address.
pub fn server(endpoint: &str) -> Server {
    Server::new(endpoint).body_limit(SERVER_LIMIT)
}

#[injectable]
pub struct Files {
    #[injectable(default)]
    stored: Mutex<HashMap<String, Bytes>>,
}

#[injectable]
pub struct UploadController {
    files: Dep<Files>,
}

impl UploadController {
    /// Stores every part that carries a file name and answers where the first one went.
    async fn store(&self, mut parts: Multipart) -> Result<Created<&'static str>, BoxError> {
        let mut first = None;
        while let Some(field) = parts.next_field().await? {
            let Some(name) = field.file_name().map(str::to_owned) else { continue };
            let content = field.bytes().await?;
            self.files.stored.lock().unwrap().insert(name.clone(), content);
            first.get_or_insert(name);
        }
        let Some(name) = first else {
            return Err(CallError::new(ErrorKind::BadRequest, "the form carried no file").into());
        };
        Ok(Created::at(format!("/files/{name}"), "stored"))
    }
}

#[routes]
impl UploadController {
    #[ulo_http::post("/files")]
    async fn upload(&self, parts: Multipart) -> Result<Created<&'static str>, BoxError> {
        self.store(parts).await
    }

    #[ulo_http::post("/avatars")]
    #[meta(BodyLimit(AVATAR_LIMIT))]
    async fn avatar(&self, parts: Multipart) -> Result<Created<&'static str>, BoxError> {
        self.store(parts).await
    }

    #[ulo_http::get("/files/{name}")]
    fn download(&self, Path(name): Path<String>) -> Result<Bytes, CallError> {
        self.files.stored.lock().unwrap().get(&name).cloned().ok_or_else(|| CallError::new(ErrorKind::NotFound, format!("no file {name}")))
    }
}

#[module(providers = [Files], controllers = [UploadController])]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
