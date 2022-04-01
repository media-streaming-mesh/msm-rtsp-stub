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
use std::net::SocketAddr;

use tokio::sync::mpsc;
use tonic::Request;

use once_cell::sync::OnceCell;
static TXMT: OnceCell<mpsc::Sender<(Message, Option<mpsc::Sender::<String>>)>> = OnceCell::new();

/// Register client at CP
pub async fn cp_register(tx: mpsc::Sender::<String>, local_addr: String, remote_addr: String) -> Result<()> {
    let message = Message {
        event: Event::Add as i32,
        local: local_addr,
        remote: remote_addr,
        data: String::new(),
    };

    match TXMT.get().unwrap().send((message, Some(tx))).await {
        Ok(()) => return Ok(()),
        Err(e) => return Err(Error::new(ErrorKind::BrokenPipe, e.to_string())),
    }
}

/// De-register client from CP
pub async fn cp_deregister(local_addr: String, remote_addr: String) -> Result<()> {
    let message = Message {
        event: Event::Delete as i32,
        local: local_addr,
        remote: remote_addr,
        data: String::new(),
    };

    match TXMT.get().unwrap().send((message, None)).await {
        Ok(()) => return Ok(()),
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

    match TXMT.get().unwrap().send((message, None)).await {
        Ok(()) => return Ok(()),
        Err(e) => return Err(Error::new(ErrorKind::BrokenPipe, e.to_string())),
    }
}

/// CP connector
pub async fn cp_connector(socket: &str) -> Result<()> {

    let sock = socket.clone();

    // Connect to gRPC CP
    match MsmControlPlaneClient::connect(sock).await {

        Ok(handle) => {

            // Now create channel to receive messages from cp functions
            let (tx, rx) = mpsc::channel::<(Message, Option<mpsc::Sender::<String>>)>(5);

            // init the channel sender handle
            match TXMT.set(tx) {
                Ok(()) => {
                    let mut chans = HashMap::new();
                    let requests = async_stream::stream! {
                        loop {
                            let _result = match rx.recv().await {
                                Some((message, channel)) => {
                                    match Event::from_i32(message.event) {
                                        Some(Event::Add) => {
                                            // New flow created by client
                                            chans.insert(&format!("{}{}", message.local, message.remote), channel.unwrap());
                                        },
                                        Some(Event::Delete) => {
                                            // flow deleted by client
                                            chans.remove(&format!("{}{}", message.local, message.remote));
                                        },
                                        Some(Event::Data) => {},
                                        _ => {
                                            println!("invalid data");
                                            break;
                                        },
                                    }

                                    // send message to CP
                                    yield message;
                                },
                                None => {
                                    println!("error error");
                                    break;
                                },
                            };
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
                                                        // data from CP
                                                        match chans.get(&format!("{}{}", message.local, message.remote)) {
                                                            Some(channel) => {
                                                                match channel.send(message.data).await {
                                                                    Ok(()) => {},
                                                                    Err(e) => return Err(Error::new(ErrorKind::Other, e.to_string())),
                                                                }
                                                            },
                                                            _ => return Err(Error::new(ErrorKind::Other, "unknown client")),
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
                },
                Err(e) => return Err(Error::new(ErrorKind::AlreadyExists, "OnceCell already set")),
            }
        },
        Err(e) => return Err(Error::new(ErrorKind::NotConnected, e.to_string())),
    }
}