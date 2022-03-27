use crate::{
    file_manager::FileManager,
    memmap::MemMap,
    proto::{Proto, ProtoReader, ProtoWriter},
    shutdown::ShutdownNotifier,
};
use anyhow::Context;
use std::sync::Arc;
use tokio::{
    io::{AsyncRead, AsyncSeek, AsyncWrite, AsyncWriteExt},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    },
};
use tracing::{debug, error, info};

pub struct Session<RWS, W, R>
where
    RWS: AsyncWrite + AsyncRead + AsyncSeek + Unpin,
    W: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    file_manager: Arc<FileManager<RWS>>,
    writer: ProtoWriter<W>,
    reader: ProtoReader<R>,
    shutdown_notifier: ShutdownNotifier,
}

impl Session<MemMap, OwnedWriteHalf, OwnedReadHalf> {
    pub fn new(
        connection: TcpStream,
        shutdown_notifier: ShutdownNotifier,
        file_manager: Arc<FileManager<MemMap>>,
    ) -> Self {
        let (reader, writer) = connection.into_split();

        Self {
            file_manager,
            reader: ProtoReader::new(reader),
            writer: ProtoWriter::new(writer),
            shutdown_notifier,
        }
    }
}

impl<RWS, W, R> Session<RWS, W, R>
where
    RWS: AsyncRead + AsyncWrite + AsyncSeek + Unpin,
    W: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    #[tracing::instrument(skip(self), err, fields(filename = self.file_manager.manifest().filename(), file_infohash = format!("{:02x?}", self.file_manager.manifest().infohash()).as_str()))]
    pub async fn serve_sharing(mut self) -> anyhow::Result<()> {
        while !self.shutdown_notifier.should_stop().await {
            let message = self.read_message().await?;

            debug!("got {message:?} message");

            match message {
                Proto::Request(index) => {
                    let data = match self.file_manager.filled_chunk(index).await {
                        Ok(data) => data,
                        Err(err) => {
                            let _ = self
                                .writer
                                .write(Proto::Error(format!(
                                    "error occurred while trying to get a chunk undex {index} index: {err}"
                                )))
                                .await
                                .map_err(|err| error!("sending an error message failed: {err}"));

                            break;
                        }
                    };

                    match self.writer.write(Proto::Data(data)).await {
                        Ok(()) => {}
                        Err(err) => {
                            error!("failed to send a chunk of data undex {index} index: {err}");
                            break;
                        }
                    }
                }
                Proto::Fin => {
                    debug!("closing sharing {}", self.file_manager.manifest().filename());
                    break;
                }
                Proto::Error(err) => {
                    error!("peer sent an error message: {err}. closing sharing.");
                }
                _ => {
                    error!("a peer send an incorrect message. closing connection.");
                    self.writer
                        .write(Proto::Error(format!("unexpected message {message:?}")))
                        .await?;

                    break;
                }
            }
        }

        self.writer.write(Proto::Fin).await?;
        self.writer.into_inner().shutdown().await?;

        Ok(())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn serve_downloading(mut self) -> anyhow::Result<()> {
        self.writer
            .write(Proto::Init(self.file_manager.manifest().infohash().to_owned()))
            .await
            .context("failed to send Init proto message")?;

        while !self.shutdown_notifier.should_stop().await {
            let message = self.read_message().await?;

            match message {
                Proto::Data(data) => {
                    if let Some(data) = data {
                        match self.file_manager.fill_chunk(data).await {
                            Ok(()) => {}
                            Err(err) => {
                                error!("an error happened when trying to fill a chunk of data: {err}");
                                self.send_error_with_fin_message(format!(
                                    "an error happened when trying to fill a chunk of data: {err}"
                                ))
                                .await?;
                            }
                        }
                    }

                    match self.file_manager.absent_chunk_index().await {
                        Some(index) => {
                            self.writer.write(Proto::Request(index)).await?;
                        }
                        None => {
                            info!("the file should be done");
                            break;
                        }
                    }
                }
                Proto::Fin => {
                    let manifest = self.file_manager.manifest();
                    debug!(
                        "closing downloading {}({:02x?})",
                        manifest.filename(),
                        manifest.infohash(),
                    );
                    break;
                }
                Proto::Error(err) => {
                    error!("peer sent an error message: {err}. closing sharing.");
                }
                _ => {
                    error!("a peer send an incorrect message. closing connection.");
                    self.writer
                        .write(Proto::Error(format!("unexpected message {message:?}")))
                        .await?;
                    break;
                }
            }
        }

        self.writer.write(Proto::Fin).await?;
        self.writer.into_inner().shutdown().await?;
        Ok(())
    }

    #[tracing::instrument(skip_all, ret, err)]
    async fn read_message(&mut self) -> anyhow::Result<Proto> {
        match self.reader.read().await {
            Ok(message) => Ok(message),
            Err(err) => {
                self.send_error_with_fin_message(format!("failed to deserialize a message: {err}"))
                    .await?;

                Err(err)
            }
        }
    }

    #[tracing::instrument(skip_all, err)]
    async fn send_error_with_fin_message(&mut self, error_message: String) -> anyhow::Result<()> {
        let _ = self
            .writer
            .write(Proto::Error(error_message))
            .await
            .map_err(|err| error!("sending an error message failed: {err}"));

        self.writer.write(Proto::Fin).await
    }
}
