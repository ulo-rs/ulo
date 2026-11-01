use ex_19_http_uploads::{AVATAR_LIMIT, SERVER_LIMIT, builder, server};
use ex_support::http::{Call, call};
use ex_support::{Served, serve};
use ulo_runtime_tokio::Tokio;

async fn app() -> Served {
    let bound = builder().runtime(Tokio::current()).wire().unwrap().connect().await.unwrap().bind(server("127.0.0.1:0")).listen().await.unwrap();
    serve(bound)
}

const CONTENT_TYPE: &str = "multipart/form-data; boundary=XB";

fn form(file: &str, content: &str) -> String {
    format!(
        "--XB\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhello\r\n\
         --XB\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"{file}\"\r\nContent-Type: text/plain\r\n\r\n{content}\r\n--XB--\r\n"
    )
}

fn upload(path: &str, body: String) -> Call {
    Call::post(path).header("content-type", CONTENT_TYPE).body(body)
}

#[tokio::test]
async fn a_file_part_is_stored_and_read_back_byte_for_byte() {
    let server = app().await;

    let stored = call(server.addr, upload("/files", form("a.txt", "line one\nline two"))).await;
    assert_eq!((stored.status, stored.body.as_str()), (201, "stored"));
    assert_eq!(stored.header("location"), Some("/files/a.txt"));

    let read = call(server.addr, Call::get("/files/a.txt")).await;
    assert_eq!((read.status, read.body.as_str()), (200, "line one\nline two"));
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn a_body_over_the_server_limit_is_a_413_and_nothing_is_stored() {
    let server = app().await;
    let big = "x".repeat(SERVER_LIMIT as usize);

    let refused = call(server.addr, upload("/files", form("big.txt", &big))).await;
    assert_eq!(refused.status, 413);
    assert_eq!(call(server.addr, Call::get("/files/big.txt")).await.status, 404);
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn a_route_limit_replaces_the_servers_so_the_same_body_fits_one_route_and_not_the_other() {
    let server = app().await;
    let body = form("me.png", &"p".repeat(AVATAR_LIMIT as usize + 1));
    assert!((body.len() as u64) < SERVER_LIMIT);

    let files = call(server.addr, upload("/files", body.clone())).await;
    assert_eq!(files.status, 201);
    let avatars = call(server.addr, upload("/avatars", body)).await;
    assert_eq!(avatars.status, 413);

    let small = call(server.addr, upload("/avatars", form("s.png", "p"))).await;
    assert_eq!((small.status, small.header("location")), (201, Some("/files/s.png")), "a form under {AVATAR_LIMIT} bytes, boundaries included, fits the route");
    server.stop().await.expect("closes clean");
}

#[tokio::test]
async fn a_body_that_breaks_the_boundary_is_a_400_and_a_form_without_a_file_is_a_400_too() {
    let server = app().await;

    let broken = call(server.addr, upload("/files", "--XB\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"a.txt\"\r\n\r\nno closing boundary".to_owned())).await;
    assert_eq!(broken.status, 400);

    let no_file = call(
        server.addr,
        upload("/files", "--XB\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhello\r\n--XB--\r\n".to_owned()),
    )
    .await;
    assert_eq!(no_file.status, 400);
    assert!(no_file.body.contains("the form carried no file"), "{}", no_file.body);

    let wrong = call(server.addr, Call::post("/files").json("{}")).await;
    assert_eq!(wrong.status, 415);
    server.stop().await.expect("closes clean");
}
