use crate::{
    config::{Config, MANIFEST_FILE_EXT},
    dht_node::DhtNode,
    file_manager::FileManager,
    manifest::Manifest,
    session::Session,
};
use anyhow::Context;
use log::{debug, error, info};
use rustydht_lib::dht::operations::GetPeersResult;
use std::{
    collections::HashSet, env, fs, future::Future, net::SocketAddr, path::PathBuf, pin::Pin, sync::Arc, time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    runtime::{Builder as RuntimeBuilder, Runtime},
    sync::{Mutex, Notify},
};

type FileManagersT = Arc<Mutex<HashSet<Arc<FileManager>>>>;

pub struct NodeService {
    config: Config,
    runtime: Option<Runtime>,
    graceful_shutdown_notifier: Arc<Notify>,
    should_stop: Arc<Mutex<bool>>,
    dht_node: Arc<DhtNode>,
    files_managers: FileManagersT,
}

impl NodeService {
    pub fn new(config: Config) -> Self {
        let graceful_shutdown_notifier = Arc::new(Notify::new());

        let runtime = RuntimeBuilder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to create tokio runtime");

        let dht_node = runtime.block_on(async {
            DhtNode::new(Duration::from_secs(60))
                .with_context(|| "failed to run DHT node")
                .unwrap()
        });
        // DHT has to be construct in tokio reactor as it binds tokio's UdpSocket under the hood

        Self {
            config,
            runtime: Some(runtime),
            graceful_shutdown_notifier,
            should_stop: Arc::new(Mutex::new(false)),
            dht_node: Arc::new(dht_node),
            files_managers: Arc::new(Mutex::new(HashSet::default())),
        }
    }

    pub fn start(&mut self) {
        type VecFutures = Vec<Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>>;
        let mut futures: VecFutures = VecFutures::new();

        futures.push({
            let dht = Arc::clone(&self.dht_node);
            Box::pin(async move { dht.run().await })
        });

        if let Some(file_list) = self.config.upload_files.take() {
            let port = self.config.node_address.port();

            for file in file_list {
                let files_managers = Arc::clone(&self.files_managers);
                let dht = Arc::clone(&self.dht_node);

                futures.push(Box::pin(async move {
                    let manifest = process_gen_manifest(file).await?;
                    let infohash = manifest.infohash()?;

                    let file_manager = FileManager::new(manifest).await?;
                    {
                        let mut files_managers = files_managers.lock().await;
                        files_managers.insert(Arc::new(file_manager));
                    };

                    dht.announce_peer(&infohash, port).await?;

                    Ok(())
                }));
            }
        }

        if let Some(manifest_files_list) = self.config.manifest_files.take() {
            for file in manifest_files_list {
                let files_managers = Arc::clone(&self.files_managers);
                let dht = Arc::clone(&self.dht_node);
                let graceful_shutdown_notifier = Arc::clone(&self.graceful_shutdown_notifier);

                futures.push(Box::pin(async move {
                    let manifest = Manifest::from_json_file(file.as_path()).await?;
                    debug!("loaded a manifest file from the {file:?} file");
                    let infohash = manifest.infohash()?;

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

                    let get_peers = { dht.get_peers(&infohash).await? };
                    debug!("starting sessions for {infohash:#X?} info hash");
                    start_sessions_with_get_peers_nodes(get_peers, file_manager, graceful_shutdown_notifier);

                    Ok(())
                }));
            }
        }

        futures.push({
            Box::pin(start_service_listener(
                self.config.node_address,
                Arc::clone(&self.graceful_shutdown_notifier),
                Arc::clone(&self.files_managers),
                Arc::clone(&self.should_stop),
            ))
        });

        futures.push({
            let should_stop = Arc::clone(&self.should_stop);
            let graceful_shutdown_notifier = Arc::clone(&self.graceful_shutdown_notifier);

            Box::pin(async move {
                graceful_shutdown_notifier.notified().await;
                let mut should_stop = should_stop.lock().await;
                *should_stop = true;

                Ok(())
            })
        });

        /*
        futures.push({
            let dht_node = Arc::clone(&self.dht_node);
            let graceful_shutdown_notifier = Arc::clone(&self.graceful_shutdown_notifier);

            Box::pin(async move {
                let receiver = {
                    let dht_node = dht_node.lock().await;
                    dht_node.subscribe()
                };

                handle_dht_event(receiver, graceful_shutdown_notifier).await;

                Ok(())
            })
        }); */

        let joined_futures = futures::future::join_all(futures);
        self.runtime
            .as_ref()
            .expect("tokio reactor must be present to run node service")
            .spawn(async move {
                joined_futures.await.into_iter().for_each(|fur_resl| {
                    if let Err(err) = fur_resl {
                        error!("{err}");
                    }
                })
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

async fn process_gen_manifest(file: PathBuf) -> anyhow::Result<Manifest> {
    debug!("creating a Manifest from {file:?}");
    let manifest = Manifest::from_path(file.as_path()).await?;

    let filename = manifest.filename.as_str();

    let mut output_file_path = env::current_dir().expect("current dir should be present");
    output_file_path.push("manifests/");

    fs::create_dir_all(output_file_path.as_path())
        .with_context(|| "failed to a create a directory for manifest files")?;

    output_file_path.push(filename);
    output_file_path.set_extension(MANIFEST_FILE_EXT);

    manifest.to_file(output_file_path.as_path()).await?;
    info!("wrote a Manifest of the {file:?} file to the {output_file_path:?} file",);

    Ok(manifest)
}

fn start_sessions_with_get_peers_nodes(
    get_peers: GetPeersResult,
    file_manager: Arc<FileManager>,
    graceful_shutdown_notifier: Arc<Notify>,
) {
    debug!("starting session with get peers nodes");
    for node in get_peers.peers() {
        let graceful_shutdown_notifier = Arc::clone(&graceful_shutdown_notifier);
        let file_manager = Arc::clone(&file_manager);

        tokio::spawn(async move {
            debug!("connecting to {node} node...");
            let connection = TcpStream::connect(node)
                .await
                .context(format!("failed connect to {node}"))
                .unwrap();
            let mut session = Session::new(connection, graceful_shutdown_notifier, file_manager);
            session.serve().await;
        });
    }
}

/*
async fn handle_dht_event(mut receiver: Receiver<DHTEvent>, graceful_shutdown_notifier: Arc<Notify>) {
    while let Some(DHTEvent {
        event_type: MessageReceived(MessageReceivedEvent {
            message: Message { message_type, .. },
        }),
    }) = receiver.recv().await
    {
        match message_type {
            MessageType::Request(_) => {}
            MessageType::Response(response) => {
                if let ResponseSpecific::SampleInfoHashesResponse(SampleInfoHashesResponseArguments { nodes, .. }) =
                    response
                {
                    for node in nodes {
                        let graceful_shutdown_notifier = Arc::clone(&graceful_shutdown_notifier);
                        tokio::spawn(async move {
                            debug!("starting session with {} {}", node.address, node.id);
                            let connection = TcpStream::connect(node.address)
                                .await
                                .context(format!("Failed to connect to {} with {} id", node.address, node.id))
                                .unwrap();

                            let mut session = Session::new(connection, graceful_shutdown_notifier);
                            session.serve().await;
                        });
                    }
                }
            }
            MessageType::Error(error) => {
                error!("dht error message: {}", error.description);
            }
        }
    }
} */

async fn start_service_listener(
    node_address: SocketAddr,
    graceful_shutdown_notifier: Arc<Notify>,
    files_managers: FileManagersT,
    should_stop: Arc<Mutex<bool>>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(node_address).await?;
    debug!("listening for new connections");

    while !*should_stop.lock().await {
        while let Ok((connection, peer_addr)) = listener.accept().await {
            info!("new connection from {peer_addr}");

            let file_manager = {
                // stumb
                let files_managers = files_managers.lock().await;
                files_managers.iter().find(|_| true).map(Arc::clone)
            };

            if let Some(file_manager) = file_manager {
                tokio::spawn({
                    let graceful_shutdown_notifier = Arc::clone(&graceful_shutdown_notifier);

                    async move {
                        info!("starting new session with {peer_addr} node");
                        let mut session = Session::new(connection, graceful_shutdown_notifier, file_manager);
                        session.serve().await
                    }
                });
            }
        }
    }

    debug!("stopped listener");

    Ok(())
}
