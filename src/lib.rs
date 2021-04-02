use std::{net::IpAddr, str::FromStr};

use libp2p::{
    core::{connection::ListenerId, muxing::StreamMuxerBox, transport::Boxed, upgrade},
    floodsub::{Floodsub, FloodsubEvent, FloodsubMessage, Topic},
    identity,
    kad::{store::MemoryStore, Kademlia, KademliaEvent},
    mdns::{MdnsEvent, TokioMdns},
    mplex,
    noise::{self, NoiseConfig, X25519Spec},
    swarm::{NetworkBehaviourEventProcess, SwarmBuilder},
    tcp::TokioTcpConfig,
    Multiaddr, NetworkBehaviour, PeerId, Swarm, Transport,
};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt},
    sync::mpsc,
};

pub static RECIPES_TOPIC: Lazy<Topic> = Lazy::new(|| Topic::new("recipes"));

pub fn get_external_addr_from_stun() -> IpAddr {
    let (_socket, stun_addr) = stunclient::just_give_me_the_udp_socket_and_its_external_address();
    stun_addr.ip()
}

pub fn print_initial_message(ip_addr: &IpAddr, peer_id: &PeerId) {
    println!(
        "My IP address and peer ID:\n {}\n {}",
        ip_addr.to_string(),
        peer_id.to_base58()
    );
}

pub fn create_stdin_reader() -> tokio::io::Lines<impl AsyncBufRead> {
    tokio::io::BufReader::new(tokio::io::stdin()).lines()
}

pub fn create_transport(keys: &identity::Keypair) -> Boxed<(PeerId, StreamMuxerBox)> {
    let auth_keys = noise::Keypair::<X25519Spec>::new()
        .into_authentic(keys)
        .expect("can create auth keys");

    TokioTcpConfig::new()
        .upgrade(upgrade::Version::V1)
        .authenticate(NoiseConfig::xx(auth_keys).into_authenticated()) // XX Handshake pattern, IX exists as well and IK - only XX currently provides interop with other libp2p impls
        .multiplex(mplex::MplexConfig::new())
        .boxed()
}

pub fn create_kademlia(peer_id: &PeerId) -> Kademlia<MemoryStore> {
    let store = MemoryStore::new(peer_id.clone());
    Kademlia::new(peer_id.clone(), store)
}

pub fn create_mdns() -> TokioMdns {
    TokioMdns::new().expect("can create mdns")
}

pub fn create_floodsub(peer_id: &PeerId) -> Floodsub {
    Floodsub::new(peer_id.clone())
}

pub fn parse_peerid(peerid: &str) -> PeerId {
    PeerId::from_str(&peerid).expect("kedmlia peerid")
}

pub fn parse_multiaddr(addr: &str) -> Multiaddr {
    addr.parse().expect("kademlia address")
}

pub fn create_swarm(
    keys: &identity::Keypair,
    peer_id: &PeerId,
) -> (
    Swarm<MyNetworkBehaviour>,
    mpsc::UnboundedReceiver<ListResponse>,
) {
    let (tx, rx) = mpsc::unbounded_channel();

    let transport = create_transport(keys);
    let mut behaviour = MyNetworkBehaviour {
        kademlia: create_kademlia(peer_id),
        mdns: create_mdns(),
        floodsub: create_floodsub(peer_id),
        floodsub_publish_tx: tx,
    };

    behaviour.floodsub.subscribe(RECIPES_TOPIC.clone());

    let swarm = SwarmBuilder::new(transport, behaviour, peer_id.clone())
        .executor(Box::new(|fut| {
            tokio::spawn(fut);
        }))
        .build();
    (swarm, rx)
}

pub trait SwarmExt {
    fn my(&mut self) -> &mut MyNetworkBehaviour;
    fn listen_all_interfaces(&mut self) -> ListenerId;
    fn print_listening_addresses(&self);
}

impl SwarmExt for Swarm<MyNetworkBehaviour> {
    fn my(&mut self) -> &mut MyNetworkBehaviour {
        &mut *self
    }

    fn listen_all_interfaces(&mut self) -> ListenerId {
        Swarm::listen_on(
            self,
            "/ip4/0.0.0.0/tcp/0"
                .parse()
                .expect("can get a local socket"),
        )
        .expect("swarm listen on all interfaces")
    }

    fn print_listening_addresses(&self) {
        for addr in Swarm::listeners(self) {
            println!("Listening: {}", addr);
        }
    }
}

#[derive(NetworkBehaviour)]
pub struct MyNetworkBehaviour {
    pub kademlia: Kademlia<MemoryStore>,
    #[behaviour(ignore)]
    pub mdns: TokioMdns,
    pub floodsub: Floodsub,

    #[behaviour(ignore)]
    floodsub_publish_tx: mpsc::UnboundedSender<ListResponse>,
}

impl NetworkBehaviourEventProcess<KademliaEvent> for MyNetworkBehaviour {
    fn inject_event(&mut self, _event: KademliaEvent) {}
}

impl NetworkBehaviourEventProcess<MdnsEvent> for MyNetworkBehaviour {
    fn inject_event(&mut self, event: MdnsEvent) {
        match event {
            MdnsEvent::Discovered(discovered_list) => {
                for (peer, _addr) in discovered_list {
                    self.floodsub.add_node_to_partial_view(peer);
                }
            }
            MdnsEvent::Expired(expired_list) => {
                for (peer, _addr) in expired_list {
                    if !self.mdns.has_node(&peer) {
                        self.floodsub.remove_node_from_partial_view(&peer);
                    }
                }
            }
        }
    }
}

impl NetworkBehaviourEventProcess<FloodsubEvent> for MyNetworkBehaviour {
    fn inject_event(&mut self, event: FloodsubEvent) {
        match event {
            FloodsubEvent::Message(msg) => {
                for topic in msg.topics.iter() {
                    if RECIPES_TOPIC.eq(&topic) {
                        self.handle_topic_recipies(&msg);
                    }
                }
            }
            FloodsubEvent::Subscribed { .. } => {}
            FloodsubEvent::Unsubscribed { .. } => {}
        }
    }
}

impl MyNetworkBehaviour {
    pub fn list_all_recipies(&mut self) {
        let request = ListRequest {
            mode: ListMode::All,
        };
        let json = serde_json::to_string(&request).expect("request to json");
        self.floodsub
            .publish(RECIPES_TOPIC.clone(), json.as_bytes());
    }

    pub fn reply_list_response(&mut self, response: ListResponse) {
        let json = serde_json::to_string(&response).expect("response to json");
        self.floodsub
            .publish(RECIPES_TOPIC.clone(), json.as_bytes());
    }

    fn handle_topic_recipies(&mut self, msg: &FloodsubMessage) {
        if let Ok(request) = serde_json::from_slice::<ListRequest>(&msg.data) {
            let tx = self.floodsub_publish_tx.clone();

            match request.mode {
                ListMode::All => reply_public_recipies(tx, msg.source.to_string()),
                ListMode::One(_) => {}
            }
        } else if let Ok(response) = serde_json::from_slice::<ListResponse>(&msg.data) {
            println!("LIST RESPONSE: {:?}", response);
        }
    }
}

fn reply_public_recipies(tx: mpsc::UnboundedSender<ListResponse>, receiver: String) {
    let response = ListResponse {
        receiver,
        entries: vec!["foo".to_string()],
    };
    tx.send(response);
}

#[derive(Debug, Serialize, Deserialize)]
struct ListRequest {
    mode: ListMode,
}

#[derive(Debug, Serialize, Deserialize)]
enum ListMode {
    All,
    One(String),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListResponse {
    receiver: String,
    entries: Vec<String>,
}
