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

use log::trace;

use std::io::{Error, ErrorKind, Result};
use std::net::SocketAddr;
use tokio::net::UdpSocket;

use once_cell::sync::OnceCell;
static RTP_TX: OnceCell<UdpSocket> = OnceCell::new();

/// init the UDP socket to send to DP
pub async fn dp_init(remote: SocketAddr) -> Result <()> {
    match UdpSocket::bind("127.0.0.1:12345").await {
        Ok(socket) => {
            match socket.connect(remote).await {
                Ok(()) => {
                    match RTP_TX.set(socket) {
                        Ok(()) => return Ok(()), 
                        _ => return Err(Error::new(ErrorKind::AlreadyExists, "gRPC OnceCell already set")),
                    }                    
                },
                Err(e) => return Err(e.into()),
            }
        },
        Err(e) => return Err(e.into()),
    }
}

/// Send packet over RTP
pub async fn dp_send(data: Vec<u8>) -> Result <usize> {
    match RTP_TX.get() {
        Some(socket) => {
            loop {
                match socket.writable().await {
                    Ok(()) => {

                        trace!("sending UDP data to DP");
                
                        match socket.try_send(&data) {
                            Ok(written) => return Ok(written),
                            Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue,
                            Err(e) => return Err(e),
                        }
                    },
                    Err(e) => return Err(e.into()),
                }
            }
        },
        None => return Err(Error::new(ErrorKind::NotFound, "OnceCell not initlialised")),
    }
}