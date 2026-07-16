// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_dap_protocol::protocol::Message;
use dapper_dap_protocol::protocol::ProtocolError;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::io::BufWriter;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;

/// Buffer size for DAP message I/O operations.
/// 64KB is chosen to accommodate typical DAP message sizes while reducing
/// syscall overhead. Most DAP messages are <10KB, so this allows batching
/// multiple messages per syscall when using write_buffered + flush.
const DAP_BUFFER_SIZE: usize = 64 * 1024;

// Channels are concrete over the DAP `Message` type — the only wire format
// in this workspace.
pub struct WriteChannel {
    writer: BufWriter<Box<dyn AsyncWrite + Send + Unpin>>,
}

impl WriteChannel {
    pub fn new(writer: Box<dyn AsyncWrite + Send + Unpin>) -> Self {
        let writer = BufWriter::with_capacity(DAP_BUFFER_SIZE, writer);
        Self { writer }
    }

    /// Write a message into the internal buffer and flush it to the
    /// underlying writer immediately.
    ///
    /// On Windows `tokio::io::stdin()` / `stdout()` dispatch every write
    /// to the blocking-thread pool, so each `flush()` costs a pool
    /// round-trip (~100-500 µs of scheduling overhead). Prefer
    /// [`write_buffered`] + an explicit [`flush`] when forwarding
    /// multiple independent messages in a loop.
    pub async fn send(&mut self, message: Message) -> anyhow::Result<()> {
        let content = message.format()?;
        self.writer.write_all(&content).await?;
        Ok(self.writer.flush().await?)
    }

    /// Write a message into the internal buffer *without* flushing.
    ///
    /// The caller **must** call [`flush`] at some later point to ensure
    /// the data actually reaches the peer. This is useful when the
    /// caller knows it will send several messages in quick succession
    /// and wants to batch them into a single `flush()` / syscall.
    pub async fn write_buffered(&mut self, message: Message) -> anyhow::Result<()> {
        let content = message.format()?;
        self.writer.write_all(&content).await?;
        Ok(())
    }

    /// Flush the internal buffer to the underlying writer.
    pub async fn flush(&mut self) -> anyhow::Result<()> {
        Ok(self.writer.flush().await?)
    }
}

pub struct ReadChannel {
    reader: BufReader<Box<dyn AsyncRead + Send + Unpin>>,
}

impl ReadChannel {
    pub fn new(reader: Box<dyn AsyncRead + Send + Unpin>) -> Self {
        let reader = BufReader::with_capacity(DAP_BUFFER_SIZE, reader);
        Self { reader }
    }

    pub async fn recv(&mut self) -> Result<Option<Message>, ProtocolError> {
        Message::read(&mut self.reader).await
    }
}

pub struct DuplexChannel {
    read: ReadChannel,
    write: WriteChannel,
}

impl DuplexChannel {
    pub fn from_streams(
        writer: Box<dyn AsyncWrite + Send + Unpin>,
        reader: Box<dyn AsyncRead + Send + Unpin>,
    ) -> Self {
        let read = ReadChannel::new(reader);
        let write = WriteChannel::new(writer);

        Self { write, read }
    }

    pub fn from_stdio() -> Self {
        let read = ReadChannel::new(Box::new(tokio::io::stdin()));
        let write = WriteChannel::new(Box::new(tokio::io::stdout()));

        Self { write, read }
    }

    pub async fn from_tcp_client(host: &str, port: u16) -> anyhow::Result<Self> {
        let stream = TcpStream::connect((host, port)).await?;
        if let Err(e) = stream.set_nodelay(true) {
            tracing::warn!("Failed to set TCP_NODELAY on client socket: {e}");
        }
        let (reader, writer) = stream.into_split();

        let read = ReadChannel::new(Box::new(reader));
        let write = WriteChannel::new(Box::new(writer));

        Ok(Self { write, read })
    }

    #[cfg(unix)]
    pub async fn from_uds_client(path: &std::path::Path) -> anyhow::Result<Self> {
        let stream = UnixStream::connect(path).await?;
        let (reader, writer) = stream.into_split();

        let read = ReadChannel::new(Box::new(reader));
        let write = WriteChannel::new(Box::new(writer));

        Ok(Self { write, read })
    }

    pub async fn from_tcp_server(port: u16) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port)).await?;
        tracing::info!("TCP server listening at {}", listener.local_addr()?);

        let (stream, addr) = listener.accept().await?;
        tracing::info!("Accepted connection from {}", addr);

        stream.set_nodelay(true)?;
        let (reader, writer) = stream.into_split();

        let read = ReadChannel::new(Box::new(reader));
        let write = WriteChannel::new(Box::new(writer));

        Ok(Self { write, read })
    }

    pub async fn send(&mut self, message: Message) -> anyhow::Result<()> {
        self.write.send(message).await
    }

    pub async fn recv(&mut self) -> Result<Option<Message>, ProtocolError> {
        self.read.recv().await
    }

    pub fn into_channels(self) -> (ReadChannel, WriteChannel) {
        (self.read, self.write)
    }

    /// Create a pair of connected in-memory DuplexChannels that can be used for
    /// internal/headless communication (in contrast to external communication
    /// over TCP/UDP/Unix sockets)
    pub fn in_memory(buffer_size_bytes: usize) -> (Self, Self) {
        let (client_to_server_tx, client_to_server_rx) = tokio::io::duplex(buffer_size_bytes);
        let (server_to_client_tx, server_to_client_rx) = tokio::io::duplex(buffer_size_bytes);

        let server =
            Self::from_streams(Box::new(server_to_client_tx), Box::new(client_to_server_rx));
        let client =
            Self::from_streams(Box::new(client_to_server_tx), Box::new(server_to_client_rx));

        (server, client)
    }
}

#[cfg(test)]
mod tests {
    use dapper_dap_protocol::protocol::Message;
    use dapper_dap_protocol::protocol::Request;
    use dapper_dap_protocol::protocol::Response;
    use dapper_dap_protocol::requests::RequestCommand;
    use dapper_dap_protocol::responses::ResponseBody;
    use dapper_dap_protocol::responses::ThreadsResponseBody;

    use super::*;

    #[tokio::test]
    async fn test_in_memory_bidirectional() {
        let (mut server, mut client) = DuplexChannel::in_memory(1024);

        let request = Request {
            seq: 1.into(),
            command: RequestCommand::Threads,
        };
        client.send(request.into()).await.unwrap();
        let received = server.recv().await.unwrap().unwrap();
        assert!(matches!(received, Message::Request(_)));

        let response = Response {
            seq: 1.into(),
            request_seq: 1.into(),
            success: true,
            message: None,
            body: ResponseBody::Threads(ThreadsResponseBody {
                ..Default::default()
            }),
        };
        server.send(response.into()).await.unwrap();
        let received = client.recv().await.unwrap().unwrap();
        assert!(matches!(received, Message::Response(_)));
    }

    #[tokio::test]
    async fn test_write_buffered_then_flush() {
        let (mut server, mut client) = DuplexChannel::in_memory(1024);

        let request1 = Request {
            seq: 1.into(),
            command: RequestCommand::Threads,
        };
        let request2 = Request {
            seq: 2.into(),
            command: RequestCommand::Threads,
        };

        client.write.write_buffered(request1.into()).await.unwrap();
        client.write.write_buffered(request2.into()).await.unwrap();
        client.write.flush().await.unwrap();

        let received1 = server.recv().await.unwrap().unwrap();
        assert!(matches!(received1, Message::Request(_)));
        let received2 = server.recv().await.unwrap().unwrap();
        assert!(matches!(received2, Message::Request(_)));
    }
}
