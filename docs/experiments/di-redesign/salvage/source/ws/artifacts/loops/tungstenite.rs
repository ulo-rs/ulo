async fn run_ws_connection(
    ws_stream: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    callbacks: Arc<WsConnectionCallbacks>,
) {
    let (write, read) = ws_stream.split();
    let (tx, mut rx) = mpsc::channel::<WsMessage>(32);

    tokio::spawn(async move {
        let mut write = write;
        while let Some(msg) = rx.recv().await {
            if let Ok(m) = ws_message_to_tungstenite(msg) {
                if write.send(m).await.is_err() {
                    break;
                }
            }
        }
    });

    let sender: Arc<dyn WsSink> = Arc::new(TokioSender::new(tx));

    // Raw TCP has no HTTP upgrade request; pass synthetic empty parts.
    let parts: RequestPart = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri("/")
        .body(())
        .unwrap()
        .into_parts()
        .0;
    let client_id = match callbacks.connect(parts, sender.clone()).await {
        Ok(id) => id,
        Err(e) => {
            tracing::debug!(error = %e, "WebSocket connect rejected by guard or handler");
            // The handshake is already done, so a refusal is answered the only
            // way the protocol leaves: the canonical envelope, then a close
            // carrying the code for it.
            for frame in ulo::ws::refusal_frames(&e) {
                let _ = sender.send(frame).await;
            }
            return;
        }
    };

    let stream_tasks: Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let stream_tasks_inner = stream_tasks.clone();

    let mut read = read;
    let panicked = std::panic::AssertUnwindSafe(async {
        while let Some(result) = read.next().await {
            match result {
                Ok(Message::Text(t)) => {
                    match callbacks
                        .message(client_id.clone(), WsMessage::Text(t.to_string()))
                        .await
                    {
                        MessageCallbackResult::Continue => {}
                        MessageCallbackResult::Stop => break,
                        MessageCallbackResult::Stream(stream) => {
                            let sink = sender.clone();
                            let handle = tokio::spawn(async move {
                                use futures_util::StreamExt;
                                tokio::pin!(stream);
                                while let Some(msg) = stream.next().await {
                                    let _ = sink.send(msg).await;
                                }
                            });
                            stream_tasks_inner.lock().unwrap().push(handle);
                        }
                    }
                }
                Ok(Message::Binary(b)) => {
                    match callbacks
                        .message(client_id.clone(), WsMessage::Binary(b.to_vec()))
                        .await
                    {
                        MessageCallbackResult::Continue => {}
                        MessageCallbackResult::Stop => break,
                        MessageCallbackResult::Stream(stream) => {
                            let sink = sender.clone();
                            let handle = tokio::spawn(async move {
                                use futures_util::StreamExt;
                                tokio::pin!(stream);
                                while let Some(msg) = stream.next().await {
                                    let _ = sink.send(msg).await;
                                }
                            });
                            stream_tasks_inner.lock().unwrap().push(handle);
                        }
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
            }
        }
    })
    .catch_unwind()
    .await
    .is_err();

    for handle in stream_tasks.lock().unwrap().drain(..) {
        handle.abort();
    }

    if panicked {
        tracing::error!(client_id = %client_id, "WebSocket handler panicked; closing connection");
    }
    callbacks.disconnect(client_id).await;
}
