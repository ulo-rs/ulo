async fn run_ws_connection(
    socket: axum::extract::ws::WebSocket,
    callbacks: Arc<WsConnectionCallbacks>,
    parts: RequestPart,
) {
    let (write, read) = socket.split();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<WsMessage>(32);

    tokio::spawn(async move {
        let mut write = write;
        while let Some(msg) = rx.recv().await {
            if let Ok(axum_msg) = ws_message_to_axum(msg) {
                if write.send(axum_msg).await.is_err() {
                    break;
                }
            }
        }
    });

    let sender: Arc<dyn WsSink> = Arc::new(TokioSender::new(tx));

    let client_id = match callbacks.connect(parts, sender.clone()).await {
        Ok(id) => id,
        Err(e) => {
            // The handshake is already done, so a refusal is answered the only
            // way the protocol leaves: the canonical envelope, then a close
            // carrying the code for it.
            for frame in ulo::ws::refusal_frames(&e) {
                let _ = sender.send(frame).await;
            }
            return;
        }
    };

    tracing::debug!(client_id = %client_id, "WebSocket connection established");

    let stream_tasks: Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let stream_tasks_inner = stream_tasks.clone();

    let mut read = read;
    let panicked = std::panic::AssertUnwindSafe(async {
        while let Some(result) = read.next().await {
            match result {
                Ok(axum_msg) => match axum_to_ws_message(axum_msg) {
                    Ok(ws_msg) => match callbacks.message(client_id.clone(), ws_msg).await {
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
                    },
                    Err(_) => {}
                },
                Err(_) => break,
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
    tracing::debug!(client_id = %client_id, "WebSocket connection closed");
    callbacks.disconnect(client_id).await;
}
