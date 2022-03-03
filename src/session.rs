use crate::{file_manager::FileManager, proto::Proto};
use anyhow::anyhow;
use futures::{future::FutureExt, TryFutureExt};
use log::debug;
use std::{
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use std::error::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    },
    pin,
    sync::{Mutex, Notify},
};

pub struct Session {
    file_manager: Arc<FileManager>,
    // _writer: Arc<Mutex<FrameWriterT>>,
    //  reader: FramedReaderT,
    graceful_shutdown_notifier: Arc<Notify>,
    should_stop: Arc<Mutex<bool>>,
}

impl Session {
    pub fn new(graceful_shutdown_notifier: Arc<Notify>, file_manager: Arc<FileManager>) -> Self {
        let should_stop = Arc::new(Mutex::new(false));

        tokio::spawn(setup_graceful_shutdown_notifier_task(
            Arc::clone(&graceful_shutdown_notifier),
            Arc::clone(&should_stop),
        ));

        Self {
            file_manager,
            graceful_shutdown_notifier,
            should_stop,
        }
    }
}

async fn setup_graceful_shutdown_notifier_task(graceful_shutdown_notifier: Arc<Notify>, should_stop: Arc<Mutex<bool>>) {
    graceful_shutdown_notifier.notified().await;
    {
        let mut should_stop = should_stop.lock().await;
        *should_stop = true;
    }
}

impl tower::Service<Proto> for Session {
    type Response = Proto;
    type Error = anyhow::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if *self.should_stop.blocking_lock() {
            Poll::Ready(Err(anyhow!("session stopped")))
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn call(&mut self, req: Proto) -> Self::Future
    {
        Box::pin(async { Ok(Proto::Init) })
    }
}

pub struct ReadLayer {
    reader: Arc<Mutex<OwnedReadHalf>>,
}

impl ReadLayer {
    pub fn new(reader: OwnedReadHalf) -> Self {
        Self {
            reader: Arc::new(Mutex::new(reader))
        }
    }
}

impl<S> tower::Layer<S> for ReadLayer
    where
        S: tower::Service<Proto>,
{
    type Service = ReadService<S>;

    fn layer(&self, service: S) -> Self::Service {
        ReadService {
            service: Arc::new(Mutex::new(service)),
            reader: Arc::clone(&self.reader),
        }
    }
}

pub struct ReadService<S: tower::Service<Proto>> {
    service: Arc<Mutex<S>>,
    reader: Arc<Mutex<OwnedReadHalf>>,
}

impl<S> tower::Service<()> for ReadService<S>
    where
        S: tower::Service<Proto> + 'static,
        <S>::Error: Send + Sync + Error,
{
    type Response = ();
    type Error = anyhow::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let future = self
            .reader
            .lock()
            .then(|reader| async move { reader.readable().await });

        pin!(future);

        future.poll(cx).map_err(Into::into)
    }

    fn call(&mut self, _: ()) -> Self::Future
    {
        let reader = self.reader.clone();
        let service = self.service.clone();

        let mut buf = Vec::with_capacity(512);
        let future =
            reader
                .lock_owned()
                .then(|mut reader| async move { reader.read(&mut buf).await.map(|_| (buf)).map_err(Into::into) })
                .and_then(|buf| async move {
                    let mut service = service.lock_owned().await;
                    let proto = serde_json::from_slice::<Proto>(&buf)?;
                    service.call(proto).await.map(|_| ()).map_err(Into::into)
                });

        Box::pin(future)
    }
}

pub struct WriteService
{
    writer: Arc<Mutex<OwnedWriteHalf>>,
}

impl tower::Service<Proto> for WriteService
{
    type Response = ();
    type Error = anyhow::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let future = self
            .writer
            .lock()
            .then(|reader| async move { reader.writable().await });

        pin!(future);

        future.poll(cx).map_err(Into::into)
    }

    fn call(&mut self, req: Proto) -> Self::Future
    {
        let writer = Arc::clone(&self.writer);
        let future = writer.lock_owned().then(|mut writer| async move {
            let bytes = serde_json::to_vec(&req).map_err(anyhow::Error::new)?;
            writer.write_all(&bytes).await.map_err(Into::into)
        });

        Box::pin(future)
    }
}

