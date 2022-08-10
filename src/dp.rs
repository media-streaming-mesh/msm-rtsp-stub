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
static RTCP_TX: OnceCell<UdpSocket> = OnceCell::new();

/// init the UDP sockets to send to DP
pub async fn dp_init(proxy_rtp: SocketAddr) -> Result <()> {
    match UdpSocket::bind("127.0.0.1:8050").await {
        Ok(socket) => {
            match socket.connect(proxy_rtp).await {
                Ok(()) => {
                    match RTP_TX.set(socket) {
                        Ok(()) => trace!("Created RTP DP socket"), 
                        _ => return Err(Error::new(ErrorKind::AlreadyExists, "RTP OnceCell already set")),
                    }                    
                },
                Err(e) => return Err(e.into()),
            }
        },
        Err(e) => return Err(e.into()),
    }

    let proxy_rtcp = SocketAddr::new(proxy_rtp.ip(), proxy_rtp.port()+1);

    match UdpSocket::bind("127.0.0.1:8051").await {
        Ok(socket) => {
            match socket.connect(proxy_rtcp).await {
                Ok(()) => {
                    match RTCP_TX.set(socket) {
                        Ok(()) => {
                            trace!("Created RTCP DP socket");
                            return Ok(())
                        },
                        _ => return Err(Error::new(ErrorKind::AlreadyExists, "RTP OnceCell already set")),
                    }
                },
                Err(e) => return Err(e.into()),
            }
        },
        Err(e) => return Err(e.into()),
    }
}

/// Demux DP packet
pub async fn dp_demux(data: Vec<u8>) -> Result <usize> {
    if data.len() > 3 {
        let channel = data[0].into();
        let length = ((data[1] as u16) << 8 | data[2] as u16).into();
        if data.len() == length + 3 {
            return dp_send(data[3..length].to_vec(), channel).await
        }
        else
        {
            return Err(Error::new(ErrorKind::InvalidData, "Incorrect length in interleaved data"))
        }
    }
    else {
        return Err(Error::new(ErrorKind::InvalidData, "Interleaved data too short"))
    }
}

/// Send packet over RTP
pub async fn dp_send(data:Vec<u8>, channel: usize) -> Result <usize> {
    if channel == 0 {
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
    else {
        match RTCP_TX.get() {
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
}