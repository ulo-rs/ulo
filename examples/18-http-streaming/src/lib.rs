use std::convert::Infallible;
use std::sync::Mutex;
use std::time::Duration;

use futures_util::stream::{self, BoxStream};
use futures_util::{Stream, StreamExt};
use tokio::sync::{Semaphore, watch};
use ulo::{App, AppBuilder, Dep, ExecutionRef, injectable, module, routes};
use ulo_http::{Body, Bytes, Event, EventId, LastEventId, Path, Sse};

pub const KEEP_ALIVE: Duration = Duration::from_secs(15);

/// An append-only log of messages whose readers wait for the next one.
pub struct Feed {
    log: Mutex<Vec<String>>,
    appended: watch::Sender<usize>,
}

#[injectable]
impl Feed {
    fn new() -> Self {
        Feed { log: Mutex::new(Vec::new()), appended: watch::channel(0).0 }
    }
}

impl Feed {
    pub fn publish(&self, text: &str) {
        let mut log = self.log.lock().unwrap();
        log.push(text.to_owned());
        self.appended.send_replace(log.len());
    }

    /// Every message after the first `after`, then each new one as it is published. The message
    /// at position `n` (from 1) carries the id `n`.
    fn since(this: Dep<Feed>, after: usize) -> impl Stream<Item = Event> + Send {
        let appended = this.appended.subscribe();
        stream::unfold((this, appended, after), |(feed, mut appended, next)| async move {
            loop {
                let text = feed.log.lock().unwrap().get(next).cloned();
                if let Some(text) = text {
                    let id = EventId::new((next + 1).to_string()).expect("digits are a valid id");
                    return Some((Event::default().id(id).data(text), (feed, appended, next + 1)));
                }
                appended.changed().await.ok()?;
            }
        })
    }
}

/// Lets `/export` write its next row only when told to.
pub struct Pace(pub Semaphore);

#[injectable]
impl Pace {
    fn new() -> Self {
        Pace(Semaphore::new(0))
    }
}

#[injectable]
pub struct StreamController {
    feed: Dep<Feed>,
    pace: Dep<Pace>,
}

#[routes]
impl StreamController {
    /// Resumes after `Last-Event-ID`, and ends with the drain instead of holding it open.
    #[ulo_http::get("/events")]
    fn events(&self, last: LastEventId, exec: ExecutionRef) -> Sse<BoxStream<'static, Event>> {
        let after = last.0.and_then(|id| id.parse().ok()).unwrap_or(0);
        let drained = async move { exec.draining().await };
        Sse::new(Feed::since(self.feed.clone(), after).take_until(drained).boxed()).keep_alive(KEEP_ALIVE)
    }

    #[ulo_http::post("/events/{text}")]
    fn publish(&self, Path(text): Path<String>) -> &'static str {
        self.feed.publish(&text);
        "published"
    }

    /// Three rows, each written when the previous one has been read and `Pace` allows the next.
    #[ulo_http::get("/export")]
    fn export(&self) -> Body {
        let pace = self.pace.clone();
        Body::stream(stream::unfold(0, move |row| {
            let pace = pace.clone();
            async move {
                if row == 3 {
                    return None;
                }
                if row > 0 {
                    pace.0.acquire().await.ok()?.forget();
                }
                Some((Ok::<_, Infallible>(Bytes::from(format!("row {}\n\n", row + 1))), row + 1))
            }
        }))
    }
}

#[module(providers = [Feed, Pace], controllers = [StreamController])]
pub struct AppModule;

pub fn builder() -> AppBuilder {
    App::builder(AppModule)
}
