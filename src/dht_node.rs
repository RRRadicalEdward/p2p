use anyhow::Context;
use log::{debug, trace};
use rustydht_lib::{
    common::{ipv4_addr_src::IPV4Consensus, Id, Node},
    dht::{
        dht_event::DHTEvent,
        operations::{announce_peer, find_node, get_peers, GetPeersResult},
        DHTBuilder, DHTSettingsBuilder, DHT,
    },
    shutdown::{create_shutdown, ShutdownSender},
};
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    time::Duration,
};
use tokio::sync::mpsc::Receiver;

pub struct DhtNode {
    dht: DHT,
    timeout_duration: Duration,
    shutdown_notifier: ShutdownSender,
}

impl DhtNode {
    pub fn new(timeout_duration: Duration) -> anyhow::Result<Self> {
        let (sender, receiver) = create_shutdown();

        let port = portpicker::pick_unused_port().expect("no free port in the system :(");
        let dht = DHTBuilder::new()
            .listen_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port))
            .settings(
                DHTSettingsBuilder::new()
                    .read_only(false)
                    .routers(vec!["localhost:6882".to_string()])
                    .build(),
            )
            .ip_source(Box::new(IPV4Consensus::new(0, 10)))
            .build(receiver)?;

        debug!("running DHT peer on 0.0.0.0:{port}");

        Ok(Self {
            dht,
            timeout_duration,
            shutdown_notifier: sender,
        })
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        self.dht.run_event_loop().await.map_err(|err| err.into())
    }

    pub async fn announce_peer<H: AsRef<[u8]>>(&self, info_hash: H, port: u16) -> anyhow::Result<Vec<Node>> {
        debug!("announce infohash: {:02x?}", info_hash.as_ref());
        let id = Id::from_bytes(info_hash.as_ref())?;
        announce_peer(&self.dht, id, Some(port), self.timeout_duration)
            .await
            .with_context(|| format!("announce peer failed for infohash {:02x?}", info_hash.as_ref()))
    }

    pub async fn find_nodes<H: AsRef<[u8]>>(&self, info_hash: H) -> anyhow::Result<Vec<Node>> {
        debug!("find nodes for infohash {:02x?}", info_hash.as_ref());
        let id = Id::from_bytes(info_hash.as_ref())?;
        find_node(&self.dht, id, self.timeout_duration)
            .await
            .with_context(|| format!("find nodes failed for infohash {:02x?}", info_hash.as_ref()))
    }

    pub async fn get_peers<H: AsRef<[u8]>>(&self, info_hash: H) -> anyhow::Result<GetPeersResult> {
        debug!("get peers for infohash {:02x?}", info_hash.as_ref());
        let id = Id::from_bytes(info_hash.as_ref())?;

        get_peers(&self.dht, id, self.timeout_duration)
            .await
            .with_context(|| format!("get peers failed for infohash {:02x?}", info_hash.as_ref()))
    }

    pub async fn stop(&mut self) {
        trace!("stopping DHT node");
        self.shutdown_notifier.shutdown().await
    }

    pub fn subscribe(&self) -> Receiver<DHTEvent> {
        self.dht.subscribe()
    }
}
