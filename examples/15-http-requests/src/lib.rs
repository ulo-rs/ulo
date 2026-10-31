use std::fmt;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use ulo::{App, AppBuilder, Dep, injectable, module, routes};
use ulo_http::headers::UserAgent;
use ulo_http::{Classify, Created, Header, Json, Path, Query, Valid, Validate, WithHeaders};

pub mod intake;

#[derive(Clone, Serialize)]
pub struct Note {
    pub id: u32,
    pub title: String,
}

/// The body of `POST /notes`; a title outside 1 to 20 characters is refused before the handler runs.
#[derive(Deserialize, Validate)]
pub struct NewNote {
    #[validate(length(min = 1, max = 20))]
    pub title: String,
}

#[derive(Deserialize)]
pub struct Page {
    pub limit: Option<usize>,
}

/// `#[classify(not_found)]` is the whole mapping: the answer is a 404 problem document.
#[derive(Debug, Classify)]
#[classify(not_found)]
pub struct NoteNotFound(pub u32);

impl fmt::Display for NoteNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no note {}", self.0)
    }
}

impl std::error::Error for NoteNotFound {}

/// A return type spelled as an alias of `Result` reaches the error chain as the domain error all the same.
pub type Found = Result<Json<Note>, NoteNotFound>;

#[injectable]
pub struct Notes {
    #[injectable(default)]
    all: Mutex<Vec<Note>>,
}

impl Notes {
    fn add(&self, title: String) -> Note {
        let mut all = self.all.lock().unwrap();
        let note = Note { id: all.len() as u32 + 1, title };
        all.push(note.clone());
        note
    }
}

#[injectable]
pub struct NotesController {
    notes: Dep<Notes>,
}

#[routes]
impl NotesController {
    #[ulo_http::post("/notes")]
    fn create(&self, Valid(Json(new)): Valid<Json<NewNote>>) -> Created<Json<Note>> {
        let note = self.notes.add(new.title);
        Created::at(format!("/notes/{}", note.id), Json(note))
    }

    #[ulo_http::get("/notes/{id}")]
    fn find(&self, Path(id): Path<u32>) -> Found {
        let all = self.notes.all.lock().unwrap();
        all.iter().find(|note| note.id == id).cloned().map(Json).ok_or(NoteNotFound(id))
    }

    #[ulo_http::get("/notes")]
    fn list(&self, Query(page): Query<Page>) -> WithHeaders<Json<Vec<Note>>> {
        let all = self.notes.all.lock().unwrap();
        let shown = all.iter().take(page.limit.unwrap_or(usize::MAX)).cloned().collect();
        WithHeaders::new(Json(shown)).header("x-total-count", all.len().to_string())
    }

    #[ulo_http::get("/agent")]
    fn agent(&self, agent: Header<UserAgent>) -> String {
        agent.as_str().to_owned()
    }
}

#[module(providers = [Notes], controllers = [NotesController, intake::IntakeController])]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
