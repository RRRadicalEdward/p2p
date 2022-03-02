use serde::{Deserialize, Serialize};
/*
use std::{collections::HashMap, net::SocketAddr};
use uuid::Uuid;
#[derive(Debug, PartialEq, Deserialize, Serialize)]
pub enum PeerMessage {
    RequestMessage(PeerRequestMessage),
    ResponseMessage(PeerResponseMessage),
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
pub enum PeerRequestMessage {
    Join(Uuid),
    Leave,
    CopyRoutingTable,
    FindClosest,
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
pub enum PeerResponseMessage {
    CopyRoutingTable(HashMap<Uuid, SocketAddr>),
    Join(Uuid),
    FindClosest((Uuid, SocketAddr)),
}
*/

#[derive(Serialize, Deserialize)]
pub enum Proto {
    Init,
    Request(usize),
    Data(Option<Vec<u8>>),
    Fin,
}
