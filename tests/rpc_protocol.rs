use std::io::{Cursor, Read};

use mado::nvim::rpc::{RpcMessage, decode_rpc_message, read_rpc_message};
use rmpv::Value;

#[test]
fn decodes_request_response_and_notification() {
    assert!(matches!(
        decode_rpc_message(Value::Array(vec![
            Value::from(0),
            Value::from(7),
            Value::from("method"),
            Value::Array(vec![]),
        ]))
        .unwrap(),
        RpcMessage::Request { id: 7, .. }
    ));

    assert!(matches!(
        decode_rpc_message(Value::Array(vec![
            Value::from(1),
            Value::from(7),
            Value::Nil,
            Value::from(true),
        ]))
        .unwrap(),
        RpcMessage::Response { id: 7, .. }
    ));

    assert!(matches!(
        decode_rpc_message(Value::Array(vec![
            Value::from(2),
            Value::from("redraw"),
            Value::Array(vec![]),
        ]))
        .unwrap(),
        RpcMessage::Notification { .. }
    ));
}

#[test]
fn reads_concatenated_messages_from_small_chunks() {
    let messages = [
        Value::Array(vec![
            Value::from(2),
            Value::from("one"),
            Value::Array(vec![]),
        ]),
        Value::Array(vec![
            Value::from(2),
            Value::from("two"),
            Value::Array(vec![]),
        ]),
    ];
    let mut bytes = Vec::new();
    for message in &messages {
        rmpv::encode::write_value(&mut bytes, message).unwrap();
    }
    let mut reader = ChunkedReader {
        inner: Cursor::new(bytes),
        chunk_size: 2,
    };

    let first = read_rpc_message(&mut reader).unwrap();
    let second = read_rpc_message(&mut reader).unwrap();
    assert!(matches!(first, RpcMessage::Notification { method, .. } if method == "one"));
    assert!(matches!(second, RpcMessage::Notification { method, .. } if method == "two"));
}

struct ChunkedReader {
    inner: Cursor<Vec<u8>>,
    chunk_size: usize,
}

impl Read for ChunkedReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let limit = buffer.len().min(self.chunk_size);
        self.inner.read(&mut buffer[..limit])
    }
}
