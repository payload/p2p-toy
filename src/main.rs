#![allow(unused_imports)]
use kad::{Kademlia, KademliaEvent};
use libp2p::{NetworkBehaviour, PeerId, Transport, core::upgrade, floodsub::{Floodsub, FloodsubEvent, Topic}, futures::{future, task}, identity, kad, kad::store::MemoryStore, mdns::{MdnsEvent, TokioMdns}, mplex, noise::{Keypair, NoiseConfig, X25519Spec}, swarm::{NetworkBehaviour, NetworkBehaviourEventProcess, Swarm, SwarmBuilder}, tcp::TokioTcpConfig};
use log::{error, info};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use task::Poll;
use std::{collections::HashSet, str::FromStr, task::Context};
use tokio::{fs, io::AsyncBufReadExt, runtime::Runtime, sync::mpsc};
use libp2p::futures::StreamExt;

static KEYS: Lazy<identity::Keypair> = Lazy::new(|| identity::Keypair::generate_ed25519());
static PEER_ID: Lazy<PeerId> = Lazy::new(|| PeerId::from(KEYS.public()));
static TOPIC: Lazy<Topic> = Lazy::new(|| Topic::new("recipes"));

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync + 'static>>;
type Recipes = Vec<Recipe>;

#[derive(Debug, Serialize, Deserialize)]
struct Recipe {}

#[derive(Debug, Serialize, Deserialize)]
struct ListResponse {}

#[derive(NetworkBehaviour)]
struct RecipeBehaviour {
    kademlia: Kademlia<MemoryStore>,
    mdns: TokioMdns,
    #[behaviour(ignore)]
    response_sender: mpsc::UnboundedSender<ListResponse>,
}

impl NetworkBehaviourEventProcess<KademliaEvent> for RecipeBehaviour {
    fn inject_event(&mut self, event: KademliaEvent) {
        println!("kademlia event: {:?}", event);
        match event {
            KademliaEvent::QueryResult { id, result, stats } => {}
            KademliaEvent::RoutingUpdated {
                peer,
                addresses,
                old_peer,
            } => {}
            KademliaEvent::UnroutablePeer { peer } => {}
            KademliaEvent::RoutablePeer { peer, address } => {}
            KademliaEvent::PendingRoutablePeer { peer, address } => {}
        }
    }
}

impl NetworkBehaviourEventProcess<MdnsEvent> for RecipeBehaviour {
    fn inject_event(&mut self, event: MdnsEvent) {
        if true {
            return;
        }
        match event {
            MdnsEvent::Discovered(discovered_list) => {
                for (peer, addr) in discovered_list {
                    println!("discovered: {:?} {:?}", addr, peer);
                    // self.floodsub.add_node_to_partial_view(peer);
                }
            }
            MdnsEvent::Expired(expired_list) => {
                for (peer, addr) in expired_list {
                    if !self.mdns.has_node(&peer) {
                        println!("expired {:?} {:?}", addr, peer);
                        // self.floodsub.remove_node_from_partial_view(&peer);
                    }
                }
            }
        }
    }
}

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    println!("My peer id is:\n{}", PEER_ID.clone().to_base58());

    let kad_peerid = std::env::var("KAD_PEERID").ok();
    let kad_addr = std::env::var("KAD_ADDR").ok();

    let auth_keys = Keypair::<X25519Spec>::new()
        .into_authentic(&KEYS)
        .expect("can create auth keys");

    let transp = TokioTcpConfig::new()
        .upgrade(upgrade::Version::V1)
        .authenticate(NoiseConfig::xx(auth_keys).into_authenticated()) // XX Handshake pattern, IX exists as well and IK - only XX currently provides interop with other libp2p impls
        .multiplex(mplex::MplexConfig::new())
        .boxed();

    let (response_sender, mut response_rcv) = mpsc::unbounded_channel();

    let store = MemoryStore::new(PEER_ID.clone());
    let mut kademlia = kad::Kademlia::new(PEER_ID.clone(), store);

    if let Some((peerid, addr)) = kad_peerid.zip(kad_addr) {
        let peerid = PeerId::from_str(&peerid).expect("kademlia peerid");
        let addr = addr.parse().expect("kademlia address");
        let update = kademlia.add_address(&peerid, addr);
        /*let update = match update {
            RoutingUpdate::Success => "success",
            RoutingUpdate::Pending => "pending",
            RoutingUpdate::Failed => "failed",
        };
        println!("kademlia add_address. {}", update);*/
    }

    let behaviour = RecipeBehaviour {
        kademlia: kademlia,
        mdns: TokioMdns::new().expect("can create mdns"),
        response_sender,
    };

    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();

    let mut swarm = SwarmBuilder::new(transp, behaviour, PEER_ID.clone())
        .executor(Box::new(|fut| {
            tokio::spawn(fut);
        }))
        .build();

    let listenerId = Swarm::listen_on(
        &mut swarm,
        "/ip4/0.0.0.0/tcp/0"
            .parse()
            .expect("can get a local socket"),
    )
    .expect("swarm can be started");

    /*
    for addr in Swarm::listeners(&swarm) {
        println!("Listening: {}", addr);
    }
    */

    if let Some(peerid) = kad_peerid {
        let to_search = PeerId::from_str(&peerid).expect("yes yes");
        swarm.behaviour_mut().get_closest_peers(to_search);
    }

    let mut listening = false;
    let mut listening_fut = future::poll_fn(move |cx: &mut Context<'_>| {
        loop {
            match swarm.poll_next_unpin(cx) {
                Poll::Ready(Some(event)) => println!("{:?}", event),
                Poll::Ready(None) => return Poll::Ready(()),
                Poll::Pending => {
                    if !listening {
                        for addr in Swarm::listeners(&swarm) {
                            println!("Listening on {}", addr);
                            listening = true;
                        }
                    }
                    return Poll::Pending
                }
            }
        }
    });
    // let moo = &mut listenting_fut;

    loop {
        let evt = {
            tokio::select! {
                line = stdin.next_line() => Some(EventType::Input(line.expect("can get line").expect("can read line from stdin"))),
                _ = &mut listening_fut => None,
                // response = response_rcv.recv() => Some(EventType::Response(response.expect("response exists"))),
            }
        };

        if let Some(evt) = evt {
            match evt {
                EventType::Input(string) => {
                    if let Some(kademlia_entry) = string.strip_prefix("kademlia ") {
                        //let mut split = kademlia_entry.split(':');
                        //let [ peerid, addr ] = kademlia_entry.split(':').take(2).collect();
                        //let peerid = split.next().expect("kademlia peerid:addr");
                        //let addr = split.next().expect("kademlia peerid:addr");
                        // TODO kademlia add_addr
                    }
                }
            }
        }
    }
}

enum EventType {
    Input(String),
}
