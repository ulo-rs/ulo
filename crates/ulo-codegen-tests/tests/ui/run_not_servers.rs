// `run` takes one `ulo::Server` or a tuple of two to eight, through `ulo::Servers`. Refused at:
// the argument, with `Servers`'s own message, for a value that is not a server, a tuple of nine
// servers and a tuple holding one value that is not a server.
use ulo::{BoundAddr, BoxError, DrainToken, Mounted, Server, Transport};

struct Greeting;

impl Transport for Greeting {
    const KEY: &'static str = "greeting";
    type Cx = ();
    type Reply = ();
}

struct Greeter;

impl Server for Greeter {
    type Transport = Greeting;

    async fn bind(&mut self, _mounted: Mounted<'_, Greeting>) -> Result<(), BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        Ok(())
    }

    async fn drain(&self, _token: DrainToken) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }

    fn bound(&self) -> Vec<BoundAddr> {
        Vec::new()
    }
}

#[ulo::module]
struct AppModule;

fn main() {
    let _ = ulo_runtime_tokio::run(AppModule, "127.0.0.1:3000");
    let _ = ulo_runtime_tokio::run(AppModule, (Greeter, Greeter, Greeter, Greeter, Greeter, Greeter, Greeter, Greeter, Greeter));
    let _ = ulo_runtime_tokio::run(AppModule, (Greeter, "127.0.0.1:3000"));
}
