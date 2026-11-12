//! Newline delimited JSON-RPC framing and the serve loop shared by the stdio and socket
//! transports.
//!
//! Adapted from the stdio gateway of the RDX MCP server (Apache-2.0, Copyright 2026 Ch0pin and
//! RDX contributors): the frame size limit, the `RESPONSE_TOO_LARGE` fallback that keeps the
//! connection usable, and the request validation and lifecycle rules. Modified: the transport
//! is generic over any reader and writer, requests are read on their own thread so that
//! `notifications/cancelled` can be seen while a tool runs, and there is no GUI worker or
//! instance discovery.

use std::io::{BufRead, Read, Write};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

use serde_json::{json, Value};

use crate::server::{Server, SharedState};

/// Largest accepted request and largest emitted response, in bytes.
pub const MAX_FRAME: u64 = 8 * 1024 * 1024;

/// Read one newline terminated frame. `Ok(None)` on end of input.
pub fn read_frame(reader: &mut impl BufRead) -> Result<Option<Value>, String> {
    let mut line = String::new();
    let n = reader
        .take(MAX_FRAME + 1)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(None);
    }
    if n as u64 > MAX_FRAME || !line.ends_with('\n') {
        return Err(format!(
            "Invalid or oversized frame: received {n} bytes (limit {MAX_FRAME}), newline terminated: {}",
            line.ends_with('\n')
        ));
    }
    serde_json::from_str(&line)
        .map(Some)
        .map_err(|e| e.to_string())
}

pub fn write_frame(writer: &mut impl Write, value: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// Write a response; a response above the frame limit is replaced by a tool error so the
/// connection stays usable.
pub fn write_response(writer: &mut impl Write, value: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() as u64 + 1 > MAX_FRAME {
        let message = "RESPONSE_TOO_LARGE: request a smaller page or a narrower filter";
        let data = json!({"error": {"code": "RESPONSE_TOO_LARGE", "message": message}});
        let replacement = json!({
            "jsonrpc": "2.0",
            "id": value["id"],
            "result": {
                "content": [{"type": "text", "text": data.to_string()}],
                "structuredContent": data,
                "isError": true,
            },
        });
        return write_frame(writer, &replacement);
    }
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

enum Incoming {
    Message(Value),
    Broken(String),
}

/// Serve one connection until its input ends. Requests are processed one at a time in arrival
/// order; `notifications/cancelled` is handled as soon as it arrives and makes the running
/// tool call (or a queued one) with that request id stop with `CANCELLED`.
pub fn serve<R, W>(reader: R, mut writer: W, shared: Arc<SharedState>) -> std::io::Result<()>
where
    R: BufRead + Send + 'static,
    W: Write,
{
    let mut server = Server::new(shared);
    let cancels = server.cancel_registry();
    let (tx, rx) = mpsc::channel::<Incoming>();
    thread::Builder::new()
        .name("jadx-mcp-reader".into())
        .spawn(move || {
            let mut reader = reader;
            loop {
                match read_frame(&mut reader) {
                    Ok(Some(message)) => {
                        if message["method"] == "notifications/cancelled" {
                            if let Some(id) = message["params"].get("requestId") {
                                cancels.cancel(id);
                            }
                            continue;
                        }
                        if tx.send(Incoming::Message(message)).is_err() {
                            return;
                        }
                    }
                    Ok(None) => return,
                    Err(e) => {
                        let _ = tx.send(Incoming::Broken(e));
                        return;
                    }
                }
            }
        })?;
    while let Ok(incoming) = rx.recv() {
        match incoming {
            Incoming::Message(message) => {
                if let Some(response) = server.handle(&message) {
                    write_response(&mut writer, &response)?;
                }
            }
            Incoming::Broken(error) => {
                write_frame(
                    &mut writer,
                    &json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":error}}),
                )?;
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frames_round_trip_and_reject_garbage() {
        let mut input = Cursor::new(b"{\"a\":1}\n".to_vec());
        assert_eq!(read_frame(&mut input).unwrap(), Some(json!({"a": 1})));
        assert_eq!(read_frame(&mut input).unwrap(), None);
        assert!(read_frame(&mut Cursor::new(b"not json\n".to_vec())).is_err());
        assert!(read_frame(&mut Cursor::new(b"{\"a\":1}".to_vec())).is_err());
    }

    #[test]
    fn oversized_responses_become_tool_errors() {
        let mut out = Vec::new();
        let big = json!({"jsonrpc":"2.0","id":7,"result":{"text":"x".repeat(MAX_FRAME as usize)}});
        write_response(&mut out, &big).unwrap();
        let reply: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(reply["id"], json!(7));
        assert_eq!(reply["result"]["isError"], json!(true));
        assert_eq!(
            reply["result"]["structuredContent"]["error"]["code"],
            json!("RESPONSE_TOO_LARGE")
        );
    }
}
