use crate::{
    config::{Config, MANIFEST_FILE_EXT},
    dht_node::DhtNode,
    file_manager::FileManager,
    manifest::Manifest,
    memmap::MemMap,
    proto::{Proto, ProtoReader, ProtoWriter},
    session::Session,
    shutdown::ShutdownNotifier,
};
use anyhow::Context;
use rustydht_lib::dht::operations::GetPeersResult;
use std::{
    collections::HashSet, env, fs, future::Future, net::SocketAddr, path::PathBuf, pin::Pin, sync::Arc, time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    runtime::{Builder as RuntimeBuilder, Runtime},
    sync::{Mutex, Notify},
};
use tracing::{debug, error, info};

type FileManagersT = Arc<Mutex<HashSet<Arc<FileManager<MemMap>>>>>;

pub struct NodeService {
    config: Config,
    runtime: Option<Runtime>,
    dht_node: Arc<DhtNode>,
    files_managers: FileManagersT,
    graceful_shutdown_notifier: Arc<Notify>,
}

impl NodeService {
    pub fn new(config: Config) -> Self {
        let graceful_shutdown_notifier = Arc::new(Notify::new());

        let runtime = RuntimeBuilder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to create tokio runtime");

        let dht_node = runtime.block_on(async {
            DhtNode::new(Duration::from_secs(15))
                .with_context(|| "failed to run DHT node")
                .unwrap()
        });
        // DHT has to be construct in tokio reactor as it binds tokio's UdpSocket under the hood

        Self {
            config,
            runtime: Some(runtime),
            dht_node: Arc::new(dht_node),
            files_managers: FileManagersT::default(),
            graceful_shutdown_notifier,
        }
    }

    #[tracing::instrument(skip_all)]
    pub fn start(&mut self) {
        type VecFutures = Vec<Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>>;
        let mut futures: VecFutures = VecFutures::new();

        futures.push({
            let dht = Arc::clone(&self.dht_node);
            Box::pin(async move { dht.run().await })
        });

        self.config.upload_files.take().into_iter().flatten().for_each(|file| {
            let port = self.config.node_address.port();

            let files_managers = Arc::clone(&self.files_managers);
            let dht = Arc::clone(&self.dht_node);

            futures.push(Box::pin(async move {
                announce_file_sharing(file, files_managers, dht, port).await
            }));
        });

        let runtime = self
            .runtime
            .as_ref()
            .expect("tokio reactor must be present to run node service");

        let shutdown_notifier =
            runtime.block_on(async { ShutdownNotifier::new(Arc::clone(&self.graceful_shutdown_notifier)) });

        self.config
            .manifest_files
            .take()
            .into_iter()
            .flatten()
            .for_each(|file| {
                let files_managers = Arc::clone(&self.files_managers);
                let dht = Arc::clone(&self.dht_node);
                let shutdown_notifier = shutdown_notifier.clone();

                futures.push(Box::pin(async move {
                    start_file_downloading(file, files_managers, dht, shutdown_notifier).await
                }));
            });

        futures.push({
            Box::pin(start_peers_listening(
                self.config.node_address,
                Arc::clone(&self.files_managers),
                shutdown_notifier,
            ))
        });

        let joined_futures = futures::future::try_join_all(futures);
        runtime.spawn(async move {
            if let Err(err) = joined_futures.await {
                error!("{err}");
            }
        });

        info!("started node service");
    }

    pub fn stop(&mut self) {
        debug!("stopping service");
        self.graceful_shutdown_notifier.notify_waiters();

        let runtime = self
            .runtime
            .take()
            .expect("node service cannot be stopped if it haven't been started");

        runtime.block_on(async {
            if let Some(dht_node) = Arc::get_mut(&mut self.dht_node) {
                dht_node.stop().await;
            }

            tokio::time::sleep(Duration::from_secs(5)).await
        });

        runtime.shutdown_background();

        info!("stopped node service");
    }
}

#[tracing::instrument(skip(dht, shutdown_notifier, files_managers), err)]
async fn start_file_downloading(
    manifest_path: PathBuf,
    files_managers: FileManagersT,
    dht: Arc<DhtNode>,
    shutdown_notifier: ShutdownNotifier,
) -> anyhow::Result<()> {
    let manifest = Manifest::from_json_file(manifest_path.as_path()).await?;
    debug!("loaded a manifest file from the {manifest_path:?} file");

    let file_manager = {
        let mut files_managers = files_managers.lock().await;
        if files_managers
            .iter()
            .any(|file_manager| file_manager.manifest().eq(&manifest))
        {
            info!(
                "{} is already loaded, no need to download it separately again",
                manifest.filename()
            );

            return Ok(());
        }

        let file_manager = Arc::new(FileManager::new(manifest).await?);
        files_managers.insert(Arc::clone(&file_manager));
        file_manager
    };

    let infohash = file_manager.manifest().infohash();
    //let mut known_peers: Vec<SocketAddr> = Vec::new();
    while !shutdown_notifier.should_stop().await {
        let get_peers = { dht.get_peers(infohash).await? };
        if !get_peers.peers().is_empty() {
            /*
            let unique_peers = get_peers
                .peers()
                .iter()
                .copied()
                .filter(|peer| !known_peers.contains(peer))
                .collect::<Vec<SocketAddr>>(); // we want to start sessions only with new peers

            known_peers.extend(unique_peers.iter());
            */
            debug!("starting download sessions for {infohash:#X?} info hash");
            start_downloading_sessions(
                get_peers.peers().iter().copied(),
                Arc::clone(&file_manager),
                shutdown_notifier.clone(),
            );
        }
        tokio::time::sleep(Duration::from_secs(15)).await;
    }

    Ok(())
}

#[tracing::instrument(skip(dht, files_managers), err)]
async fn announce_file_sharing(
    file: PathBuf,
    files_managers: FileManagersT,
    dht: Arc<DhtNode>,
    port: u16,
) -> anyhow::Result<()> {
    let manifest = process_gen_manifest(file).await?;
    let infohash = manifest.infohash().to_owned();

    let file_manager = FileManager::new(manifest).await?;
    {
        let mut files_managers = files_managers.lock().await;
        files_managers.insert(Arc::new(file_manager));
    };

    dht.announce_peer(&infohash, port).await?;

    Ok(())
}

#[tracing::instrument(ret)]
async fn process_gen_manifest(file: PathBuf) -> anyhow::Result<Manifest> {
    debug!("creating a Manifest from {file:?}");
    let manifest = Manifest::from_path(file.as_path()).await?;

    let mut output_file_path = env::current_dir().expect("current dir should be present");
    output_file_path.push("manifests/");

    fs::create_dir_all(output_file_path.as_path())
        .with_context(|| "failed to a create a directory for manifest files")?;

    output_file_path.push(manifest.filename());
    output_file_path.set_extension(MANIFEST_FILE_EXT);

    manifest.to_file(output_file_path.as_path()).await?;
    info!("wrote a Manifest of the {file:?} file to the {output_file_path:?} file",);

    Ok(manifest)
}

#[tracing::instrument(skip_all)]
fn start_downloading_sessions(
    peers: impl Iterator<Item = SocketAddr>,
    file_manager: Arc<FileManager<MemMap>>,
    shutdown_notifier: ShutdownNotifier,
) {
    debug!("starting sessions with get peers nodes");
    peers.for_each(|peer| {
        let file_manager = Arc::clone(&file_manager);
        let shutdown_notifier = shutdown_notifier.clone();

        tokio::spawn(async move { start_session_with_peer(peer, file_manager, shutdown_notifier).await });
    });
}

#[tracing::instrument(skip_all, fields(peer, filename = file_manager.manifest().filename(), file_infohash = format!("{:02x?}", file_manager.manifest().infohash()).as_str()))]
async fn start_session_with_peer(
    peer: SocketAddr,
    file_manager: Arc<FileManager<MemMap>>,
    shutdown_notifier: ShutdownNotifier,
) {
    debug!("connecting to {peer} peer...");
    let connection = TcpStream::connect(peer)
        .await
        .context(format!("failed connect to {peer}"))
        .unwrap();

    debug!("connected to {peer}");

    let session = Session::new(connection, shutdown_notifier, file_manager);
    if let Err(e) = session.serve_downloading().await {
        error!("a downloading session failed: {e}");
    }
}

#[tracing::instrument(skip(files_managers, shutdown_notifier), err)]
async fn start_peers_listening(
    node_address: SocketAddr,
    files_managers: FileManagersT,
    shutdown_notifier: ShutdownNotifier,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(node_address).await?;
    debug!("listening for new connections");

    while !shutdown_notifier.should_stop().await {
        while let Ok((connection, peer_addr)) = listener.accept().await {
            info!("new connection from {peer_addr}");

            let files_managers = Arc::clone(&files_managers);
            let shutdown_notifier = shutdown_notifier.clone();
            tokio::spawn(async move {
                let mut reader = ProtoReader::new(connection);
                let message = reader.read().await.context("failed to read init message").unwrap();

                let mut writer = ProtoWriter::new(reader.into_inner());

                let infohash = if let Proto::Init(infohash) = message {
                    infohash
                } else {
                    writer
                        .write(Proto::Error(format!(
                            "incorrect first message(expected Init with desired file hash, but got {message:?}"
                        )))
                        .await
                        .unwrap();

                    return;
                };

                let file_manager = {
                    let files_managers = files_managers.lock().await;
                    files_managers
                        .iter()
                        .find(|file_manager| file_manager.manifest().infohash().eq(&infohash))
                        .map(Arc::clone)
                };

                if let Some(file_manager) = file_manager {
                    start_session_with_accepted_peer(writer.into_inner(), shutdown_notifier, file_manager, peer_addr)
                        .await
                } else {
                    writer
                        .write(Proto::Error(format!(
                            "i don't have a file with {infohash:02x?} infohash"
                        )))
                        .await
                        .unwrap();
                }
            });
        }
    }

    debug!("stopped listener");

    Ok(())
}

#[tracing::instrument(skip_all, fields(peer_addr, filename = file_manager.manifest().filename(), file_infohash = format!("{:02x?}", file_manager.manifest().infohash()).as_str()))]
async fn start_session_with_accepted_peer(
    connection: TcpStream,
    shutdown_notifier: ShutdownNotifier,
    file_manager: Arc<FileManager<MemMap>>,
    peer_addr: SocketAddr,
) {
    info!("starting new session with {peer_addr} node");

    let session = Session::new(connection, shutdown_notifier, file_manager);
    if let Err(e) = session.serve_sharing().await {
        error!("sharing session with {peer_addr} failed: {e}");
    }
}
