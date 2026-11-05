use qwe_protocol::{
    PROTOCOL_VERSION, ProtocolErrorBody, RequestEnvelope, ResponseEnvelope, read_frame,
    validate_version, verify_same_uid, write_frame,
};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug)]
pub enum WorkerEvent {
    ControlClosed { session_id: String },
}

type PendingReply = Result<ResponseEnvelope, String>;

pub struct WorkerClient {
    session_id: String,
    writer: Mutex<std::os::unix::net::UnixStream>,
    pending: Arc<Mutex<HashMap<String, Sender<PendingReply>>>>,
    events: Sender<WorkerEvent>,
    next_request: AtomicU64,
    closed: Arc<AtomicBool>,
}

impl WorkerClient {
    pub fn connect(
        socket: &Path,
        session_id: &str,
        events: Sender<WorkerEvent>,
    ) -> Result<Arc<Self>, std::io::Error> {
        let stream = std::os::unix::net::UnixStream::connect(socket)?;
        verify_same_uid(&stream).map_err(|failure| std::io::Error::other(failure.to_string()))?;
        let mut reader = stream.try_clone()?;
        let client = Arc::new(Self {
            session_id: session_id.to_string(),
            writer: Mutex::new(stream),
            pending: Arc::new(Mutex::new(HashMap::new())),
            events,
            next_request: AtomicU64::new(1),
            closed: Arc::new(AtomicBool::new(false)),
        });
        let pending = Arc::clone(&client.pending);
        let events = client.events.clone();
        let closed = Arc::clone(&client.closed);
        let session_id = client.session_id.clone();
        thread::spawn(move || {
            loop {
                let response = match read_frame::<_, ResponseEnvelope>(&mut reader) {
                    Ok(response) => response,
                    Err(failure) => {
                        closed.store(true, Ordering::SeqCst);
                        let message = failure.to_string();
                        for (_, sender) in pending
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .drain()
                        {
                            let _ = sender.send(Err(message.clone()));
                        }
                        let _ = events.send(WorkerEvent::ControlClosed {
                            session_id: session_id.clone(),
                        });
                        return;
                    }
                };
                if let Some(sender) = pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&response.request_id)
                {
                    let _ = sender.send(Ok(response));
                }
            }
        });
        Ok(client)
    }

    pub fn call(
        &self,
        operation: &str,
        payload: Value,
        timeout: Duration,
    ) -> Result<Value, ProtocolErrorBody> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(self.unreachable("worker control channel is closed"));
        }
        let request_number = self.next_request.fetch_add(1, Ordering::SeqCst);
        let request_id = format!("{}-{request_number}", self.session_id);
        let (sender, receiver) = mpsc::channel();
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(request_id.clone(), sender);
        let request = RequestEnvelope {
            version: PROTOCOL_VERSION,
            request_id: request_id.clone(),
            idempotency_key: None,
            operation: operation.into(),
            payload,
        };
        let write_result = {
            let mut writer = self
                .writer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            writer
                .set_write_timeout(Some(timeout))
                .and_then(|()| write_frame(&mut *writer, &request).map_err(std::io::Error::other))
        };
        if let Err(failure) = write_result {
            self.pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&request_id);
            self.mark_closed();
            return Err(self.unreachable(failure.to_string()));
        }
        let response = match receive_with_timeout(&receiver, timeout) {
            Ok(Ok(response)) => response,
            Ok(Err(message)) => {
                self.mark_closed();
                return Err(self.unreachable(message));
            }
            Err(()) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&request_id);
                return Err(self.unreachable("worker control request timed out"));
            }
        };
        validate_version(response.version)
            .map_err(|failure| self.unreachable(failure.to_string()))?;
        response.result
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn mark_closed(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            let _ = self.events.send(WorkerEvent::ControlClosed {
                session_id: self.session_id.clone(),
            });
        }
    }

    fn unreachable(&self, message: impl Into<String>) -> ProtocolErrorBody {
        ProtocolErrorBody {
            code: "worker_unreachable".into(),
            message: message.into(),
        }
    }
}

fn receive_with_timeout<T>(receiver: &Receiver<T>, timeout: Duration) -> Result<T, ()> {
    receiver.recv_timeout(timeout).map_err(|_| ())
}
