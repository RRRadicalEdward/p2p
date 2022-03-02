use crate::{file_manager::FileManager, proto::Proto};
use anyhow::anyhow;
use futures::{future::FutureExt, TryFutureExt, TryStreamExt};
use log::debug;
use sha1::digest::Output;
use std::{
    fs::read,
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncReadExt, Interest, ReadBuf, Ready},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf, ReadHalf, WriteHalf},
        TcpStream,
    },
    pin,
    sync::{Mutex, Notify},
};
use tower::Service;
//use tokio_serde::{formats::SymmetricalJson, SymmetricallyFramed};
//use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

//type FramedReaderT = SymmetricallyFramed<FramedRead<OwnedReadHalf, LengthDelimitedCodec>, (), SymmetricalJson<()>>;
//type FrameWriterT = SymmetricallyFramed<FramedWrite<OwnedWriteHalf, LengthDelimitedCodec>, (), SymmetricalJson<()>>;

pub struct Session {
    peer_addr: SocketAddr,
    file_manager: Arc<FileManager>,
    // _writer: Arc<Mutex<FrameWriterT>>,
    //  reader: FramedReaderT,
    graceful_shutdown_notifier: Arc<Notify>,
    should_stop: Arc<Mutex<bool>>,
}

impl Session {
    pub fn new(connection: TcpStream, graceful_shutdown_notifier: Arc<Notify>, file_manager: Arc<FileManager>) -> Self {
        let peer_addr = connection
            .peer_addr()
            .expect("I would be wondered if getting peer addr can fail 🙂");
        /*
        let (read, write) = connection.into_split();

        let reader = tokio_serde::SymmetricallyFramed::new(
            FramedRead::new(read, LengthDelimitedCodec::new()),
            SymmetricalJson::<()>::default(),
        );

        let writer = tokio_serde::SymmetricallyFramed::new(
            FramedWrite::new(write, LengthDelimitedCodec::new()),
            SymmetricalJson::<()>::default(),
        );
        */
        Self {
            peer_addr,
            file_manager,
            //   reader,
            //   _writer: Arc::new(Mutex::new(writer)),
            graceful_shutdown_notifier,
            should_stop: Arc::new(Mutex::new(false)),
        }
    }

    pub async fn serve(&mut self) {
        let graceful_shutdown_notifier_task = tokio::spawn(setup_graceful_shutdown_notifier_task(
            Arc::clone(&self.graceful_shutdown_notifier),
            Arc::clone(&self.should_stop),
        ));

        /*
        while !*self.should_stop.lock().await {
            if let Some(message) = self.reader.try_next().await.unwrap() {
                debug!("got {message:?} message from {} node", self.peer_addr);
            }
        }
        */

        debug!("finishing session with {} node", self.peer_addr);
        graceful_shutdown_notifier_task.abort(); // peer disconnected so we do not need to wait anymore for the graceful shutdown notifier signal
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

    fn call(&mut self, req: Proto) -> Self::Future {
        //match req {}

        Box::pin(async { Ok(Proto::Init) })
    }
}

struct ReadLayer {
    reader: Arc<Mutex<OwnedReadHalf>>,
}

impl<S> tower::Layer<S> for ReadLayer
where
    S: tower::Service<Vec<u8>>,
{
    type Service = ReadService<S>;

    fn layer(&self, service: S) -> Self::Service {
        ReadService {
            service,
            reader: Arc::clone(&self.reader),
        }
    }
}

struct ReadService<S: tower::Service<Vec<u8>>> {
    service: S,
    reader: Arc<Mutex<OwnedReadHalf>>,
}

impl<S> tower::Service<()> for ReadService<S>
where
    S: tower::Service<Vec<u8>>,
{
    type Response = ();
    type Error = anyhow::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let future = self
            .reader
            .lock()
            .then(|reader| async move { reader.ready(Interest::READABLE).await });

        pin!(future);

        match future.poll(cx) {
            Poll::Ready(Ok(ready)) => {
                if ready.is_readable() {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e.into())),
            Poll::Pending => Poll::Pending,
        }
    }

    fn call(&mut self, req: ()) -> Self::Future {
        let mut buf = Vec::with_capacity(512);
        let future = self
            .reader
            .lock()
            .then(|mut reader| async { reader.read(&mut buf).await.map(|_| ()).map_err(|err| err.into()) })
            .map_ok(|_| self.service.call(buf));

        pin!(future);

        future
    }
}

struct WriteLayer {
    write: OwnedWriteHalf,
}
