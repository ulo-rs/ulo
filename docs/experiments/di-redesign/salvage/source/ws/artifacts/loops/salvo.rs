async fn run_ws_connection(
    ws: salvo::websocket::WebSocket,
    callbacks: Arc<WsConnectionCallbacks>,
    parts: RequestPart,
) {
    let (write, read) = ws.split();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<WsMessage>(32);

    tokio::spawn(async move {
        let mut write = write;
        while let Some(msg) = rx.recv().await {
            match ws_message_to_salvo(msg) {
                Ok(salvo_msg) => {
                    if let Err(e) = write.send(salvo_msg).await {
                        tracing::debug!(error = %e, "WebSocket write failed; closing write task");
                        break;
                    }
                }
                Err(e) => {
                    tracing::debug!(error = %e, "skipping outbound message: convert failed");
                }
            }
        }
    });

    let sender: Arc<dyn WsSink> = Arc::new(TokioSender::new(tx));

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

    tracing::debug!(client_id = %client_id, "WebSocket connection established");

    let stream_tasks: Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let stream_tasks_inner = stream_tasks.clone();

    let mut read = read;
    while let Some(result) = read.next().await {
        match result {
            Ok(salvo_msg) => match salvo_to_ws_message(salvo_msg) {
                Ok(ws_msg) => match callbacks.message(client_id.clone(), ws_msg).await {
                    MessageCallbackResult::Continue => {}
                    MessageCallbackResult::Stop => break,
                    MessageCallbackResult::Stream(stream) => {
                        let sink = sender.clone();
                        let handle = tokio::spawn(async move {
                            tokio::pin!(stream);
                            while let Some(msg) = stream.next().await {
                                if let Err(e) = sink.send(msg).await {
                                    tracing::debug!(
                                        error = %e,
                                        "stream task: sink closed; ending stream"
                                    );
                                    break;
                                }
                            }
                        });
                        stream_tasks_inner.lock().unwrap().push(handle);
                    }
                },
                // Close frames return ConnectionClosed — log at debug, not warn.
                Err(e) => {
                    tracing::debug!(client_id = %client_id, error = %e, "ending read loop");
                    break;
                }
            },
            Err(e) => {
                tracing::debug!(client_id = %client_id, error = %e, "salvo WebSocket read error");
                break;
            }
        }
    }

    for handle in stream_tasks.lock().unwrap().drain(..) {
        handle.abort();
    }

    tracing::debug!(client_id = %client_id, "WebSocket connection closed");
    callbacks.disconnect(client_id).await;
}
