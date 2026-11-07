// Checks private byte leases and publication ordering.
use super::*;
use tokio_util::codec::LinesCodec;

/// Creates a bus with a test-sized budget and a live failure observer.
fn bus(bytes: usize) -> (Events, watch::Receiver<bool>) {
    let (sender, _) = broadcast::channel(1024);
    let (failed, failure) = watch::channel(false);
    (Events {
        sender, budget: Arc::new(Semaphore::new(bytes)), terminal: Mutex::new(None),
        stopping: Arc::new(AtomicBool::new(false)), failed,
    }, failure)
}

/// Accounts for raw UTF-8 JSON bytes, including whitespace and escaping.
#[test]
fn exact_payload_charge() {
    for raw in [" { \"text\" : \"\\u0000\\n🦀\" } ", "{\"items\":[[],{},[],{},null]}"] {
        let (events, _) = bus(raw.len());
        let mut receiver = events.sender.subscribe();
        assert!(events.publish(raw.into()));
        assert_eq!(events.budget.available_permits(), 0);
        let value = receiver.try_recv().unwrap();
        let copy = value.clone();
        drop(value);
        assert_eq!(events.budget.available_permits(), 0);
        assert_eq!(copy.value(), serde_json::from_str::<Value>(raw).unwrap());
        drop(copy);
        assert_eq!(events.budget.available_permits(), raw.len());
        assert!(events.publish(raw.into()));
        drop(receiver);
        assert_eq!(events.budget.available_permits(), raw.len());
        assert!(events.publish(raw.into()));
        assert_eq!(events.budget.available_permits(), raw.len());
    }
}

/// Later intentional cleanup cannot overwrite failure or publish another ordinary event.
#[test]
fn terminal_is_final() {
    let (events, _) = bus(16);
    let mut receiver = events.sender.subscribe();
    assert!(matches!(events.finish(End::Overloaded), End::Overloaded));
    events.stopping.store(true, Ordering::Release);
    assert!(matches!(events.finish(End::Expected), End::Overloaded));
    assert!(!events.publish("{\"later\":true}".into()));
    assert!(receiver.try_recv().is_err());
    assert_eq!(events.budget.available_permits(), 16);
    assert_eq!(Notification::terminal(End::Overloaded).value()["params"]["expected"], false);
}

/// A byte failure latches once and reserves no partial capacity.
#[test]
fn failed_charge_releases_nothing() {
    let (events, failure) = bus(1);
    let _receiver = events.sender.subscribe();
    assert!(!events.publish("{}".into()));
    assert!(*failure.borrow());
    assert!(matches!(events.finish(End::Expected), End::Overloaded));
    assert_eq!(events.budget.available_permits(), 1);
}

/// A generated failure rejects new calls and discards an already queued mutation.
#[tokio::test]
async fn overload_prevents_queued_mutation() {
    let (transport, peer) = tokio::io::duplex(4096);
    let (output, input) = tokio::io::split(transport);
    let observed = tokio::spawn(async move {
        let (reader, mut writer) = tokio::io::split(peer);
        let mut lines = FramedRead::new(reader, LinesCodec::new());
        let mut mutations = Vec::new();
        while let Some(Ok(line)) = lines.next().await {
            let frame: Value = serde_json::from_str(&line).unwrap();
            match frame["method"].as_str().unwrap() {
                "initialize" => writer.write_all(format!("{}\n", json!({
                    "id": frame["id"], "result": {},
                })).as_bytes()).await.unwrap(),
                "initialized" => {},
                "turn/start" => mutations.push(frame),
                _ => panic!("unexpected fixture method"),
            }
        }
        mutations
    });
    let client = Client::transport(output, input, None).await.unwrap();
    let _receiver = client.subscribe();
    // No await occurs between queueing and overload, so the reader sees both ready.
    client.outbound.try_send(json!({ "id": 800, "method": "turn/start", "params": {} }))
        .unwrap();
    assert!(!client.events.publish(json!({ "method": "miniclaw/approvalResolved",
        "params": { "requestId": "x".repeat(EVENT_BUDGET) } })
        .to_string().into_boxed_str()));
    assert!(!client.available(), "latched overload still admitted RPC work");
    assert!(client.call("turn/start", json!({})).await.is_err());
    tokio::time::timeout(Duration::from_secs(3), client.closed()).await.unwrap();
    assert!(observed.await.unwrap().is_empty(), "queued mutation reached the peer");
}

struct Refill {
    writer: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    outbound: Arc<Mutex<Option<mpsc::Sender<Value>>>>,
    remaining: Arc<std::sync::atomic::AtomicUsize>,
}

impl AsyncWrite for Refill {
    /// Replenishes outbound work synchronously so the queue cannot briefly run empty.
    fn poll_write(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>,
        bytes: &[u8]) -> std::task::Poll<std::io::Result<usize>> {
        let result = std::pin::Pin::new(&mut self.writer).poll_write(cx, bytes);
        if matches!(result, std::task::Poll::Ready(Ok(count)) if count > 0)
            && self.remaining.fetch_update(Ordering::SeqCst, Ordering::SeqCst,
                |count| count.checked_sub(1)).is_ok()
        {
            self.outbound.lock().unwrap().as_ref().unwrap()
                .try_send(json!({ "method": "fixture/work" })).unwrap();
        }
        result
    }

    /// Forwards flush requests to the underlying controlled peer stream.
    fn poll_flush(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>)
        -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.writer).poll_flush(cx)
    }

    /// Forwards shutdown without retaining a production client or shutdown sender.
    fn poll_shutdown(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>)
        -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.writer).poll_shutdown(cx)
    }
}

/// Ready RPC replies progress while outbound work stays continuously ready.
#[tokio::test]
async fn queued_writes_do_not_starve_input() {
    let (transport, peer) = tokio::io::duplex(4096);
    let (output, writer) = tokio::io::split(transport);
    let outbound = Arc::new(Mutex::new(None));
    let remaining = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = tokio::spawn(async move {
        let (reader, mut writer) = tokio::io::split(peer);
        let mut lines = FramedRead::new(reader, LinesCodec::new());
        while let Some(Ok(line)) = lines.next().await {
            let frame: Value = serde_json::from_str(&line).unwrap();
            let reply = match frame["method"].as_str().unwrap() {
                "initialize" => Some(json!({ "id": frame["id"], "result": {} })),
                "initialized" => None,
                "fixture/reply" => Some(json!({ "id": frame["id"], "result": "ready" })),
                "fixture/work" => None,
                _ => panic!("unexpected fixture method"),
            };
            if let Some(reply) = reply {
                writer.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
            }
        }
    });
    let client = Client::transport(output, Refill {
        writer, outbound: outbound.clone(), remaining: remaining.clone(),
    }, None).await.unwrap();
    *outbound.lock().unwrap() = Some(client.outbound.clone());
    remaining.store(1024, Ordering::SeqCst);
    client.outbound.try_send(json!({ "method": "fixture/work" })).unwrap();
    assert_eq!(client.call("fixture/reply", json!({})).await.unwrap(), "ready");
    assert!(remaining.load(Ordering::SeqCst) > 0,
        "reader waited until all continuously queued writes were exhausted");
    client.stop().await;
    observed.await.unwrap();
}

#[tokio::test]
async fn unclaimed_scoped_tool_is_refused_without_waiting() {
    let (transport, peer) = tokio::io::duplex(4096);
    let (output, input) = tokio::io::split(transport);
    let observed = tokio::spawn(async move {
        let (reader, mut writer) = tokio::io::split(peer);
        let mut lines = FramedRead::new(reader, LinesCodec::new());
        let initialize: Value = serde_json::from_str(&lines.next().await.unwrap().unwrap()).unwrap();
        writer.write_all(format!("{}\n", json!({ "id": initialize["id"], "result": {} }))
            .as_bytes()).await.unwrap();
        let initialized: Value = serde_json::from_str(&lines.next().await.unwrap().unwrap()).unwrap();
        assert_eq!(initialized["method"], "initialized");
        writer.write_all(format!("{}\n", json!({ "id": 99, "method": "item/tool/call",
            "params": { "tool": "miniclaw_scoped_native", "threadId": "thread",
                "turnId": "turn", "callId": "call", "arguments": {} } })).as_bytes())
            .await.unwrap();
        serde_json::from_str::<Value>(&lines.next().await.unwrap().unwrap()).unwrap()
    });
    let client = Client::transport(output, input, None).await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(2), observed).await.unwrap().unwrap();
    assert_eq!(reply["id"], 99);
    assert_eq!(reply["result"]["success"], false);
    client.stop().await;
}

#[tokio::test]
async fn claimed_scoped_tool_reaches_one_bounded_receiver() {
    let (transport, peer) = tokio::io::duplex(4096);
    let (output, input) = tokio::io::split(transport);
    let (ready, send) = oneshot::channel();
    let observed = tokio::spawn(async move {
        let (reader, mut writer) = tokio::io::split(peer);
        let mut lines = FramedRead::new(reader, LinesCodec::new());
        let initialize: Value = serde_json::from_str(&lines.next().await.unwrap().unwrap()).unwrap();
        writer.write_all(format!("{}\n", json!({ "id": initialize["id"], "result": {} }))
            .as_bytes()).await.unwrap();
        let initialized: Value = serde_json::from_str(&lines.next().await.unwrap().unwrap()).unwrap();
        assert_eq!(initialized["method"], "initialized");
        let _ = send.await;
        writer.write_all(format!("{}\n", json!({ "id": 100, "method": "item/tool/call",
            "params": { "tool": "miniclaw_scoped_native", "threadId": "thread",
                "turnId": "turn", "callId": "call", "arguments": {} } })).as_bytes())
            .await.unwrap();
        serde_json::from_str::<Value>(&lines.next().await.unwrap().unwrap()).unwrap()
    });
    let client = Client::transport(output, input, None).await.unwrap();
    let mut requests = client.take_tool_requests().unwrap();
    assert!(client.take_tool_requests().is_none());
    ready.send(()).unwrap();
    let request = tokio::time::timeout(Duration::from_secs(2), requests.recv())
        .await.unwrap().unwrap();
    assert_eq!(request.params["tool"], "miniclaw_scoped_native");
    request.answer.send(json!({ "success": true, "contentItems": [
        { "type": "inputText", "text": "synthetic-only" }
    ] })).unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(2), observed).await.unwrap().unwrap();
    assert_eq!(reply["id"], 100);
    assert_eq!(reply["result"]["contentItems"][0]["text"], "synthetic-only");
    client.stop().await;
}

/// A reply larger than one frame fails the call that waited for it and nothing else: the
/// connection stays up, the process is not stopped, and the next call is answered.
#[tokio::test]
async fn an_oversized_frame_fails_the_waiting_call_and_keeps_the_connection() {
    let (transport, peer) = tokio::io::duplex(1 << 20);
    let (output, input) = tokio::io::split(transport);
    let observed = tokio::spawn(async move {
        let (reader, mut writer) = tokio::io::split(peer);
        let mut lines = FramedRead::new(reader, LinesCodec::new());
        while let Some(Ok(line)) = lines.next().await {
            let frame: Value = serde_json::from_str(&line).unwrap();
            let reply = match frame["method"].as_str().unwrap() {
                "initialize" => Some(json!({ "id": frame["id"], "result": {} }).to_string()),
                "initialized" => None,
                "fixture/huge" => Some(format!("{{\"id\":{},\"result\":\"{}\"}}",
                    frame["id"], "x".repeat(MAX_FRAME))),
                "fixture/reply" => Some(json!({ "id": frame["id"], "result": "ready" }).to_string()),
                _ => panic!("unexpected fixture method"),
            };
            if let Some(reply) = reply {
                writer.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
            }
        }
    });
    let client = Client::transport(output, input, None).await.unwrap();
    assert_eq!(client.call("fixture/huge", json!({})).await, Err(Failure::Unavailable));
    assert!(client.available(), "an unreadable reply ended the connection");
    assert_eq!(client.call("fixture/reply", json!({})).await.unwrap(), "ready");
    client.stop().await;
    observed.await.unwrap();
}

/// The frame decoder keeps a line up to the limit, drops a longer one whole and reads on, and
/// does so whether the long line arrives in one piece or in parts.
#[test]
fn a_long_line_is_dropped_and_the_next_one_is_read() {
    let mut frames = Frames::default();
    let mut buffer = BytesMut::from(&b"{\"a\":1}\r\n"[..]);
    assert_eq!(frames.decode(&mut buffer).unwrap(), Some(Frame::Line("{\"a\":1}".into())));
    assert_eq!(frames.decode(&mut buffer).unwrap(), None);
    // One piece: the long line and the next line arrive together.
    buffer.extend_from_slice(&vec![b'x'; MAX_FRAME + 1]);
    buffer.extend_from_slice(b"\nnext\n");
    assert_eq!(frames.decode(&mut buffer).unwrap(), Some(Frame::Oversized));
    assert_eq!(frames.decode(&mut buffer).unwrap(), Some(Frame::Line("next".into())));
    // In parts: the limit is passed before the newline is seen, and the rest is discarded.
    buffer.extend_from_slice(&vec![b'y'; MAX_FRAME + 1]);
    assert_eq!(frames.decode(&mut buffer).unwrap(), None);
    assert!(buffer.is_empty(), "an over-limit partial line is not kept");
    buffer.extend_from_slice(b"yyy\nlast\n");
    assert_eq!(frames.decode(&mut buffer).unwrap(), Some(Frame::Oversized));
    assert_eq!(frames.decode(&mut buffer).unwrap(), Some(Frame::Line("last".into())));
    assert_eq!(frames.decode(&mut buffer).unwrap(), None);
}
