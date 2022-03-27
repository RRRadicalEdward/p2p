use memmap2::MmapMut;
use std::{
    fs::File,
    io::{Cursor, Error, Seek, SeekFrom, Write},
    ops::{Deref, DerefMut},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncSeek, AsyncWrite, ReadBuf},
    pin,
};

pub struct MemMap {
    memmap: Cursor<MmapMut>,
    seek_pos: u64,
}

impl MemMap {
    pub fn new(file: File) -> anyhow::Result<Self> {
        Ok(Self {
            memmap: Cursor::new(unsafe { MmapMut::map_mut(&file)? }),
            seek_pos: 0,
        })
    }
}

impl AsyncRead for MemMap {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let data = self.memmap.get_ref().deref();
        pin!(data);

        data.poll_read(cx, buf)
    }
}

impl AsyncWrite for MemMap {
    fn poll_write(mut self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize, Error>> {
        Poll::Ready(self.memmap.get_mut().deref_mut().write_all(buf).map(|_| buf.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Poll::Ready(self.memmap.get_mut().flush_async())
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Poll::Ready(self.memmap.get_mut().flush())
    }
}

impl AsyncSeek for MemMap {
    fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> std::io::Result<()> {
        self.memmap.seek(position).map(|pos| {
            self.seek_pos = pos;
        })
    }

    fn poll_complete(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<u64>> {
        Poll::Ready(Ok(self.seek_pos))
    }
}
