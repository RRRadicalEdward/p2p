use std::{collections::HashMap, net::SocketAddr};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Default, Debug)]
pub struct RoutingManager {
    routing_table: Mutex<HashMap<Uuid, SocketAddr>>,
}

impl RoutingManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn add_one(&self, element: (Uuid, SocketAddr)) {
        let mut routing_table = self.routing_table.lock().await;
        routing_table.insert(element.0, element.1);
    }

    pub async fn add<I: IntoIterator<Item = (Uuid, SocketAddr)>>(&self, elements: I) {
        let mut routing_table = self.routing_table.lock().await;
        routing_table.extend(elements);
    }

    pub async fn remove(&self, uuid: &Uuid) {
        let mut routing_table = self.routing_table.lock().await;
        routing_table.remove(uuid);
    }

    pub async fn view(&self) -> HashMap<Uuid, SocketAddr> {
        let routing_table = self.routing_table.lock().await;
        routing_table.clone()
    }
}
