// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering::SeqCst;

use dapper_config::DapperConfig;
use dapper_dap_protocol::data_types::Seq;
use dapper_dap_protocol::protocol as dap;
use dapper_dap_protocol::protocol::Message;
use dapper_dap_protocol::requests::RequestCommand;
use dapper_session::SessionId;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::backend::Backend;
use crate::client::ClientId;
use crate::client::Command;
use crate::client::CommandResult;
use crate::client::ControlCommand;
use crate::client::ControlResult;
use crate::client::EventChannel;
use crate::client::ListenerPayload;
use crate::client::ProxyClient;
use crate::client::ProxyRequest;
use crate::debug_session_tracker::ClientType;
use crate::debug_session_tracker::DebugSessionTracker;
use crate::transport::DuplexChannel;
use crate::transport::ReadChannel;
use crate::transport::WriteChannel;

type DAPClient = DuplexChannel<Message>;

#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
pub struct ClientSeq(Seq);
#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
pub struct BackendSeq(Seq);

#[derive(Debug, Default)]
struct MessageRemapperInner {
    pub forward: HashMap<ClientSeq, BackendSeq>,
    pub reverse: HashMap<BackendSeq, ClientSeq>,
}

#[derive(Debug, Clone)]
pub struct MessageRemapper {
    inner: Arc<Mutex<MessageRemapperInner>>,
}

impl MessageRemapper {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MessageRemapperInner::default())),
        }
    }

    pub fn map(&self, client_seq: ClientSeq, backend_seq: BackendSeq) -> BackendSeq {
        match self.inner.lock() {
            Ok(mut inner) => {
                if let Some(old_backend_seq) = inner.forward.insert(client_seq, backend_seq) {
                    inner.reverse.remove(&old_backend_seq);
                }
                inner.reverse.insert(backend_seq, client_seq);
                backend_seq
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to acquire MessageRemapper lock for map");
                backend_seq
            }
        }
    }

    pub fn unmap(&self, backend_seq: BackendSeq) -> Option<ClientSeq> {
        match self.inner.lock() {
            Ok(mut inner) => {
                let client_seq = inner.reverse.remove(&backend_seq);
                if let Some(client_seq) = client_seq {
                    inner.forward.remove(&client_seq);
                }
                client_seq
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to acquire MessageRemapper lock for unmap");
                None
            }
        }
    }

    /// Read-only forward lookup: client seq → backend seq.
    ///
    /// Used to translate references inside other messages (e.g. `cancel`'s
    /// `requestId`) without consuming the mapping — the referenced request is
    /// still in flight and its response will trigger the eventual `unmap`.
    ///
    /// `None` indicates either a missing mapping or a poisoned mutex (the
    /// latter is logged at warn). Callers treat both the same way: they
    /// cannot translate the reference and proceed defensively.
    pub fn lookup_backend(&self, client_seq: ClientSeq) -> Option<BackendSeq> {
        match self.inner.lock() {
            Ok(inner) => inner.forward.get(&client_seq).copied(),
            Err(e) => {
                tracing::warn!(error = %e, "Failed to acquire MessageRemapper lock for lookup_backend");
                None
            }
        }
    }
}

/// Translate a `cancel` request's `requestId` from the client's seq frame to
/// the backend's frame. Other request kinds and cancels without a `requestId`
/// (or with only a `progressId`) pass through unchanged. On a lookup miss,
/// the `requestId` is forwarded unchanged (logged at debug level — the
/// common cause is a benign cancel/response race; rationale in the function
/// body).
///
/// IMPORTANT: only call from the main-client → backend path. Secondary
/// clients (control plane / agents) already produce `requestId` values in
/// the backend's seq frame and must bypass this translation — invoking it
/// on that path would silently double-translate.
fn translate_cancel(request: &mut dap::Request, remapper: &MessageRemapper) {
    let RequestCommand::Cancel(Some(args)) = &mut request.command else {
        return;
    };
    let Some(referenced_client_seq) = args.request_id else {
        return;
    };
    let referenced = ClientSeq(Seq::from(referenced_client_seq));
    match remapper.lookup_backend(referenced) {
        Some(backend) => {
            args.request_id = Some(i64::from(backend.0));
        }
        None => {
            // Either the referenced request already received its response
            // (and `unmap` consumed the mapping — a benign cancel/response
            // race), or the client referenced a seq it never sent. Forward
            // unchanged — mutating the client's intent silently is worse
            // than letting the backend apply its own "no such request"
            // semantics, and preserving the value keeps the cancel
            // observable in logs on both sides. The race case is normal
            // DAP behavior, so this is logged at debug level rather than
            // warn to avoid production noise.
            tracing::debug!(
                client_request_id = referenced_client_seq,
                "cancel references unknown client request_id (response may have already arrived); forwarding unchanged"
            );
        }
    }
}

/// Shared backend writer that allows both the main client (VS Code) and
/// secondary clients (control plane) to write to the debug adapter.
///
/// Uses `tokio::sync::Mutex` so the lock can be held across `.await` points
/// (the `send` method flushes the underlying writer). In the common case
/// only one task is writing at a time, so the lock is uncontended and cheap.
type SharedBackendWriter = Arc<tokio::sync::Mutex<WriteChannel<Message>>>;

pub struct ProxyServer {
    backend: Backend<Message>,
    /// This receiver is the single stream of messages that will be consumed by the backend
    to_backend_rx: mpsc::UnboundedReceiver<ProxyRequest>,
    /// This sender will be cloned and passed to the client
    to_backend_tx: mpsc::UnboundedSender<ProxyRequest>,
    /// This broadcast sender will be used to supply a stream of messages to listener tasks.
    /// Uses Arc<Message> so receivers clone the Arc (atomic refcount) instead of deep-cloning
    /// the entire Message (which contains nested Strings, IndexMaps, and Vecs).
    to_listeners_tx: broadcast::Sender<Arc<Message>>,
    /// EventChannel for proxy-generated events (cloned for each client)
    event_channel: EventChannel,
    /// Receiver for EventChannel messages to reach the main client
    event_channel_rx: mpsc::UnboundedReceiver<Message>,
    /// Tracks debug session state including breakpoints
    debug_session_tracker: DebugSessionTracker,
    /// Dapper configuration
    config: DapperConfig,
}

impl ProxyServer {
    pub fn new(
        backend: Backend<Message>,
        config: DapperConfig,
        session_id: SessionId,
        parent_session_id: Option<SessionId>,
    ) -> Self {
        let (to_backend_tx, to_backend_rx) = mpsc::unbounded_channel();
        let (to_listeners_tx, _) = broadcast::channel(8192);

        let (event_channel, event_channel_rx) = EventChannel::new_pair();

        let debug_session_tracker =
            DebugSessionTracker::new(session_id).with_parent_session_id(parent_session_id);

        Self {
            backend,
            to_backend_rx,
            to_backend_tx,
            to_listeners_tx,
            event_channel,
            event_channel_rx,
            debug_session_tracker,
            config,
        }
    }

    pub fn create_client(&self, id: ClientId) -> ProxyClient {
        let event_channel = self.event_channel.clone();
        ProxyClient::new(
            id,
            self.to_backend_tx.clone(),
            event_channel,
            self.debug_session_tracker.clone(),
            self.config.clone(),
        )
    }

    pub fn get_debug_session_tracker(&self) -> DebugSessionTracker {
        self.debug_session_tracker.clone()
    }

    pub async fn run(self, main_client: DAPClient) -> anyhow::Result<()> {
        let (main_client_read, main_client_write) = main_client.into_channels();

        let remapper = MessageRemapper::new();

        let (backend_read, backend_write) = self.backend.duplex.into_channels();
        let shared_backend_write: SharedBackendWriter =
            Arc::new(tokio::sync::Mutex::new(backend_write));

        // Shared sequence counter so the main-client path and the
        // secondary-client path never collide on backend seq numbers.
        let backend_seq = Arc::new(AtomicI64::new(1));

        // The main client writes directly to the backend via the shared
        // writer, bypassing the mpsc→oneshot round-trip that secondary
        // clients (control plane) use. This eliminates the per-message
        // task context-switch that was serialising all VS Code requests
        // and causing visible latency on every F10/F11 step.
        let client_to_backend_task = Self::main_client_to_backend(
            main_client_read,
            shared_backend_write.clone(),
            remapper.clone(),
            self.debug_session_tracker.clone(),
            backend_seq.clone(),
        );

        let backend_to_main_client_and_listeners_task = Self::backend_to_main_client_and_listeners(
            backend_read,
            self.event_channel_rx,
            main_client_write,
            self.to_listeners_tx.clone(),
            remapper,
            self.debug_session_tracker.clone(),
        );

        let client_requests_task = Self::handle_client_requests(
            self.to_backend_rx,
            shared_backend_write,
            self.to_listeners_tx.clone(),
            backend_seq,
            self.debug_session_tracker.clone(),
        );

        // The first task to finish (normally the main client disconnecting)
        // tears the whole proxy down. A `select!` over spawned JoinHandles
        // would leak the losing tasks: `handle_client_requests` keeps a
        // clone of the shared backend writer alive, so the adapter's stdin
        // never closes and the `backend.handle` await below can hang
        // forever. A JoinSet lets us abort the survivors.
        let mut tasks = JoinSet::new();
        tasks.spawn(async move { ("Client to backend", client_to_backend_task.await) });
        tasks.spawn(async move {
            (
                "Backend to main client and listeners",
                backend_to_main_client_and_listeners_task.await,
            )
        });
        tasks.spawn(async move { ("Client requests", client_requests_task.await) });

        match tasks.join_next().await {
            Some(Ok((task_name, result))) => Self::handle_task_completion(task_name, Ok(result)),
            Some(Err(e)) => Self::handle_task_completion("Proxy pipeline", Err(e)),
            None => {}
        }

        // Abort the remaining tasks and wait for them to finish so the
        // shared backend writer is dropped before waiting on the adapter.
        tasks.shutdown().await;

        if let Some(handle) = self.backend.handle {
            handle.await??;
        }

        Ok(())
    }

    async fn handle_client_requests(
        mut to_backend_rx: mpsc::UnboundedReceiver<ProxyRequest>,
        backend_write: SharedBackendWriter,
        to_listeners_tx: broadcast::Sender<Arc<Message>>,
        backend_seq: Arc<AtomicI64>,
        debug_session_tracker: DebugSessionTracker,
    ) -> anyhow::Result<()> {
        while let Some(request) = to_backend_rx.recv().await {
            let client_id = &request.client_id;
            tracing::debug!("Processing request from client: {client_id:?}");

            let result = match request.command {
                Command::Control(control_command) => match control_command {
                    ControlCommand::Status => CommandResult::Control(ControlResult::Status),
                },
                Command::Debugger(message) => {
                    debug_session_tracker
                        .track_message_from_client(&message, ClientType::Secondary);

                    tracing::debug!(
                        "Forwarding message to backend from client {client_id:?}: {message:?}"
                    );

                    let mut seq = Seq::default();
                    let listener = to_listeners_tx.subscribe();
                    match message {
                        Message::Request(mut message) => {
                            seq = Seq(backend_seq.fetch_add(1, SeqCst));
                            message.seq = seq;

                            // Secondary clients see backend seqs directly via
                            // `ListenerPayload { seq, .. }` from previous
                            // requests they made and are required to build any
                            // seq references (e.g. `cancel.requestId`) using
                            // those backend-frame values. There is no type-
                            // level enforcement: do NOT forward `requestId`
                            // values from any external source (e.g. an MCP
                            // tool input piping a user-specified seq) through
                            // this path without translation. See the doc on
                            // `translate_cancel` for the matching invariant.
                            let msg: Message = message.into();
                            tracing::trace!(target: "dap", source = %DapSource::ControlPlane, message = ?msg);

                            let mut writer = backend_write.lock().await;
                            writer.send(msg).await?;
                        }
                        Message::Response(response) => {
                            let msg: Message = response.into();
                            tracing::trace!(target: "dap", source = %DapSource::ControlPlane, message = ?msg);

                            let mut writer = backend_write.lock().await;
                            writer.send(msg).await?;
                        }
                        Message::Event(_) | Message::Custom(_) => {
                            tracing::warn!(
                                "Unexpected message type from client {client_id:?}, ignoring"
                            );
                        }
                    }
                    let result_payload = ListenerPayload {
                        seq,
                        messages: listener,
                    };

                    CommandResult::Debugger(result_payload)
                }
            };
            if let Err(r) = request.result.send(result) {
                tracing::error!("Client disconnected before receiving result: {r:?}");
            }
        }

        Ok(())
    }

    async fn backend_to_main_client_and_listeners(
        mut backend_read: ReadChannel<Message>,
        mut event_channel_rx: mpsc::UnboundedReceiver<Message>,
        mut main_client: WriteChannel<Message>,
        to_listeners_tx: broadcast::Sender<Arc<Message>>,
        remapper: MessageRemapper,
        debug_session_tracker: DebugSessionTracker,
    ) -> anyhow::Result<()> {
        let event_seq_counter = AtomicI64::new(1);

        loop {
            let mut message = tokio::select! {
                // Messages from backend process (debugger)
                result = backend_read.recv() => {
                    match result? {
                        Some(msg) => msg,
                        None => break, // Backend closed
                    }
                }
                // Messages from EventChannel
                Some(msg) = event_channel_rx.recv() => msg,
            };

            tracing::trace!(target: "dap", source = %DapSource::Backend, message = ?message);

            // Only clone for broadcast when there are active listeners.
            // receiver_count() is an atomic load — essentially free.
            // Wrapping in Arc means receivers get a cheap refcount increment
            // instead of deep-cloning the entire Message.
            if to_listeners_tx.receiver_count() > 0 {
                let _ = to_listeners_tx.send(Arc::new(message.clone()));
            }

            // Assign sequence numbers for main client display
            let next_seq = Seq(event_seq_counter.fetch_add(1, SeqCst));
            match &mut message {
                Message::Event(event) => {
                    event.seq = next_seq;
                }
                Message::Response(response) => {
                    response.seq = next_seq;
                }
                _ => {}
            }

            // Remap sequence numbers and send to main client.
            // Responses are only sent if they match a request from the main client.
            // Track before sending to avoid cloning (send consumes the message).
            match &mut message {
                Message::Response(resp) => {
                    let backend_seq = BackendSeq(resp.request_seq);
                    if let Some(client_seq) = remapper.unmap(backend_seq) {
                        resp.request_seq = client_seq.0;
                        debug_session_tracker.track_message_to_client(&message);
                        main_client.send(message).await?;
                    }
                }
                _ => {
                    debug_session_tracker.track_message_to_client(&message);
                    main_client.send(message).await?;
                }
            }
        }

        Ok(())
    }

    /// Reads messages from the main VS Code client and writes them directly
    /// to the backend debug adapter. This bypasses `to_backend_tx` because
    /// the main client only sends DAP messages, never control commands.
    async fn main_client_to_backend(
        mut main_client: ReadChannel<Message>,
        backend_write: SharedBackendWriter,
        remapper: MessageRemapper,
        debug_session_tracker: DebugSessionTracker,
        backend_seq: Arc<AtomicI64>,
    ) -> anyhow::Result<()> {
        while let Some(message) = main_client.recv().await? {
            // Track messages from the client
            debug_session_tracker.track_message_from_client(&message, ClientType::Main);

            let client_seq = ClientSeq(message.seq());

            match message {
                Message::Request(mut request) => {
                    let seq = Seq(backend_seq.fetch_add(1, SeqCst));
                    request.seq = seq;

                    // Translate `cancel.requestId` BEFORE recording this
                    // request's own mapping. `client_seq` was captured from
                    // the original `message.seq()` above, so a malformed
                    // self-referencing cancel (`request_id == its own seq`)
                    // would, with the reverse order, find the just-inserted
                    // self-mapping and silently rewrite to its own backend
                    // seq. Translating first leaves such requests in the
                    // forward-unchanged branch.
                    translate_cancel(&mut request, &remapper);

                    // Map after translation so the response reader can find
                    // the mapping when the backend's response arrives.
                    remapper.map(client_seq, BackendSeq(seq));

                    let msg: Message = request.into();
                    tracing::trace!(target: "dap", source = %DapSource::MainClient, message = ?msg);
                    let mut writer = backend_write.lock().await;
                    writer.send(msg).await?;
                }
                Message::Response(response) => {
                    let msg: Message = response.into();
                    tracing::trace!(target: "dap", source = %DapSource::MainClient, message = ?msg);
                    let mut writer = backend_write.lock().await;
                    writer.send(msg).await?;
                }
                other => {
                    tracing::warn!(
                        message_type = ?other.message_type(),
                        "Unexpected message type from main client, ignoring"
                    );
                }
            }
        }

        Ok(())
    }

    /// Handles task completion, logging only actual errors while treating graceful shutdowns as info
    fn handle_task_completion(
        task_name: &str,
        result: Result<anyhow::Result<()>, tokio::task::JoinError>,
    ) {
        match result {
            Ok(Ok(())) => {
                // Task completed successfully (connection closed gracefully)
                tracing::info!("{} task completed (connection closed)", task_name);
            }
            Ok(Err(e)) => {
                // Task returned an error
                tracing::error!("{} task failed with error: {:#}", task_name, e);
            }
            Err(e) if e.is_cancelled() => {
                // Task was cancelled (normal during shutdown)
                tracing::debug!("{} task was cancelled", task_name);
            }
            Err(e) if e.is_panic() => {
                // Task panicked - this is a serious error
                tracing::error!("{} task panicked: {:#}", task_name, e);
            }
            Err(e) => {
                // Other error
                tracing::error!("{} task error: {:#}", task_name, e);
            }
        }
    }
}

#[derive(strum::Display)]
enum DapSource {
    ControlPlane,
    Backend,
    MainClient,
}

#[cfg(test)]
mod tests {
    use dapper_control_api::NavigateResult;
    use dapper_control_api::NavigationType;
    use dapper_dap_protocol::capabilities::Capabilities;
    use dapper_dap_protocol::data_types::Thread;
    use dapper_dap_protocol::data_types::ThreadId;
    use dapper_dap_protocol::enums::StoppedReason;
    use dapper_dap_protocol::events::EventKind;
    use dapper_dap_protocol::events::StoppedEventBody;
    use dapper_dap_protocol::protocol::Event;
    use dapper_dap_protocol::protocol::Request;
    use dapper_dap_protocol::protocol::Response;
    use dapper_dap_protocol::requests::InitializeRequestArguments;
    use dapper_dap_protocol::requests::RequestCommand;
    use dapper_dap_protocol::responses::ResponseBody;
    use dapper_dap_protocol::responses::ThreadsResponseBody;

    use super::*;
    use crate::backend::Backend;
    use crate::client::ClientId;
    use crate::transport::DuplexChannel;

    /// A helper that creates a ProxyServer wired to in-memory channels,
    /// spawns the proxy, and returns the client-side and backend-side
    /// endpoints for driving messages in tests.
    struct TestProxy {
        /// The "VS Code" side — send requests here, read responses from here
        main_client: DuplexChannel<Message>,
        /// The "debug adapter" side — read forwarded requests here, send responses here
        mock_backend: DuplexChannel<Message>,
        /// A secondary client (control plane) connected to the proxy
        proxy_client: ProxyClient,
        /// Handle to the spawned proxy task
        handle: tokio::task::JoinHandle<anyhow::Result<()>>,
    }

    impl TestProxy {
        fn new() -> Self {
            let (backend_server_side, mock_backend) = DuplexChannel::in_memory(4096);
            let (main_client_server_side, main_client) = DuplexChannel::in_memory(4096);

            let backend = Backend {
                duplex: backend_server_side,
                handle: None,
            };
            let config = DapperConfig::default();
            let proxy_server =
                ProxyServer::new(backend, config, SessionId::from("test-session"), None);

            let proxy_client = proxy_server.create_client(ClientId::new("test-control"));

            let handle = tokio::spawn(proxy_server.run(main_client_server_side));

            Self {
                main_client,
                mock_backend,
                proxy_client,
                handle,
            }
        }
    }

    fn make_threads_request(seq: i64) -> Message {
        let mut req = Request::new(RequestCommand::Threads);
        req.seq = Seq(seq);
        req.into()
    }

    fn make_threads_response(request_seq: Seq) -> Message {
        let resp = Response {
            seq: Seq(1), // Overwritten by the proxy; value here is arbitrary
            request_seq,
            success: true,
            message: None,
            body: ResponseBody::Threads(ThreadsResponseBody {
                threads: vec![Thread {
                    id: ThreadId(1),
                    name: "main".to_string(),
                }],
                ..Default::default()
            }),
        };
        resp.into()
    }

    fn make_stopped_event() -> Message {
        Event::new(EventKind::Stopped(StoppedEventBody {
            reason: StoppedReason::Breakpoint,
            thread_id: Some(ThreadId(1)),
            all_threads_stopped: Some(true),
            ..Default::default()
        }))
        .into()
    }

    fn make_cancel_request(seq: i64, request_id: Option<i64>) -> Request {
        let mut req = Request::new(RequestCommand::Cancel(Some(
            dapper_dap_protocol::requests::CancelArguments {
                request_id,
                ..Default::default()
            },
        )));
        req.seq = Seq(seq);
        req
    }

    #[tokio::test]
    async fn test_main_client_request_response_roundtrip() {
        let mut tp = TestProxy::new();

        // Main client sends a Threads request with seq=10
        tp.main_client.send(make_threads_request(10)).await.unwrap();

        // Mock backend receives the forwarded request (seq is remapped)
        let forwarded = tp.mock_backend.recv().await.unwrap().unwrap();
        let forwarded_req = match forwarded {
            Message::Request(r) => r,
            other => panic!("Expected Request, got {:?}", other.message_type()),
        };
        // The backend sees a remapped seq (starting from 1)
        assert_ne!(forwarded_req.seq, Seq(10), "seq should be remapped");

        // Mock backend sends a response matching the forwarded request's seq
        tp.mock_backend
            .send(make_threads_response(forwarded_req.seq))
            .await
            .unwrap();

        // Main client receives the response with request_seq remapped back to 10
        let response = tp.main_client.recv().await.unwrap().unwrap();
        let resp = match response {
            Message::Response(r) => r,
            other => panic!("Expected Response, got {:?}", other.message_type()),
        };
        assert_eq!(
            resp.request_seq,
            Seq(10),
            "request_seq should be remapped back to client's original seq"
        );
        assert!(resp.success);
    }

    /// Regression test: when a real DAP client is attached (non-headless), the
    /// proxy must forward an adapter-originated `startDebugging` reverse request
    /// verbatim to the client and the client's response verbatim back to the
    /// adapter — it must never intercept it. (Headless interception lives in
    /// `SessionInitializer`, not the proxy.)
    #[tokio::test]
    async fn test_reverse_request_forwarded_to_main_client_verbatim() {
        let mut tp = TestProxy::new();

        // The adapter (backend) issues a `startDebugging` reverse request.
        let reverse_request: Message = serde_json::from_value(serde_json::json!({
            "type": "request",
            "seq": 77,
            "command": "startDebugging",
            "arguments": { "request": "launch", "configuration": { "name": "child" } }
        }))
        .unwrap();
        tp.mock_backend.send(reverse_request).await.unwrap();

        // It reaches the client unchanged (same seq, same command) — not intercepted.
        let forwarded = tp.main_client.recv().await.unwrap().unwrap();
        match forwarded {
            Message::Request(r) => {
                assert_eq!(
                    r.seq,
                    Seq(77),
                    "reverse request seq must be forwarded verbatim"
                );
                assert!(
                    matches!(r.command, RequestCommand::StartDebugging(_)),
                    "expected a startDebugging reverse request"
                );
            }
            other => panic!("expected forwarded Request, got {:?}", other.message_type()),
        }

        // The client answers; the response is forwarded verbatim back to the adapter.
        let client_response: Message = serde_json::from_value(serde_json::json!({
            "type": "response",
            "seq": 1,
            "request_seq": 77,
            "success": true,
            "command": "startDebugging"
        }))
        .unwrap();
        tp.main_client.send(client_response).await.unwrap();

        let back = tp.mock_backend.recv().await.unwrap().unwrap();
        match back {
            Message::Response(r) => {
                assert_eq!(
                    r.request_seq,
                    Seq(77),
                    "response request_seq must reach the adapter unchanged"
                );
                assert!(r.success);
                assert!(matches!(r.body, ResponseBody::StartDebugging));
            }
            other => panic!(
                "expected forwarded Response, got {:?}",
                other.message_type()
            ),
        }
    }

    /// End-to-end proof that `cancel.requestId` is translated through the live
    /// `main_client_to_backend` task. The main client sends a normal request
    /// (client seq 5), the mock backend observes the remapped seq, and a
    /// follow-up cancel referencing client seq 5 must arrive at the backend
    /// with `request_id` rewritten to the same backend seq.
    #[tokio::test]
    async fn test_main_client_cancel_request_id_is_remapped() {
        let mut tp = TestProxy::new();

        // 1. Main client sends a Threads request with client seq=5.
        tp.main_client.send(make_threads_request(5)).await.unwrap();

        // 2. Mock backend observes the forwarded request with a remapped seq.
        let forwarded = tp.mock_backend.recv().await.unwrap().unwrap();
        let forwarded_req = match forwarded {
            Message::Request(r) => r,
            other => panic!("Expected Request, got {:?}", other.message_type()),
        };
        let backend_threads_seq = forwarded_req.seq;
        assert_ne!(
            backend_threads_seq,
            Seq(5),
            "original request seq should be remapped"
        );

        // 3. Main client sends a cancel referencing client seq=5.
        tp.main_client
            .send(make_cancel_request(6, Some(5)).into())
            .await
            .unwrap();

        // 4. Mock backend observes the forwarded cancel. Its outer seq is
        //    remapped (standard request flow) and its inner request_id is
        //    rewritten to the backend seq of the original threads request.
        let forwarded_cancel = tp.mock_backend.recv().await.unwrap().unwrap();
        let forwarded_cancel_req = match forwarded_cancel {
            Message::Request(r) => r,
            other => panic!("Expected Request, got {:?}", other.message_type()),
        };
        assert_ne!(
            forwarded_cancel_req.seq,
            Seq(6),
            "cancel's outer seq should be remapped like any other request"
        );
        match &forwarded_cancel_req.command {
            RequestCommand::Cancel(Some(args)) => {
                assert_eq!(
                    args.request_id,
                    Some(i64::from(backend_threads_seq)),
                    "cancel's request_id should be rewritten to the backend seq \
                     of the original request (client seq 5 → backend seq {:?})",
                    backend_threads_seq
                );
            }
            other => panic!("Expected Cancel command, got {:?}", other),
        }
    }

    /// Canonical race: the original request has already received its response
    /// (so `unmap` consumed the mapping) by the time the client's cancel
    /// arrives. The lookup misses; the cancel must be forwarded with
    /// `request_id` unchanged so the backend can apply its own "no such
    /// request" semantics. This is normal DAP behaviour, not an error.
    #[tokio::test]
    async fn test_main_client_cancel_after_response_forwards_unchanged() {
        let mut tp = TestProxy::new();

        // 1. Standard request → response round-trip. After this, the (5 → backend_seq)
        //    mapping has been consumed by `unmap` when the response was forwarded.
        tp.main_client.send(make_threads_request(5)).await.unwrap();
        let forwarded = tp.mock_backend.recv().await.unwrap().unwrap();
        let backend_threads_seq = match &forwarded {
            Message::Request(r) => r.seq,
            other => panic!("Expected Request, got {:?}", other.message_type()),
        };
        tp.mock_backend
            .send(make_threads_response(backend_threads_seq))
            .await
            .unwrap();
        // Drain the response on the main-client side so we know the
        // proxy has had a chance to call `unmap`.
        let response = tp.main_client.recv().await.unwrap().unwrap();
        assert!(matches!(response, Message::Response(_)));

        // 2. Now send a cancel referencing the same client seq=5. The mapping
        //    is gone; the cancel's `request_id` must be forwarded unchanged.
        tp.main_client
            .send(make_cancel_request(6, Some(5)).into())
            .await
            .unwrap();

        let forwarded_cancel = tp.mock_backend.recv().await.unwrap().unwrap();
        let forwarded_cancel_req = match forwarded_cancel {
            Message::Request(r) => r,
            other => panic!("Expected Request, got {:?}", other.message_type()),
        };
        match &forwarded_cancel_req.command {
            RequestCommand::Cancel(Some(args)) => {
                assert_eq!(
                    args.request_id,
                    Some(5),
                    "cancel after response must forward request_id unchanged"
                );
            }
            other => panic!("Expected Cancel command, got {:?}", other),
        }
    }

    /// Regression test for the `translate_cancel`-before-`map` ordering in
    /// `main_client_to_backend`. `client_seq` is captured from the incoming
    /// message's seq before the proxy overwrites `request.seq` with a fresh
    /// backend seq. So if `remapper.map(client_seq, BackendSeq(seq))` ran
    /// BEFORE `translate_cancel`, a malformed self-referencing cancel
    /// (`request_id == its own seq`) would find the just-inserted
    /// self-mapping and silently rewrite `request_id` to the cancel's own
    /// backend seq. Translating first leaves such requests in the
    /// forward-unchanged branch, preserving `request_id`.
    #[tokio::test]
    async fn test_main_client_self_referencing_cancel_is_not_rewritten() {
        let mut tp = TestProxy::new();

        // A cancel whose requestId equals its own client seq.
        let self_referencing_seq = 99;
        tp.main_client
            .send(make_cancel_request(self_referencing_seq, Some(self_referencing_seq)).into())
            .await
            .unwrap();

        let forwarded = tp.mock_backend.recv().await.unwrap().unwrap();
        let forwarded_req = match forwarded {
            Message::Request(r) => r,
            other => panic!("Expected Request, got {:?}", other.message_type()),
        };
        // The outer seq is remapped (standard request flow).
        assert_ne!(forwarded_req.seq, Seq(self_referencing_seq));
        // The inner request_id is preserved unchanged because no prior
        // request with that client seq was mapped — the lookup misses and
        // the forward-unchanged branch fires.
        match &forwarded_req.command {
            RequestCommand::Cancel(Some(args)) => {
                assert_eq!(
                    args.request_id,
                    Some(self_referencing_seq),
                    "self-referencing cancel must forward request_id unchanged"
                );
            }
            other => panic!("Expected Cancel command, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_secondary_client_request_not_forwarded_to_main() {
        let mut tp = TestProxy::new();

        // Secondary client sends a Threads request through the ProxyClient
        let req = Request::new(RequestCommand::Threads);
        let listener_payload = tp.proxy_client.send_message(req.into()).await.unwrap();

        // Mock backend receives the forwarded request
        let forwarded = tp.mock_backend.recv().await.unwrap().unwrap();
        let forwarded_seq = forwarded.seq();

        // Mock backend sends a response
        tp.mock_backend
            .send(make_threads_response(forwarded_seq))
            .await
            .unwrap();

        // Secondary client receives the response through the listener
        let mut messages = listener_payload.messages;
        let response =
            crate::client::helpers::wait_for_response(listener_payload.seq, &mut messages)
                .await
                .unwrap();
        assert!(response.success);

        // Main client should NOT receive this response (it wasn't from the main client)
        let timeout_result =
            tokio::time::timeout(std::time::Duration::from_millis(50), tp.main_client.recv()).await;
        assert!(
            timeout_result.is_err(),
            "Main client should not receive the secondary client's response"
        );
    }

    #[tokio::test]
    async fn test_event_forwarded_to_main_client() {
        let mut tp = TestProxy::new();

        // Mock backend sends a Stopped event
        tp.mock_backend.send(make_stopped_event()).await.unwrap();

        // Main client receives the event
        let msg = tp.main_client.recv().await.unwrap().unwrap();
        match msg {
            Message::Event(event) => {
                assert!(
                    matches!(event.event, EventKind::Stopped(_)),
                    "Expected Stopped event"
                );
            }
            other => panic!("Expected Event, got {:?}", other.message_type()),
        }
    }

    #[tokio::test]
    async fn test_event_broadcast_to_listener() {
        let mut tp = TestProxy::new();

        // Secondary client sends a request to subscribe to the broadcast
        let req = Request::new(RequestCommand::Threads);
        let listener_payload = tp.proxy_client.send_message(req.into()).await.unwrap();
        let mut messages = listener_payload.messages;

        // Drain the forwarded request from the mock backend
        let _ = tp.mock_backend.recv().await.unwrap().unwrap();

        // Mock backend sends a Stopped event
        tp.mock_backend.send(make_stopped_event()).await.unwrap();

        // The broadcast listener receives the event
        let broadcast_msg =
            tokio::time::timeout(std::time::Duration::from_millis(100), messages.recv())
                .await
                .unwrap()
                .unwrap();

        assert!(
            matches!(broadcast_msg.as_ref(), Message::Event(e) if matches!(e.event, EventKind::Stopped(_))),
            "Listener should receive the Stopped event"
        );
    }

    #[tokio::test]
    async fn test_event_channel_messages_reach_main_client() {
        let mut tp = TestProxy::new();

        // Inject an event through the EventChannel
        let event = EventKind::Stopped(StoppedEventBody {
            reason: StoppedReason::Step,
            thread_id: Some(ThreadId(1)),
            all_threads_stopped: Some(true),
            ..Default::default()
        });
        tp.proxy_client.event_channel().send_event(event).unwrap();

        // Main client receives the injected event
        let msg = tp.main_client.recv().await.unwrap().unwrap();
        match msg {
            Message::Event(event) => {
                assert!(
                    matches!(event.event, EventKind::Stopped(_)),
                    "Expected Stopped event from EventChannel"
                );
            }
            other => panic!("Expected Event, got {:?}", other.message_type()),
        }
    }

    #[tokio::test]
    async fn test_client_disconnect_shuts_down_proxy() {
        let tp = TestProxy::new();

        // Drop the main client — the proxy should detect the disconnection
        drop(tp.main_client);

        // The proxy task should complete cleanly (no panic, no error)
        let join_result = tokio::time::timeout(std::time::Duration::from_secs(2), tp.handle)
            .await
            .expect("Proxy should shut down after client disconnect");
        join_result
            .expect("Proxy task should not panic")
            .expect("Proxy should shut down cleanly");
    }

    #[tokio::test]
    async fn test_backend_disconnect_shuts_down_proxy() {
        let tp = TestProxy::new();

        // Drop the mock backend — the proxy should detect the disconnection
        drop(tp.mock_backend);

        // The proxy task should complete cleanly (no panic, no error)
        let join_result = tokio::time::timeout(std::time::Duration::from_secs(2), tp.handle)
            .await
            .expect("Proxy should shut down after backend disconnect");
        join_result
            .expect("Proxy task should not panic")
            .expect("Proxy should shut down cleanly");
    }

    #[test]
    fn test_remapper_map_and_unmap() {
        let remapper = MessageRemapper::new();
        let client_seq = ClientSeq(Seq(5));
        let backend_seq = BackendSeq(Seq(100));

        let returned = remapper.map(client_seq, backend_seq);
        assert_eq!(returned, backend_seq);

        let unmapped = remapper.unmap(backend_seq);
        assert_eq!(unmapped, Some(client_seq));
    }

    #[test]
    fn test_remapper_unmap_unknown_seq() {
        let remapper = MessageRemapper::new();
        let unknown_seq = BackendSeq(Seq(999));

        let result = remapper.unmap(unknown_seq);
        assert_eq!(result, None);
    }

    #[test]
    fn test_remapper_multiple_in_flight() {
        let remapper = MessageRemapper::new();

        let client1 = ClientSeq(Seq(1));
        let backend1 = BackendSeq(Seq(100));
        let client2 = ClientSeq(Seq(2));
        let backend2 = BackendSeq(Seq(101));
        let client3 = ClientSeq(Seq(3));
        let backend3 = BackendSeq(Seq(102));

        remapper.map(client1, backend1);
        remapper.map(client2, backend2);
        remapper.map(client3, backend3);

        // Unmap out of order
        assert_eq!(remapper.unmap(backend2), Some(client2));
        assert_eq!(remapper.unmap(backend1), Some(client1));
        assert_eq!(remapper.unmap(backend3), Some(client3));
    }

    #[test]
    fn test_remapper_unmap_removes_mapping() {
        let remapper = MessageRemapper::new();
        let client_seq = ClientSeq(Seq(5));
        let backend_seq = BackendSeq(Seq(100));

        remapper.map(client_seq, backend_seq);

        // First unmap succeeds
        assert_eq!(remapper.unmap(backend_seq), Some(client_seq));

        // Second unmap returns None (already consumed)
        assert_eq!(remapper.unmap(backend_seq), None);
    }

    #[test]
    fn test_remapper_map_overwrites() {
        let remapper = MessageRemapper::new();
        let client_seq = ClientSeq(Seq(5));
        let backend_seq1 = BackendSeq(Seq(100));
        let backend_seq2 = BackendSeq(Seq(200));

        remapper.map(client_seq, backend_seq1);
        remapper.map(client_seq, backend_seq2);

        // The latest mapping wins
        assert_eq!(remapper.unmap(backend_seq2), Some(client_seq));

        // The old backend_seq1 mapping was cleaned up by the overwrite
        assert_eq!(remapper.unmap(backend_seq1), None);
    }

    #[test]
    fn test_remapper_lookup_backend_returns_mapping() {
        let remapper = MessageRemapper::new();
        let client_seq = ClientSeq(Seq(5));
        let backend_seq = BackendSeq(Seq(100));

        remapper.map(client_seq, backend_seq);

        // lookup is read-only — returns the same value across repeated calls.
        assert_eq!(remapper.lookup_backend(client_seq), Some(backend_seq));
        assert_eq!(remapper.lookup_backend(client_seq), Some(backend_seq));

        // The mapping is still consumable by `unmap`.
        assert_eq!(remapper.unmap(backend_seq), Some(client_seq));
    }

    #[test]
    fn test_remapper_lookup_backend_unknown() {
        let remapper = MessageRemapper::new();
        assert_eq!(remapper.lookup_backend(ClientSeq(Seq(5))), None);
    }

    #[test]
    fn test_remapper_lookup_backend_after_unmap() {
        let remapper = MessageRemapper::new();
        let client_seq = ClientSeq(Seq(5));
        let backend_seq = BackendSeq(Seq(100));

        remapper.map(client_seq, backend_seq);
        assert_eq!(remapper.unmap(backend_seq), Some(client_seq));

        // Once `unmap` consumes the mapping, `lookup_backend` no longer finds it.
        assert_eq!(remapper.lookup_backend(client_seq), None);
    }

    #[test]
    fn test_translate_cancel_known_request_id() {
        let remapper = MessageRemapper::new();
        let client_seq = ClientSeq(Seq(5));
        let backend_seq = BackendSeq(Seq(100));
        remapper.map(client_seq, backend_seq);

        let mut req = make_cancel_request(7, Some(5));
        translate_cancel(&mut req, &remapper);

        match &req.command {
            RequestCommand::Cancel(Some(args)) => {
                assert_eq!(args.request_id, Some(100));
            }
            other => panic!("Expected Cancel command, got {:?}", other),
        }

        // Translation must be read-only: the original mapping is still alive
        // so the in-flight request's response can still be unmapped later.
        assert_eq!(remapper.lookup_backend(client_seq), Some(backend_seq));
    }

    #[test]
    fn test_translate_cancel_unknown_request_id_preserved() {
        let remapper = MessageRemapper::new();

        let mut req = make_cancel_request(7, Some(42));
        translate_cancel(&mut req, &remapper);

        // The unknown requestId is forwarded unchanged so the backend can
        // apply its own "no such request" semantics.
        match &req.command {
            RequestCommand::Cancel(Some(args)) => {
                assert_eq!(args.request_id, Some(42));
            }
            other => panic!("Expected Cancel command, got {:?}", other),
        }
    }

    #[test]
    fn test_translate_cancel_progress_id_only() {
        let remapper = MessageRemapper::new();

        let mut req = Request::new(RequestCommand::Cancel(Some(
            dapper_dap_protocol::requests::CancelArguments {
                request_id: None,
                progress_id: Some("p1".to_string()),
                ..Default::default()
            },
        )));
        req.seq = Seq(7);

        translate_cancel(&mut req, &remapper);

        match &req.command {
            RequestCommand::Cancel(Some(args)) => {
                assert_eq!(args.request_id, None);
                assert_eq!(args.progress_id.as_deref(), Some("p1"));
            }
            other => panic!("Expected Cancel command, got {:?}", other),
        }
    }

    #[test]
    fn test_translate_cancel_no_arguments() {
        let remapper = MessageRemapper::new();

        let mut req = Request::new(RequestCommand::Cancel(None));
        req.seq = Seq(7);

        translate_cancel(&mut req, &remapper);

        // No panic, no mutation: still Cancel(None).
        match &req.command {
            RequestCommand::Cancel(None) => {}
            other => panic!("Expected Cancel(None), got {:?}", other),
        }
    }

    #[test]
    fn test_translate_non_cancel_request() {
        let remapper = MessageRemapper::new();
        // Pre-populate so we'd notice if translate_cancel mistakenly mutated
        // a non-cancel request based on `seq`.
        remapper.map(ClientSeq(Seq(7)), BackendSeq(Seq(100)));

        let mut req = Request::new(RequestCommand::Threads);
        req.seq = Seq(7);

        translate_cancel(&mut req, &remapper);

        match &req.command {
            RequestCommand::Threads => {}
            other => panic!("Expected Threads command, got {:?}", other),
        }
        assert_eq!(req.seq, Seq(7));
    }

    // -- Reverse-debugging tests ---------------------------------------------

    fn make_initialize_request(seq: i64) -> Message {
        let mut req = Request::new(RequestCommand::Initialize(
            InitializeRequestArguments::default(),
        ));
        req.seq = Seq(seq);
        req.into()
    }

    fn make_initialize_response_with_caps(request_seq: Seq, caps: Capabilities) -> Message {
        Response {
            seq: Seq(0), // Overwritten by the proxy
            request_seq,
            success: true,
            message: None,
            body: ResponseBody::Initialize(Some(caps)),
        }
        .into()
    }

    fn make_step_back_response(request_seq: Seq) -> Message {
        Response {
            seq: Seq(0),
            request_seq,
            success: true,
            message: None,
            body: ResponseBody::StepBack,
        }
        .into()
    }

    fn make_reverse_continue_response(request_seq: Seq) -> Message {
        Response {
            seq: Seq(0),
            request_seq,
            success: true,
            message: None,
            body: ResponseBody::ReverseContinue,
        }
        .into()
    }

    /// Drive a complete `initialize` request/response round-trip through the
    /// proxy so its tracker absorbs `caps`. The proxy only stores capabilities
    /// when an `Initialize` response flows backend→main_client through the
    /// usual seq-remap path, so we drive a real `initialize` request from
    /// `main_client`, capture the proxy's mapped seq off `mock_backend`, and
    /// reply with that seq. A response synthesised on `mock_backend` without
    /// the matching forwarded request would carry the wrong `request_seq` and
    /// the tracker would silently drop it.
    async fn drive_initialize_with_caps(tp: &mut TestProxy, caps: Capabilities) {
        tp.main_client
            .send(make_initialize_request(1))
            .await
            .unwrap();
        let forwarded = tp.mock_backend.recv().await.unwrap().unwrap();
        let mapped_seq = match forwarded {
            Message::Request(r) => r.seq,
            other => panic!(
                "expected initialize Request, got {:?}",
                other.message_type()
            ),
        };
        tp.mock_backend
            .send(make_initialize_response_with_caps(mapped_seq, caps))
            .await
            .unwrap();
        // Drain the response so the channel is empty for the test that follows.
        let _ = tp.main_client.recv().await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn navigate_step_back_success_when_capability_advertised() {
        let mut tp = TestProxy::new();
        drive_initialize_with_caps(
            &mut tp,
            Capabilities {
                supports_step_back: Some(true),
                ..Default::default()
            },
        )
        .await;

        // navigate(...) blocks on the backend response, so spawn it and drive
        // the backend side concurrently from the test task.
        let proxy_client = tp.proxy_client.clone();
        let nav = tokio::spawn(async move {
            proxy_client
                .navigate(NavigationType::StepBack, ThreadId(1), None)
                .await
        });

        let forwarded = tp.mock_backend.recv().await.unwrap().unwrap();
        let req = match forwarded {
            Message::Request(r) => r,
            other => panic!("expected stepBack Request, got {:?}", other.message_type()),
        };
        match &req.command {
            RequestCommand::StepBack(args) => assert_eq!(args.thread_id, ThreadId(1)),
            other => panic!("expected StepBack command, got {:?}", other),
        }
        tp.mock_backend
            .send(make_step_back_response(req.seq))
            .await
            .unwrap();

        // StepBack does NOT enter the wait-for-stopped branch in
        // ProxyClient::navigate, so the test does not need to send a stopped
        // event and the navigate future completes as soon as the response
        // round-trips.
        let result = nav.await.unwrap().expect("navigate should succeed");
        assert_eq!(result.result, NavigateResult::CommandExecuted);
        assert_eq!(result.navigation_type, NavigationType::StepBack);
    }

    #[tokio::test]
    async fn navigate_step_back_gated_when_capability_missing() {
        let mut tp = TestProxy::new();
        drive_initialize_with_caps(
            &mut tp,
            Capabilities {
                supports_step_back: Some(false),
                ..Default::default()
            },
        )
        .await;

        // The gate returns before sending anything, so `.await`ing directly is
        // safe here — no concurrent backend driving needed.
        let err = tp
            .proxy_client
            .navigate(NavigationType::StepBack, ThreadId(1), None)
            .await
            .expect_err("step_back should be gated");
        let msg = format!("{}", err);
        assert!(
            msg.contains("does not advertise the DAP `supportsStepBack` capability"),
            "unexpected error: {}",
            msg
        );

        // Confirm no request reached the backend.
        let timeout = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            tp.mock_backend.recv(),
        )
        .await;
        assert!(
            timeout.is_err(),
            "backend should not have received a request"
        );
    }

    #[tokio::test]
    async fn navigate_reverse_continue_waits_for_stopped_event() {
        let mut tp = TestProxy::new();
        drive_initialize_with_caps(
            &mut tp,
            Capabilities {
                supports_step_back: Some(true),
                ..Default::default()
            },
        )
        .await;

        let proxy_client = tp.proxy_client.clone();
        let nav = tokio::spawn(async move {
            proxy_client
                .navigate(NavigationType::ReverseContinue, ThreadId(1), None)
                .await
        });

        let forwarded = tp.mock_backend.recv().await.unwrap().unwrap();
        let req = match forwarded {
            Message::Request(r) => r,
            other => panic!(
                "expected reverseContinue Request, got {:?}",
                other.message_type()
            ),
        };
        match &req.command {
            RequestCommand::ReverseContinue(_) => {}
            other => panic!("expected ReverseContinue command, got {:?}", other),
        }
        tp.mock_backend
            .send(make_reverse_continue_response(req.seq))
            .await
            .unwrap();

        // Send the stopped event without an artificial sleep — the proxy's
        // event listener buffers messages, so the navigate future will pick
        // up the event whether it's already parked on the wait branch or
        // arrives there a tick later. The Stopped assertion below is what
        // proves the wait branch fired (StepBack short-circuits before it).
        tp.mock_backend.send(make_stopped_event()).await.unwrap();

        let result = nav.await.unwrap().expect("navigate should succeed");
        match result.result {
            NavigateResult::Stopped(_) => {}
            other => panic!("expected Stopped result, got {:?}", other),
        }
        assert_eq!(result.navigation_type, NavigationType::ReverseContinue);
    }

    #[tokio::test]
    async fn navigate_reverse_continue_gated_when_capabilities_unknown() {
        let tp = TestProxy::new();
        // Skip the initialize round-trip entirely — capabilities should be
        // None and the gate must reject with the "capabilities unknown" path.
        let err = tp
            .proxy_client
            .navigate(NavigationType::ReverseContinue, ThreadId(1), None)
            .await
            .expect_err("reverse_continue should be gated");
        let msg = format!("{}", err);
        assert!(
            msg.contains("capabilities unknown (initialize not yet received)"),
            "unexpected error: {}",
            msg
        );
        // Cross-check that the gate fired on the unknown-capabilities branch
        // rather than the explicit-not-supported branch — a regression that
        // collapses the two messages would otherwise pass the previous assert.
        assert!(
            !msg.contains("does not advertise"),
            "expected unknown-capabilities branch, got explicit-unsupported message: {}",
            msg
        );
    }
}
