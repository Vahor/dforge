use std::{collections::HashMap, sync::RwLock, time::SystemTime};

use core::fmt::Debug;
use pcap::{Activated, Capture};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::{
    node::Node,
    sniffer::parser::{
        metadata::{PacketHeader, PacketMetadata, ParseResult},
        packet::PacketParser,
        wrapper::DataWrapper,
    },
};

use super::{parser::packet::Packet, protocol::protocol::EventId};

pub type Listener = fn(&Packet, &Node);
pub type ListenerId = &'static str;
pub type Subscription = (ListenerId, Listener);

#[derive(Debug)]
pub struct PacketListener {
    subscriptions: Arc<Mutex<HashMap<EventId, Vec<Subscription>>>>,
    node: Option<Arc<Node>>,
    pub last_packet_time: Arc<RwLock<u128>>,
}

impl PacketListener {
    pub fn new() -> PacketListener {
        return PacketListener {
            subscriptions: Arc::new(Mutex::new(HashMap::new())),
            node: None,
            last_packet_time: Arc::new(RwLock::new(0)),
        };
    }

    pub fn set_node(&mut self, node: Arc<Node>) {
        self.node = Some(node);
    }

    pub fn subscribe(&mut self, event: EventId, listener_id: ListenerId, listener: Listener) {
        info!("Subscribing to event: {:?} for {:?}", event, listener_id);
        self.subscriptions
            .lock()
            .unwrap()
            .entry(event)
            .or_default()
            .push((listener_id, listener));
    }

    pub fn unsubscribe(&mut self, event: &EventId, listener_id: ListenerId) {
        info!(
            "Unsubscribing from event: {:?} for {:?}",
            event, listener_id
        );
        self.subscriptions
            .lock()
            .unwrap()
            .get_mut(event)
            .map(|listeners| listeners.retain(|(id, _)| id != &listener_id));
    }

    pub fn notify(&self, event: &Packet) {
        PacketListener::_notify(
            &self.subscriptions.lock().unwrap(),
            event,
            &self.node.as_ref().unwrap(),
        );
    }

    fn _notify(subscriptions: &HashMap<EventId, Vec<Subscription>>, packet: &Packet, node: &Node) {
        let listeners = subscriptions.get(&packet.id);
        if let Some(listeners) = listeners {
            for (_, listener) in listeners {
                listener(packet, node);
            }
        }
    }

    pub fn has_subscriptions_for(&self, event: &EventId, listener_id: ListenerId) -> bool {
        let subscriptions = self.subscriptions.lock().unwrap();
        subscriptions.get(event).map_or(false, |listeners| {
            listeners.iter().any(|(id, _)| id == &listener_id)
        })
    }

    pub fn has_subscriptions(&self, event: &EventId) -> bool {
        return PacketListener::_has_subscriptions(&self.subscriptions.lock().unwrap(), event);
    }

    fn _has_subscriptions(
        subscriptions: &HashMap<EventId, Vec<Subscription>>,
        event: &EventId,
    ) -> bool {
        return subscriptions
            .get(event)
            .map_or(false, |listeners| !listeners.is_empty());
    }

    pub fn run(&self) -> Result<(), PacketListenerError> {
        if self.node.is_none() {
            return Err(PacketListenerError::InvalidCaptureDevice);
        }

        let config = self.node.as_ref().unwrap().config.config.read().unwrap();
        let interface = config.network.interface.as_str();
        let port = config.network.port;

        info!(
            "Starting sniffer on interface: {} and port: {}",
            interface, port
        );

        let mut cap = Capture::from_device(interface)
            .unwrap()
            .immediate_mode(true)
            .open()
            .expect("Failed to open device");
        cap.direction(pcap::Direction::In).unwrap();

        cap.filter(format!("tcp port {}", port).as_str(), false)
            .unwrap();

        self.run_with_capture(cap.into())
    }

    pub fn run_with_capture(
        &self,
        mut cap: Capture<dyn Activated>,
    ) -> Result<(), PacketListenerError> {
        if self.node.is_none() {
            return Err(PacketListenerError::InvalidCaptureDevice);
        }

        debug!("Running packet listener");
        let subscriptions = self.subscriptions.clone();
        let procol_manager = self.node.as_ref().unwrap().protocol.clone();
        let node = self.node.clone().unwrap();
        let last_packet_time = self.last_packet_time.clone();

        tauri::async_runtime::spawn(async move {
            let buffer = &mut DataWrapper::new(Vec::new());
            let mut last_packet_header: Option<PacketHeader> = None;

            while let Ok(packet) = cap.next_packet() {
                let data = packet.data.to_vec();
                let now = SystemTime::now();

                *last_packet_time.write().unwrap() = now
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_millis();

                let packet_header = PacketHeader::from_vec(&data);
                if packet_header.is_err() {
                    warn!("Failed to parse packet header: {:?}", packet_header);
                    continue;
                }
                let header = packet_header.unwrap();

                let mut reorder = false;
                if let Some(ref _last_packet_header) = last_packet_header {
                    if _last_packet_header.source_ip != header.source_ip {
                    } else if _last_packet_header.seq_num < header.seq_num {
                        buffer.reorder(header.body.clone()); // TODO: remove clone
                        reorder = true;
                    }
                }

                if !reorder {
                    buffer.extend_from_slice(&header.body);
                }
                let metadata = PacketMetadata::from_buffer(buffer.get_remaining().to_vec());

                match metadata {
                    Err(err) => match err {
                        ParseResult::Incomplete => {
                            // warn!("Incomplete packet: {:?}", err);
                            last_packet_header = Some(header);
                        }
                        _ => {
                            warn!("Failed to parse metadata: {:?}", err);
                            buffer.clear();
                        }
                    },
                    Ok(metadata) => {
                        buffer.clear(); // TODO: adapt to other ranges
                                        // debug!("Parsed metadata: {:?}", metadata.id);
                        last_packet_header = None;
                        if PacketListener::_has_subscriptions(
                            &subscriptions.lock().unwrap(),
                            &metadata.id,
                        ) {
                            let mut parser = PacketParser::from_metadata(&metadata);
                            match parser.parse(&procol_manager.read().unwrap()) {
                                Ok(packet) => {
                                    PacketListener::_notify(
                                        &subscriptions.lock().unwrap(),
                                        &packet,
                                        &node,
                                    );
                                }
                                Err(err) => {
                                    warn!(
                                        "Failed to parse packet: {:?} for {:?}",
                                        err, metadata.id
                                    );
                                }
                            }
                        }
                    }
                };
            }
        });

        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum PacketListenerError {
    #[error("Failed to open device")]
    FailedToOpenDevice,
    #[error("Invalid capture device")]
    InvalidCaptureDevice,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn test_packet_listener() {
        let mut listener = PacketListener::new();

        assert_eq!(listener.subscriptions.lock().unwrap().len(), 0);

        let listener_id = "test";
        let listener_fn = |_event: &Packet, _: &Node| {};
        let event = 0;

        listener.subscribe(event.clone(), listener_id, listener_fn);
        assert_eq!(listener.subscriptions.lock().unwrap().len(), 1);
        assert_eq!(
            listener
                .subscriptions
                .lock()
                .unwrap()
                .get(&event)
                .unwrap()
                .len(),
            1
        );

        listener.unsubscribe(&event, listener_id);
        assert_eq!(listener.subscriptions.lock().unwrap().len(), 1);
        assert_eq!(
            listener
                .subscriptions
                .lock()
                .unwrap()
                .get(&event)
                .unwrap()
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn test_with_capture() {
        let cap = Capture::from_file("tests/fixtures/cap.pcap").unwrap();
        let path = "tests/fixtures/".to_string();
        let path = Path::new(&path);
        let node = Node::new(path, None, false).await;
        if let Err(err) = node {
            panic!("Failed to create node: {:?}", err);
        }
        let node = node.unwrap();
        let listener_fn = |event: &Packet, node: &Node| {
            let key = event.id.to_string();
            let mut store = node.store.lock().unwrap();
            match store.get(&key) {
                Some(count) => {
                    let count = count.parse::<u32>().unwrap();
                    let count = count + 1;
                    let count = count.to_string();
                    store.insert(key, count);
                }
                None => {
                    store.insert(key, "1".to_string());
                }
            }
        };

        let mut listener = node.packet_listener.lock().unwrap();
        let id = "test";
        listener.subscribe(1338, id, listener_fn);

        let res = listener.run_with_capture(cap.into());
        if let Err(err) = res {
            panic!("Failed to run with capture: {:?}", err);
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        info!("Store: {:?}", node.store.lock().unwrap());
    }
}
