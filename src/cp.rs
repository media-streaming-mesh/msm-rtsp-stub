/*
 * Copyright (c) 2022 Cisco and/or its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at:
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

pub mod msm_cp {
    tonic::include_proto!("msm_cp");
}

use crate::client::client_outbound;

use self::msm_cp::msm_control_plane_client::MsmControlPlaneClient;
use self::msm_cp::{Event, Message};

use std::collections::HashMap;
use std::io::{Error, ErrorKind, Result};

use tokio::sync::mpsc;
use tonic::transport::Channel;
use tonic::Request;

use once_cell::sync::OnceCell;
static GRPC_TX: OnceCell<mpsc::Sender<Message>> = OnceCell::new();
static HASH_TX: OnceCell<mpsc::Sender<(HashmapCommand, String, Option<mpsc::Sender<String>>, Option<String>)>> = OnceCell::new();

enum HashmapCommand {
    Insert,
    Remove,
    Send,
}

/// Register client at CP
pub async fn cp_register(tx: mpsc::Sender::<String>, local_addr: String, remote_addr: String) -> Result<()> {
    let message = Message {
        event: Event::Add as i32,
        local: local_addr.clone(),
        remote: remote_addr.clone(),
        data: String::new(),
    };

    match GRPC_TX.get().unwrap().send(message).await {
        Ok(()) => {
            match HASH_TX.get().unwrap().send((HashmapCommand::Insert, format!("{} {}", local_addr, remote_addr), Some(tx), None)).await {
                Ok(()) => return Ok(()),
                Err(e) => return Err(Error::new(ErrorKind::BrokenPipe, e.to_string())),
            }
        },
        Err(e) => return Err(Error::new(ErrorKind::BrokenPipe, e.to_string())),
    }
}

/// De-register client from CP
pub async fn cp_deregister(local_addr: String, remote_addr: String) -> Result<()> {
    let message = Message {
        event: Event::Delete as i32,
        local: local_addr.clone(),
        remote: remote_addr.clone(),
        data: String::new(),
    };

    match GRPC_TX.get().unwrap().send(message).await {
        Ok(()) => {
            match HASH_TX.get().unwrap().send((HashmapCommand::Remove,  format!("{} {}", local_addr, remote_addr), None, None)).await {
                Ok(()) => return Ok(()),
                Err(e) => return Err(Error::new(ErrorKind::BrokenPipe, e.to_string())),
            }
        },  
        Err(e) => return Err(Error::new(ErrorKind::BrokenPipe, e.to_string())),
    }
}

/// Send data message to CP
pub async fn cp_data(local_addr: String, remote_addr: String, message_string: String) -> Result<()> {
    let message = Message {
        event: Event::Data as i32,
        local: local_addr,
        remote: remote_addr,
        data: message_string,
    };

    match GRPC_TX.get().unwrap().send(message).await {
        Ok(()) => return Ok(()),
        Err(e) => return Err(Error::new(ErrorKind::BrokenPipe, e.to_string())),
    }
}

/// hashmap owner
async fn cp_hashmap(mut chan_rx: mpsc::Receiver<(HashmapCommand, String, Option<mpsc::Sender<String>>, Option<String>)>) -> () {
    let mut channels = HashMap::<String, mpsc::Sender<String>>::new();

    loop {
        let (command, key, optional_value, optional_data) = chan_rx.recv().await.unwrap();
        println!("hashmap key is {}", key);
        match command {
            HashmapCommand::Insert => {
                match channels.insert(key.clone(), optional_value.unwrap()) {
                    Some(_value) => { println!("key {} already present!", key) },
                    None => { println!("key {} added", key) },
                }
            },
            HashmapCommand::Remove => {
                match channels.remove(&key) {
                    Some(_value) => { println!("key {} removed", key) },
                    None => { println!("key {} not present!", key) },
                }
            },
            HashmapCommand::Send => {
                println!("sending data to key {}", key);
                match channels.get(&key) {
                    Some(value_ref) => { 
                        println!("found channel for key {}",  key);
                        match value_ref.send(optional_data.unwrap()).await {
                            Ok(()) => {},
                            Err(_e) => { println!("unable to send data for key {}", key) },
                        }
                    },
                    None => { println!("key {} not present!", key) },
                }
            },
        }
    }
}

/// Send to hashmap owner
async fn cp_access_hashmap(command: HashmapCommand, key: String, optional_channel: Option<mpsc::Sender<String>>, optional_data: Option<String>) -> Result<()> {
    match HASH_TX.get() {
        Some(channel) => {
            println!("sending command to hashmap for key {}", key);
            match channel.send((command, key, optional_channel, optional_data)).await {
                Ok(()) => return Ok(()),
                Err(e) => return Err(Error::new(ErrorKind::BrokenPipe, e.to_string())),
            }
        },
        None => return Err(Error::new(ErrorKind::NotFound, "OnceCell not initlialised")),
    }
}

/// Add flow from CP
async fn cp_add_flow(local_addr: String, remote_addr: String) -> Result<()> {
    // Create channel to send CP messages to client
    let (tx, rx) = mpsc::channel::<String>(5);

    match client_outbound(local_addr.clone(), remote_addr.clone(), rx).await {
        Ok(()) => return cp_access_hashmap(HashmapCommand::Insert, format!("{} {}", local_addr.clone(), remote_addr.clone()), Some(tx.clone()), None).await,
        Err(e) => return Err(e),
    }
}

/// Delete flow from CP
async fn cp_del_flow(key: String) -> Result<()> {
    return cp_access_hashmap(HashmapCommand::Remove, key, None, None).await;
}

/// Received data from CP
async fn cp_data_rcvd(key: String, data: String) -> Result<()> {
    println!("Data {} received from CP for flow {}", data, key);
    return cp_access_hashmap(HashmapCommand::Send, key, None, Some(data)).await;
}

/// Run bidirectional streaming RPC
async fn cp_stream(handle: &mut MsmControlPlaneClient<Channel>, mut grpc_rx: mpsc::Receiver<Message>) -> Result<()> {

    let requests = async_stream::stream! {
        loop {
            println!("request for CP");
            yield grpc_rx.recv().await.unwrap();
        }
    };
        
    match handle.send(Request::new(requests)).await {
        Ok(responses) => {
            let mut inbound = responses.into_inner();

            loop {
                match inbound.message().await {
                    Ok(option) => {
                        println!("received from CP");
                        match option {
                            Some(message) => {
                                println!("Message {} received from CP", message.event);
                                match Event::from_i32(message.event) {
                                    Some(Event::Add) => {
                                        println!("add from CP");
                                        match cp_add_flow(message.local, message.remote).await {
                                            Ok(()) => println!("CP added flow"),
                                            Err(e) => return Err(e),
                                        }
                                    },
                                    Some(Event::Delete) => {
                                        println!("delete from CP");
                                        match cp_del_flow(format!("{}{}", message.local, message.remote)).await {
                                            Ok(()) => println!("CP deleted flow"),
                                            Err(e) => return Err(e),
                                        }
                                    },
                                    Some(Event::Data) => {
                                        println!("data from CP");
                                        match cp_data_rcvd(format!("{} {}", message.local, message.remote), message.data).await {
                                            Ok(()) => println!("data received from CP"),
                                            Err(e) => return Err(e),
                                        }
                                    },
                                    None => return Err(Error::new(ErrorKind::InvalidInput, "Invalid gRPC operation")),
                                }
                            },
                            None => return Err(Error::new(ErrorKind::Other, "no message")),
                        }
                    },
                    Err(e) => return Err(Error::new(ErrorKind::Other, e.to_string())),
                }
            }
        },
        Err(e) => return Err(Error::new(ErrorKind::BrokenPipe, e.to_string())),
    }
}

/// CP connector
pub async fn cp_connector(socket: String) -> Result<()> {

    // Connect to gRPC CP
    match MsmControlPlaneClient::connect(socket).await {

        Ok(mut handle) => {

            // Now create channel to receive messages from CP functions
            let (grpc_tx, grpc_rx) = mpsc::channel::<Message>(5);

            // init the channel sender handle
            match GRPC_TX.set(grpc_tx) {
                Ok(()) => {

                    // create channel to access hash-map entries
                    let (hash_tx, hash_rx) = mpsc::channel::<(HashmapCommand, String, Option<mpsc::Sender<String>>, Option<String>)>(1);

                    match HASH_TX.set(hash_tx) {
                        Ok(()) => {
                            // start the hash-map task
                            tokio::spawn(async move { cp_hashmap(hash_rx).await });

                            // now start handling messages
                            return cp_stream(&mut handle, grpc_rx).await
                        },
                        _ => return Err(Error::new(ErrorKind::AlreadyExists, "Hashmap OnceCell already set")),
                    }
                }
                _ => return Err(Error::new(ErrorKind::AlreadyExists, "gRPC OnceCell already set")),
            }
        },
        Err(e) => return Err(Error::new(ErrorKind::NotConnected, e.to_string())),
    }
}