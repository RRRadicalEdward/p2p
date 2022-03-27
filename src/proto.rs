use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const READER_BUF_CAPACITY: usize = 512;

#[derive(Serialize, Deserialize, Debug)]
pub enum Proto {
    Init(Vec<u8>),
    Request(usize),
    Data(Option<Vec<u8>>),
    Error(String),
    Fin,
}

pub struct ProtoReader<R>
where
    R: AsyncRead + Unpin,
{
    reader: R,
    buf: [u8; READER_BUF_CAPACITY],
}

impl<R> ProtoReader<R>
where
    R: AsyncRead + Unpin,
{
    pub fn new(reader: R) -> Self {
        ProtoReader {
            reader,
            buf: [0; READER_BUF_CAPACITY],
        }
    }

    #[tracing::instrument(skip(self), ret, err)]
    pub async fn read(&mut self) -> anyhow::Result<Proto> {
        let were_read = self
            .reader
            .read(&mut self.buf)
            .await
            .context("failed to read a proto message")?;

        serde_json::from_slice(&self.buf[0..were_read])
            .map_err(Into::<anyhow::Error>::into)
            .context("serde failed to deserialized a proto message")
    }

    pub fn into_inner(self) -> R {
        self.reader
    }
}

pub struct ProtoWriter<W>
where
    W: AsyncWrite + Unpin,
{
    writer: W,
}

impl<W> ProtoWriter<W>
where
    W: AsyncWrite + Unpin,
{
    pub fn new(writer: W) -> Self {
        ProtoWriter { writer }
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn write(&mut self, message: Proto) -> anyhow::Result<()> {
        let message = serde_json::to_vec(&message).context("serde failed to serialize a proto message")?;

        self.writer
            .write_all(&message)
            .await
            .context("failed to send an message")
    }

    pub fn into_inner(self) -> W {
        self.writer
    }
}
