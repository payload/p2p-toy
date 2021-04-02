#![allow(unused_imports)]
use std::{
    collections::HashSet,
    task::{Context, Poll},
};

use ansi_term::Color;
use libp2p::{
    floodsub::Topic,
    futures::{future, StreamExt},
    identity,
    multiaddr::Protocol,
    swarm::AddressScore,
    Multiaddr, PeerId, Swarm,
};
use log::info;
use once_cell::sync::Lazy;
use p2p_toy::{
    create_kademlia, create_stdin_reader, create_swarm, create_transport,
    get_external_addr_from_stun, print_initial_message, ListResponse, MyNetworkBehaviour, SwarmExt,
    RECIPES_TOPIC,
};

mod behaviours;

static KEYS: Lazy<identity::Keypair> = Lazy::new(|| identity::Keypair::generate_ed25519());
static PEER_ID: Lazy<PeerId> = Lazy::new(|| PeerId::from(KEYS.public()));

#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    let ip_addr = get_external_addr_from_stun();
    print_initial_message(&ip_addr, &PEER_ID);
    let (mut swarm, mut rx) = create_swarm(&KEYS, &PEER_ID);
    swarm.listen_all_interfaces();

    let mut stdin = create_stdin_reader();

    loop {
        use MainEvent::*;
        let event = tokio::select! {
            event = swarm.next() => SwarmEvent(event),
            Some(response) = rx.next() => Response(response),
            line = stdin.next_line() => Input(line.expect("can get line").expect("can read line from stdin")),
        };

        match event {
            SwarmEvent(event) => println!("swarm event"),
            Response(response) => swarm.reply_list_response(response),
            Input(line) => handle_input(&mut swarm, line).await,
        }
    }
}

enum MainEvent {
    SwarmEvent(()),
    Response(ListResponse),
    Input(String),
}

type MySwarm = Swarm<MyNetworkBehaviour>;

async fn handle_input(swarm: &mut MySwarm, line: String) {
    match line.as_str() {
        "ls p" => handle_list_peers(swarm).await,
        cmd if cmd.starts_with("ls r") => handle_list_recipies(cmd, swarm).await,
        cmd if cmd.starts_with("create r") => handle_create_recipe(cmd).await,
        cmd if cmd.starts_with("publish r") => handle_publish_recipe(cmd).await,
        cmd if cmd.starts_with("dial ") => handle_dial(cmd, swarm).await,
        cmd if cmd.starts_with("listen ") => handle_listen_on(cmd, swarm).await,
        _ => (),
    };
}

async fn handle_list_peers(swarm: &mut MySwarm) {
    let h1 = |string| println!("{}", Color::Blue.bold().paint(string));

    h1("Discovered peers:");
    let my: &mut MyNetworkBehaviour = &mut *swarm;
    let nodes: HashSet<_> = my.mdns.discovered_nodes().collect();
    for node in nodes {
        println!(" {}", node);
    }

    h1("External addresses:");
    for addr in Swarm::external_addresses(swarm) {
        println!(" {:?}", addr);
    }

    h1("Number of peers:");
    let net = Swarm::network_info(swarm);
    println!(" {:?}", net.num_peers());

    h1("Listening on:");
    for addr in Swarm::listeners(swarm) {
        println!(" {:?}", addr);
    }
}

async fn handle_list_recipies(cmd: &str, swarm: &mut MySwarm) {
    let my: &mut MyNetworkBehaviour = &mut *swarm;
    my.list_all_recipies();
}

async fn handle_create_recipe(cmd: &str) {}

async fn handle_publish_recipe(cmd: &str) {}

async fn handle_dial(cmd: &str, swarm: &mut MySwarm) {
    let mut args = cmd.split_ascii_whitespace().skip(1);
    if let Some(ip_port) = args.next() {
        let mut split = ip_port.split(':');
        let ip = split.next().expect("ip");
        let port = split.next().expect("port");
        let addr = format!("/ip4/{}/tcp/{}", ip, port)
            .parse()
            .expect("parse multiaddr");
        let result = Swarm::dial_addr(swarm, addr);
        println!("dial {:?}", result);
    }
}

async fn handle_listen_on(cmd: &str, swarm: &mut MySwarm) {
    let mut args = cmd.split_ascii_whitespace().skip(1);
    if let Some(port) = args.next() {
        let addr = format!("/ip4/0.0.0.0/tcp/{}", port)
            .parse()
            .expect("parse multiaddr");
        let result = Swarm::listen_on(swarm, addr);
        println!("listen {:?}", result);
    }
}
