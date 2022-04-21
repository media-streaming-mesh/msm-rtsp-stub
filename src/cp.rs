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

use http::Uri;
use log::{debug, trace, warn, error};

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
const CHANNEL_SIZE: usize = 5;

enum HashmapCommand {
    Insert,
    Remove,
    Send,
}

/// Queue message to send to CP
pub async fn cp_send(message: Message) -> Result<()> {
    match GRPC_TX.get() {
        Some(channel) => {
            match channel.send(message).await {
                Ok(()) => return Ok(()),
                Err(e) => return Err(Error::new(ErrorKind::Other, e.to_string())),
            }
        },
        None => return Err(Error::new(ErrorKind::NotFound, "gRPC handle not initialised")),
    }
}

/// Register stub at CP
pub async fn cp_register() -> Result<()> {
    let message = Message {
        event: Event::Register as i32,
        local: String::new(),
        remote: String::new(),
        data: String::new(),
    };

    return cp_send(message).await
}

/// Add client to CP
pub async fn cp_add(tx: mpsc::Sender::<String>, local_addr: String, remote_addr: String) -> Result<()> {
    let message = Message {
        event: Event::Add as i32,
        local: local_addr.clone(),
        remote: remote_addr.clone(),
        data: String::new(),
    };

    match cp_send(message).await {
        Ok(()) => {
            match cp_access_hashmap(HashmapCommand::Insert, format!("{} {}", local_addr, remote_addr), Some(tx), None).await {
                Ok(()) => return Ok(()),
                Err(e) => return Err(Error::new(ErrorKind::BrokenPipe, e.to_string())),
            }
        },
        Err(e) => return Err(Error::new(ErrorKind::BrokenPipe, e.to_string())),
    }
}

/// Delete client from CP
pub async fn cp_delete(local_addr: String, remote_addr: String) -> Result<()> {
    let message = Message {
        event: Event::Delete as i32,
        local: local_addr.clone(),
        remote: remote_addr.clone(),
        data: String::new(),
    };

    match cp_send(message).await {
        Ok(()) => {
            match cp_access_hashmap(HashmapCommand::Remove, format!("{} {}", local_addr, remote_addr), None, None).await {
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

    return cp_send(message).await
}

/// hashmap owner
async fn cp_hashmap(mut chan_rx: mpsc::Receiver<(HashmapCommand, String, Option<mpsc::Sender<String>>, Option<String>)>) -> () {
    let mut channels = HashMap::<String, mpsc::Sender<String>>::new();

    loop {
        match chan_rx.recv().await {
            Some((command, key, optional_value, optional_data)) => {
                match command {
                    HashmapCommand::Insert => {
                        match optional_value {
                            Some(value) => {
                                match channels.insert(key.clone(), value) {
                                    Some(_value) => { warn!("key {} already present!", key) },
                                    None => { debug!("key {} added", key) },
                                }
                            },
                            None => {
                                error!("no value sent with hashmap insert");
                                return
                            },
                        }
                    },
                    HashmapCommand::Remove => {
                        match channels.remove(&key) {
                            Some(_value) => { debug!("key {} removed", key) },
                            None => { warn!("key {} not present!", key) },
                        }
                    },
                    HashmapCommand::Send => {
                        trace!("sending data to key {}", key);
                        match channels.get(&key) {
                            Some(value_ref) => { 
                                trace!("found channel for key {}",  key);
                                match optional_data {
                                    Some(data) => {
                                        match value_ref.send(data).await {
                                            Ok(()) => { debug!("sent data to channel") },
                                            Err(_e) => { warn!("unable to send data for key {}", key) },
                                        }
                                    },
                                    None => {
                                        error!("no data for hashmap send");
                                        return
                                    },
                                }
                            },
                            None => { warn!("key {} not present!", key) },
                        }
                    },
                }
            },
            None => {
                error!("hashmap channel closed!");
                return
            }
        }
    }
}

/// Send to hashmap owner
async fn cp_access_hashmap(command: HashmapCommand, key: String, optional_channel: Option<mpsc::Sender<String>>, optional_data: Option<String>) -> Result<()> {
    match HASH_TX.get() {
        Some(channel) => {
            trace!("sending command to hashmap for key {}", key);
            match channel.send((command, key, optional_channel, optional_data)).await {
                Ok(()) => return Ok(()),
                Err(e) => return Err(Error::new(ErrorKind::BrokenPipe, e.to_string())),
            }
        },
        None => return Err(Error::new(ErrorKind::NotFound, "hashmap handle not initlialised")),
    }
}

/// Add flow from CP
async fn cp_add_flow(remote_addr: String) -> Result<()> {
    // Create channel to send CP messages to client
    let (tx, rx) = mpsc::channel::<String>(CHANNEL_SIZE);

    match client_outbound(remote_addr.clone(), rx).await {
        // connected to client so add it to CP
        Ok(local_addr) => return cp_add(tx, local_addr, remote_addr).await,
        Err(e) => return Err(e),
    }
}

/// Delete flow from CP
async fn cp_del_flow(key: String) -> Result<()> {
    return cp_access_hashmap(HashmapCommand::Remove, key, None, None).await;
}

/// Received data from CP
async fn cp_data_rcvd(key: String, data: String) -> Result<()> {
    trace!("Data {} received from CP for flow {}", data, key);
    return cp_access_hashmap(HashmapCommand::Send, key, None, Some(data)).await;
}

/// Run bidirectional streaming RPC
async fn cp_stream(handle: &mut MsmControlPlaneClient<Channel>, mut grpc_rx: mpsc::Receiver<Message>) -> Result<()> {

    let requests = async_stream::stream! {
        loop {
            trace!("request for CP");
            match grpc_rx.recv().await {
                Some(message) => yield(message),
                None => {
                    error!("no message received from CP");
                    return
                },
            }
        }
    };
        
    match handle.send(Request::new(requests)).await {
        Ok(responses) => {
            let mut inbound = responses.into_inner();

            loop {
                match inbound.message().await {
                    Ok(option) => {
                        trace!("received from CP");
                        match option {
                            Some(message) => {
                                trace!("Message {} received from CP", message.event);
                                match Event::from_i32(message.event) {
                                    Some(Event::Register) => {
                                        error!("register from CP!");
                                        return Err(Error::new(ErrorKind::InvalidInput, "Invalid register message from CP"));
                                    },
                                    Some(Event::Add) => {
                                        trace!("add from CP");
                                        match cp_add_flow(message.remote).await {
                                            Ok(()) => debug!("CP added flow"),
                                            Err(e) => return Err(e),
                                        }
                                    },
                                    Some(Event::Delete) => {
                                        trace!("delete from CP");
                                        match cp_del_flow(format!("{}{}", message.local, message.remote)).await {
                                            Ok(()) => debug!("CP deleted flow"),
                                            Err(e) => return Err(e),
                                        }
                                    },
                                    Some(Event::Data) => {
                                        trace!("data from CP");
                                        match cp_data_rcvd(format!("{} {}", message.local, message.remote), message.data).await {
                                            Ok(()) => debug!("data received from CP"),
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
pub async fn cp_connector(uri: Uri) -> Result<()> {

    debug!("connecting to gRPC CP");
    
    // Connect to gRPC CP
    match MsmControlPlaneClient::connect(uri).await {

        Ok(mut handle) => {

            // Now create channel to receive messages from CP functions
            let (grpc_tx, grpc_rx) = mpsc::channel::<Message>(CHANNEL_SIZE);

            // init the channel sender handle
            match GRPC_TX.set(grpc_tx) {
                Ok(()) => {

                    // Now register the stub with the CP
                    match cp_register().await {
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
                        },
                        Err(e) => return Err(e),
                    }
                },
                _ => return Err(Error::new(ErrorKind::AlreadyExists, "gRPC OnceCell already set")),
            }
        },
        Err(e) => return Err(Error::new(ErrorKind::NotConnected, e.to_string())),
    }
}