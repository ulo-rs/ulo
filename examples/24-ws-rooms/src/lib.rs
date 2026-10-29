use serde::Deserialize;
use ulo::{App, AppBuilder, Dep, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_ws::{BroadcastError, Payload, Rooms, WsCx, WsModule};
use ulo_ws_redis::Redis;

#[derive(Deserialize)]
pub struct Shout {
    pub room: String,
    pub text: String,
}

#[injectable]
pub struct Lobby;

#[routes]
#[ulo_ws::gateway(path = "/rooms", port = own)]
impl Lobby {
    #[ulo_ws::message("join")]
    async fn join(&self, room: Payload<String>, cx: WsCx) {
        cx.conn().join(room.0).await;
    }

    /// To every member of the room, whichever process holds the connection.
    #[ulo_ws::message("shout")]
    async fn shout(&self, shout: Payload<Shout>, rooms: Dep<Rooms>) -> Result<(), BroadcastError> {
        rooms.to_room(shout.0.room).emit("shouted", &shout.0.text).await
    }

    #[ulo_ws::message("ping")]
    fn ping(&self) -> &'static str {
        "pong"
    }
}

/// The module chooses the broadcast adapter: in memory, which reaches this process's connections
/// only, or Redis, which carries a broadcast to every process on the same URL.
pub struct Root {
    redis: Option<String>,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        match &self.redis {
            Some(url) => m.import(WsModule::for_root().broadcast(Redis::url(url))),
            None => m.import(WsModule::for_root()),
        };
        m.controller::<Lobby>();
    }
}

pub fn builder(redis_url: Option<String>) -> AppBuilder {
    App::builder(Root { redis: redis_url })
}
