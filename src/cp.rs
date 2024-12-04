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

use async_stream::stream;

use crate::client::client_outbound;
use crate::dp::dp_init;

use log::{trace, debug, info, warn, error};

use prost::UnknownEnumValue;

use self::msm_cp::msm_control_plane_client::MsmControlPlaneClient;
use self::msm_cp::{Event, Message};

use std::collections::HashMap;
use std::fmt;
use std::io::{Error, ErrorKind, Result};
use std::net::SocketAddr;
use std::str::FromStr;

use tokio::sync::{mpsc, Mutex};
use tokio::time;
use tokio_util::sync::CancellationToken;
use tonic::transport::Channel;
use tonic::transport::Uri;
use tonic::Request;

use once_cell::sync::{Lazy, OnceCell};

// global mutable handle to channel that sends to GRPC thread
// Uses async-safe Tokio Mutex to control access to current sender channel
// will create a channel each time we (re-)connect to the control plane
static GRPC_TX: Lazy<Mutex<Option<mpsc::Sender<Message>>>> = Lazy::new(|| Mutex::new(None));

// global immutable handle to channel that sends to hashmap thread
// will be created once and shared by all users of the hashmap
static HASH_TX: OnceCell<mpsc::Sender<(HashmapCommand, String, Option<mpsc::Sender<Vec<u8>>>, Option<String>)>> = OnceCell::new();

// global immutable flag to indicate that DP is initialised
static DP_INIT: OnceCell<()> = OnceCell::new();

const CP_CHANNEL_SIZE: usize = 5;
const HASH_CHANNEL_SIZE: usize = 5;

#[derive(Debug)]
enum HashmapCommand {
    Insert,
    Remove,
    Send,
}

impl fmt::Display for HashmapCommand {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// write channel tx handle to GPRC mutex
async fn cp_grpc_handle_write(channel: mpsc::Sender<Message>) -> () {
    let mut guard = GRPC_TX.lock().await;
    *guard = Some(channel);
    return // will implicitly release the lock
}

/// read channel tx handle from GRPC mutex
async fn cp_grpc_handle_read() -> Result<mpsc::Sender<Message>> {
    match &*GRPC_TX.lock().await {
        Some(channel) => return Ok(channel.clone()),
        None => return Err(Error::new(ErrorKind::Other, "mutex not initialised")),
    }
}

/// Queue message to send to CP
pub async fn cp_send(message: Message) -> Result<()> {
    trace!("cp send");
    match cp_grpc_handle_read().await {
        Ok(channel) => {
            trace!("queueing for CP");

            match channel.send(message).await {
                Err(e) => return Err(Error::new(ErrorKind::Other, e.to_string())),
                Ok(_) => return Ok(()),
            }
        },
        Err(e) => return Err(e),
    }
}

/// Register stub at CP
pub async fn cp_register(pod_id: String) -> Result<()> {
    trace!("cp register");

    let message = Message {
        event: Event::Register as i32,
        local: String::new(),
        remote: String::new(),
        data: pod_id,
    };
        
    return cp_send(message).await
}

/// Add client to CP
pub async fn cp_add(tx: mpsc::Sender::<Vec<u8>>, local_addr: String, remote_addr: String) -> Result<()> {
    trace!("cp_add for {} {}", local_addr, remote_addr);
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
    trace!("cp delete for {} {}", local_addr, remote_addr);
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
    trace!("CP message from client {} {}, data {}", local_addr, remote_addr, message_string);
    let message = Message {
        event: Event::Data as i32,
        local: local_addr,
        remote: remote_addr,
        data: message_string,
    };

    return cp_send(message).await
}

/// hashmap owner
async fn cp_hashmap(mut chan_rx: mpsc::Receiver<(HashmapCommand, String, Option<mpsc::Sender<Vec<u8>>>, Option<String>)>) -> () {
    let mut channels = HashMap::<String, mpsc::Sender<Vec<u8>>>::new();

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
                                        trace!("Received from CP: {}", data.to_string());
                                        match value_ref.send(data.into_bytes()).await {
                                            Ok(()) => { debug!("sent CP data to channel") },
                                            Err(_e) => { warn!("unable to send CP data for key {}", key) },
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
async fn cp_access_hashmap(command: HashmapCommand, key: String, optional_channel: Option<mpsc::Sender<Vec<u8>>>, optional_data: Option<String>) -> Result<()> {
    match HASH_TX.get() {
        Some(channel) => {
            trace!("sending command {} to hashmap for key {}", command.to_string(), key);
            match channel.send((command, key, optional_channel, optional_data)).await {
                Ok(()) => return Ok(()),
                Err(e) => return Err(Error::new(ErrorKind::BrokenPipe, e.to_string())),
            }
        },
        None => return Err(Error::new(ErrorKind::NotFound, "hashmap handle not initlialised")),
    }
}

/// Add flow from CP
async fn cp_add_flow(remote_addr: String, cancellation_token: CancellationToken) -> Result<()> {
    trace!("CP add flow for {}", remote_addr);
    match client_outbound(remote_addr.clone(), cancellation_token).await {
        // connected to client so add it to CP
        Ok(()) => return Ok(()),
        Err(e) => return Err(e),
    }
}

/// Delete flow from CP
async fn cp_del_flow(key: String) -> Result<()> {
    return cp_access_hashmap(HashmapCommand::Remove, key, None, None).await;
}

/// Received data from CP
async fn cp_data_rcvd(key: String, data: String) -> Result<()> {
    debug!("Data {} received from CP for flow {}", data, key);
    return cp_access_hashmap(HashmapCommand::Send, key, None, Some(data)).await;
}

/// Run bidirectional streaming RPC
async fn cp_stream(handle: &mut MsmControlPlaneClient<Channel>, mut grpc_rx: mpsc::Receiver<Message>, cancellation_token: CancellationToken) -> Result<()> {

    let requests = stream! {
        loop {
            trace!("awaiting request for CP");
            match grpc_rx.recv().await {
                Some(message) => {
                    trace!("received {} messsage for CP", message.event);
                    yield(message)
                },
                None => {
                    error!("no message received for CP");
                    return
                },
            }
        }
    };
        
    match handle.send(Request::new(requests)).await {
        Ok(responses) => {
            let mut inbound = responses.into_inner();

            loop {
                trace!("awaiting message from CP");
                match inbound.message().await {
                    Ok(option) => {
                        trace!("received from CP");
                        match option {
                            Some(message) => {
                                trace!("Message {} received from CP", message.event);
                                match Event::try_from(message.event) {
                                    Ok(Event::Register) => {
                                        error!("register from CP!");
                                        return Err(Error::new(ErrorKind::InvalidInput, "Invalid register message from CP"));
                                    },
                                    Ok(Event::Config) => {
                                        match SocketAddr::from_str(&message.remote) {
                                            Ok(socket_addr) => {
                                                match DP_INIT.get() {
                                                    Some(()) => debug!("DP already initialised"),
                                                    None => {
                                                        match DP_INIT.set(()) {
                                                            Ok(()) => {
                                                                debug!("DP init flag set");
                                                                match dp_init(socket_addr).await {
                                                                    Ok(()) => debug!("Connected to DP {}", socket_addr),
                                                                    Err(e) => error!("Error connecting to DP: {}", e),
                                                                }
                                                            },
                                                            _ => error!("unable to set DP lock"),
                                                        }
                                                    }
                                                }
                                            },
                                            Err(e) => error!("Unable to parse CP config: {}", e),
                                        }
                                    },
                                    Ok(Event::Request) => {
                                        trace!("Request to add from CP");
                                        let flow_token = cancellation_token.clone();
                                        match cp_add_flow(message.remote, flow_token).await {
                                            Ok(()) => debug!("CP added flow"),
                                            Err(e) => return Err(e),
                                        }
                                    },
                                    Ok(Event::Add) => {
                                        trace!("add from CP!");
                                        return Err(Error::new(ErrorKind::InvalidInput, "Invalid add message from CP"));
                                    },
                                    Ok(Event::Delete) => {
                                        trace!("delete from CP");
                                        match cp_del_flow(format!("{}{}", message.local, message.remote)).await {
                                            Ok(()) => debug!("CP deleted flow"),
                                            Err(e) => return Err(e),
                                        }
                                    },
                                    Ok(Event::Data) => {
                                        trace!("data from CP");
                                        match cp_data_rcvd(format!("{} {}", message.local, message.remote), message.data).await {
                                            Ok(()) => debug!("data received from CP"),
                                            Err(e) => return Err(e),
                                        }
                                    },
                                    Err(UnknownEnumValue(e)) => return Err(Error::new(ErrorKind::Other, e.to_string())),
                                }
                            },
                            None => {
                                trace!("no message received from CP");
                                return Err(Error::new(ErrorKind::Other, "no message"))
                            },
                        }
                    },
                    Err(e) => {
                        trace!("unable to receive from CP");
                        return Err(Error::new(ErrorKind::Other, e.to_string()))
                    },
                }
            }
        },
        Err(e) => return Err(Error::new(ErrorKind::BrokenPipe, e.to_string())),
    }
}

/// CP connector
pub async fn cp_connector(uri: Uri, pod_id: String, cancellation_token: CancellationToken) -> Result<()> {

    // create channel to access hash-map entries
    let (hash_tx, hash_rx) = mpsc::channel::<(HashmapCommand, String, Option<mpsc::Sender<Vec<u8>>>, Option<String>)>(HASH_CHANNEL_SIZE);

    match HASH_TX.set(hash_tx) {
        Ok(()) => {

            // start the hash-map task
            let hashmap_token = cancellation_token.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = cp_hashmap(hash_rx) => {
                        info!("hashmap task finished");
                    }
                    _ = hashmap_token.cancelled() => {
                        info!("hashmap task cancelled");
                    }
                }
            });

            loop {
                // create channel to receive messages from CP functions
                let (grpc_tx, grpc_rx) = mpsc::channel::<Message>(CP_CHANNEL_SIZE);

                // write the tx handle into the mutex
                cp_grpc_handle_write(grpc_tx).await;

                debug!("connecting to gRPC CP");

                match MsmControlPlaneClient::connect(uri.clone()).await {
                    Ok(mut handle) => {

                        trace!("connected to gRPC CP");

                        // Now register the stub with the CP
                        match cp_register(pod_id.clone()).await {
                            Ok(()) => {
                                let stream_token = cancellation_token.clone();
                                tokio::select! {
                                    result = cp_stream(&mut handle, grpc_rx, stream_token) => {
                                        match result {
                                            Ok(()) => trace!("CP disconnected"),
                                            Err(e) => warn!("CP disconnected due to error {}", e.to_string()),
                                        }
                                    }
                                    _ = cancellation_token.cancelled() => {
                                        info!("CP task cancelled");
                                    }
                                }
                            },
                            Err(e) => return Err(e),
                        }
                    },
                    Err(e) => {
                        trace!("Unable to connect to control-plane - error {}", e.to_string());
                        time::sleep(time::Duration::from_secs(1)).await;
                    },
                }
            }
        },
        _ => return Err(Error::new(ErrorKind::AlreadyExists, "Hashmap handle already set")),
    }
}
