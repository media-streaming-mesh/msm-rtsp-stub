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
            match HASH_TX.get().unwrap().send((HashmapCommand::Insert, format!("{}{}", local_addr, remote_addr), Some(tx), None)).await {
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
            match HASH_TX.get().unwrap().send((HashmapCommand::Remove,  format!("{}{}", local_addr, remote_addr), None, None)).await {
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
                match channels.get(&key) {
                    Some(value_ref) => { 
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

/// Run bidirectional streaming RPC
async fn cp_stream(handle: &mut MsmControlPlaneClient<Channel>, mut grpc_rx: mpsc::Receiver<Message>) -> Result<()> {

    let requests = async_stream::stream! {
        loop {
            yield grpc_rx.recv().await.unwrap();
        }
    };
        
    match handle.send(Request::new(requests)).await {
        Ok(responses) => {
            let mut inbound = responses.into_inner();

            loop {
                match inbound.message().await {
                    Ok(option) => {
                        match option {
                            Some(message) => {
                                match Event::from_i32(message.event) {
                                    Some(Event::Add) => {
                                        // New flow created by CP
                                    },
                                    Some(Event::Delete) => {
                                        // CP deleting a flow
                                    },
                                    Some(Event::Data) => {
                                        // data from CP - send to client via the hashmap handler
                                        match HASH_TX.get().unwrap().send(( HashmapCommand::Send,
                                                                            format!("{}{}", message.local, message.remote),
                                                                            None,
                                                                            Some(message.data)  )).await {
                                            Ok(()) => return Ok(()),
                                            Err(e) => return Err(Error::new(ErrorKind::BrokenPipe, e.to_string())),
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