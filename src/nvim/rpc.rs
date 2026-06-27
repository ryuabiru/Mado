use std::collections::HashMap;
use std::io::{BufReader, BufWriter, Read, Write};
use std::process::{ChildStdin, ChildStdout};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use rmpv::Value;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const EVENT_QUEUE_CAPACITY: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub enum RpcMessage {
    Request {
        id: u64,
        method: String,
        params: Vec<Value>,
    },
    Response {
        id: u64,
        error: Value,
        result: Value,
    },
    Notification {
        method: String,
        params: Vec<Value>,
    },
}

#[derive(Debug)]
pub enum RpcEvent {
    Notification { method: String, params: Vec<Value> },
    ProtocolError(String),
    Eof,
}

#[derive(Clone)]
pub struct RpcClient {
    writer: Arc<Mutex<BufWriter<ChildStdin>>>,
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<RpcResponse>>>>,
    next_id: Arc<AtomicU64>,
}

#[derive(Debug)]
struct RpcResponse {
    error: Value,
    result: Value,
}

impl RpcClient {
    pub fn start(
        stdin: ChildStdin,
        stdout: ChildStdout,
    ) -> Result<(Self, mpsc::Receiver<RpcEvent>)> {
        let writer = Arc::new(Mutex::new(BufWriter::new(stdin)));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        // Backpressure keeps a stalled window from accumulating redraw batches forever.
        let (event_tx, event_rx) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);

        spawn_reader(stdout, Arc::clone(&writer), Arc::clone(&pending), event_tx)?;

        Ok((
            Self {
                writer,
                pending,
                next_id: Arc::new(AtomicU64::new(1)),
            },
            event_rx,
        ))
    }

    pub fn request(&self, method: &str, params: Vec<Value>) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (response_tx, response_rx) = mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| anyhow!("RPC pending-request lock was poisoned"))?
            .insert(id, response_tx);

        let message = Value::Array(vec![
            Value::from(0),
            Value::from(id),
            Value::from(method),
            Value::Array(params),
        ]);

        if let Err(error) = write_value(&self.writer, &message) {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            return Err(error).context("failed to write RPC request");
        }

        let response = match response_rx.recv_timeout(REQUEST_TIMEOUT) {
            Ok(response) => response,
            Err(error) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&id);
                }
                return Err(error).context("timed out waiting for Neovim RPC response");
            }
        };

        if response.error.is_nil() {
            Ok(response.result)
        } else {
            bail!("Neovim RPC error: {:?}", response.error)
        }
    }

    pub fn notify(&self, method: &str, params: Vec<Value>) -> Result<()> {
        let message = Value::Array(vec![
            Value::from(2),
            Value::from(method),
            Value::Array(params),
        ]);
        write_value(&self.writer, &message)
    }
}

pub fn read_rpc_message<R: Read>(reader: &mut R) -> Result<RpcMessage> {
    let value = rmpv::decode::read_value(reader).context("invalid MessagePack value")?;
    decode_rpc_message(value)
}

pub fn decode_rpc_message(value: Value) -> Result<RpcMessage> {
    let values = value.as_array().context("RPC message must be an array")?;
    let kind = value_u64(values.first(), "RPC message type")?;

    match kind {
        0 => Ok(RpcMessage::Request {
            id: value_u64(values.get(1), "request id")?,
            method: value_string(values.get(2), "request method")?,
            params: value_array(values.get(3), "request params")?,
        }),
        1 => Ok(RpcMessage::Response {
            id: value_u64(values.get(1), "response id")?,
            error: values.get(2).context("missing response error")?.clone(),
            result: values.get(3).context("missing response result")?.clone(),
        }),
        2 => Ok(RpcMessage::Notification {
            method: value_string(values.get(1), "notification method")?,
            params: value_array(values.get(2), "notification params")?,
        }),
        other => bail!("unknown RPC message type {other}"),
    }
}

fn spawn_reader(
    stdout: ChildStdout,
    writer: Arc<Mutex<BufWriter<ChildStdin>>>,
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<RpcResponse>>>>,
    event_tx: mpsc::SyncSender<RpcEvent>,
) -> Result<()> {
    thread::Builder::new()
        .name("mado-rpc-reader".into())
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_rpc_message(&mut reader) {
                    Ok(RpcMessage::Response { id, error, result }) => {
                        let sender = pending.lock().ok().and_then(|mut map| map.remove(&id));
                        if let Some(sender) = sender {
                            let _ = sender.send(RpcResponse { error, result });
                        } else {
                            let _ = event_tx.send(RpcEvent::ProtocolError(format!(
                                "response for unknown request id {id}"
                            )));
                        }
                    }
                    Ok(RpcMessage::Notification { method, params }) => {
                        if event_tx
                            .send(RpcEvent::Notification { method, params })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(RpcMessage::Request { id, method, .. }) => {
                        let error = Value::from(format!(
                            "Mado does not implement Neovim RPC request '{method}'"
                        ));
                        let response =
                            Value::Array(vec![Value::from(1), Value::from(id), error, Value::Nil]);
                        if let Err(error) = write_value(&writer, &response) {
                            let _ = event_tx.send(RpcEvent::ProtocolError(error.to_string()));
                            break;
                        }
                    }
                    Err(error) if is_eof(&error) => {
                        let _ = event_tx.send(RpcEvent::Eof);
                        break;
                    }
                    Err(error) => {
                        let _ = event_tx.send(RpcEvent::ProtocolError(error.to_string()));
                        break;
                    }
                }
            }
        })
        .context("failed to spawn RPC reader thread")?;
    Ok(())
}

fn write_value(writer: &Arc<Mutex<BufWriter<ChildStdin>>>, value: &Value) -> Result<()> {
    let mut writer = writer
        .lock()
        .map_err(|_| anyhow!("RPC writer lock was poisoned"))?;
    rmpv::encode::write_value(&mut *writer, value).context("failed to encode MessagePack value")?;
    writer.flush().context("failed to flush Neovim stdin")
}

fn value_u64(value: Option<&Value>, label: &str) -> Result<u64> {
    value
        .and_then(Value::as_u64)
        .with_context(|| format!("{label} must be an unsigned integer"))
}

fn value_string(value: Option<&Value>, label: &str) -> Result<String> {
    value
        .and_then(Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("{label} must be a UTF-8 string"))
}

fn value_array(value: Option<&Value>, label: &str) -> Result<Vec<Value>> {
    value
        .and_then(Value::as_array)
        .cloned()
        .with_context(|| format!("{label} must be an array"))
}

fn is_eof(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| io_error.kind() == std::io::ErrorKind::UnexpectedEof)
    })
}
