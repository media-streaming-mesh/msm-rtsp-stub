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

use async_recursion::async_recursion;

use log::{debug, trace, warn};

use std::io::{Error, ErrorKind, Result};
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};

use once_cell::sync::Lazy;

// global mutable handles to UDP sockets to data plane
// Uses async-safe Tokio Mutex to control access to current socket
// will create sockets each time we (re-)connect to the data plane
static RTP_TX: Lazy<Mutex<Option<UdpSocket>>> = Lazy::new(|| Mutex::new(None));
static RTCP_TX: Lazy<Mutex<Option<UdpSocket>>> = Lazy::new(|| Mutex::new(None));

const RTSP_MAGIC_FLAG: u8 = 0x24;

/// write channel tx handle to RTP mutex
async fn dp_rtp_handle_write(socket: UdpSocket) -> () {
    let mut guard = RTP_TX.lock().await;
    *guard = Some(socket);
    return // will implicitly release the lock
}

/// read channel tx handle from RTP mutex
async fn dp_rtp_handle_read() -> Result<&'static UdpSocket> {
    match *RTP_TX.lock().await {
        Some(socket) => return Ok(&socket),
        None => return Err(Error::new(ErrorKind::Other, "RTP mutex not initialised")),
    }
}
/// write channel tx handle to RTCP mutex
async fn dp_rtcp_handle_write(socket: UdpSocket) -> () {
    let mut guard = RTCP_TX.lock().await;
    *guard = Some(socket);
    return // will implicitly release the lock
}

/// read channel tx handle from RTCP mutex
async fn dp_rtcp_handle_read() -> Result<&'static UdpSocket> {
    match *RTCP_TX.lock().await {
        Some(socket) => return Ok(&socket),
        None => return Err(Error::new(ErrorKind::Other, "RTCP mutex not initialised")),
    }
}

/// init the UDP sockets to send to DP
pub async fn dp_init(proxy_rtp: SocketAddr) -> Result <()> {

    trace!("RTP proxy is {}", proxy_rtp);

    let rtp_port = envmnt::get_u16("LOCAL_RTP_PORT", 8050);
    let rtcp_port = rtp_port + 1;

    match UdpSocket::bind("0.0.0.0:".to_owned() + &rtp_port.to_string()).await {
        Ok(socket) => {
            trace!("bound RTP listen socket");
            match socket.connect(proxy_rtp).await {
                Ok(()) => {
                    trace!("connected to RTP proxy");
                    dp_rtp_handle_write(socket).await;            
                },
                Err(e) => return Err(e.into()),
            }
        },
        Err(e) => return Err(e.into()),
    }

    let proxy_rtcp = SocketAddr::new(proxy_rtp.ip(), proxy_rtp.port()+1);

    trace!("RTCP proxy is {}", proxy_rtcp);

    match UdpSocket::bind("0.0.0.0:".to_owned() + &rtcp_port.to_string()).await {
        Ok(socket) => {
            trace!("bound RTCP listen socket");
            match socket.connect(proxy_rtcp).await {
                Ok(()) => {
                    trace!("connected to RTCP proxy");
                    dp_rtcp_handle_write(socket).await;
                },
                Err(e) => return Err(e.into()),
            }
        },
        Err(e) => return Err(e.into()),
    }

    return Ok(())
}

/// demux interleaved data
#[async_recursion]
pub async fn dp_demux(length: usize, data: &mut [u8]) -> Result <(bool, usize, Option<&mut [u8]>)> {
    if data[0] != RTSP_MAGIC_FLAG {
        // this is CP data
        return Ok((false, 0, Some(data)))
    }

    if length < 4 {
        return Err(Error::new(ErrorKind::InvalidData, "Interleaved data too short"))
    }
    
    let channel: usize = data[1].into();
    let length_inside: usize = ((data[2] as u16) << 8 | data[3] as u16).into();
    
    trace!("Channel is {}", channel);
    trace!("Length is {}", length);
    trace!("Length inside is {}", length_inside);

    if length < length_inside + 4 {
        // this is a fragment of DP data
        debug!("Remaining buffer is {} bytes, length inside is {} bytes (channel {})", length, length_inside, channel);
        return Ok((true, 0, Some(data)))
    }

    // Send first (or only) RTP/RTCP data block 
    match dp_send(data[4..length_inside+4].to_vec(), channel).await {
        Ok(written) => {
            trace!("wrote {} bytes to DP", written);
            let next = written + 4;
            let left = length - next;
            if left > 0 {
                trace!("recursing as {} bytes left in buffer", left);
                // recursive call to demux will handle any remaining RTP/RTCP data blocks
                match dp_demux(left, &mut data[next..]).await {
                    Ok((fragment, wrote, opt_rem_data)) => return Ok((fragment, wrote+written, opt_rem_data)),
                    Err(e) => return Err(Error::new(ErrorKind::Other, e.to_string())),  
                }
            }
            else {
                // this was the final DP message in the buffer, and buffer is now empty
                return Ok((false, written, None))
            }     
        },
        Err(e) => return Err(e),
    }
}

/// Send RTP/RTCP UDP packet to the DP
pub async fn dp_send(data:Vec<u8>, channel: usize) -> Result <usize> {
    if channel == 0 {
        match dp_rtp_handle_read().await {
            Ok(socket) => {
                loop {
                    match socket.writable().await {
                        Ok(()) => {
    
                            trace!("sending RTP data to DP");
                    
                            match socket.try_send(&data) {
                                Ok(written) => {
                                    trace!("{} RTP bytes written", written);
                                    return Ok(written)
                                },
                                Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue,
                                Err(e) => {
                                    trace!("unable to send UDP");
                                    return Err(e)
                                }
                            }
                        },
                        Err(e) => return Err(e.into()),
                    }
                }
            },
            Err(e) => return Err(e),
        }
    }
    else {
        match dp_rtcp_handle_read().await {
            Ok(socket) => {
                loop {
                    match socket.writable().await {
                        Ok(()) => {
    
                            trace!("sending RTCP data to DP");
                    
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
            Err(e) => return Err(e),
        }
    }
}

pub async fn dp_rtp_recv(tx: mpsc::Sender::<Vec<u8>>) -> Result<usize> {
    match dp_rtp_handle_read().await {
        Ok(socket) => {
            let mut len = 0;
            loop {
                let mut buf = [0u8; 65536];
                trace!("attempting receive from RTP socket");
                match socket.recv(&mut buf[4..]).await {
                    Ok (rcvd) => {
                        trace!("{} bytes of RTP data received", rcvd);
                        len += rcvd;
                        buf[0] = RTSP_MAGIC_FLAG;
                        buf[1] = 0;
                        buf[2] = (rcvd as u16 >> 8) as u8;
                        buf[3] = rcvd as u8;
                        match tx.send((&buf[0..rcvd+4]).to_vec()).await {
                            Ok(()) => debug!("sent RTP data to client"),
                            Err(e) => warn!("unable to send RTP data, error{}",  e.to_string()),
                        }
                    },
                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue, // try again
                    Err(ref e) if e.kind() == ErrorKind::TimedOut => break,
                    Err(e) => return Err(e),
                }
            }
            return Ok(len)
        },
        Err(e) => return Err(e),
    }
}

pub async fn dp_rtcp_recv(tx: mpsc::Sender::<Vec<u8>>) -> Result<usize> {
    match dp_rtcp_handle_read().await {
        Ok(socket) => {
            let mut len = 0;
            loop {
                let mut buf = [0u8; 65536];
                trace!("attempting receive from RTCP socket");
                match socket.recv(&mut buf[4..]).await {
                    Ok (rcvd) => {
                        trace!("{} bytes of RTCP data received", rcvd);
                        len += rcvd;
                        buf[0] = RTSP_MAGIC_FLAG;
                        buf[1] = 1;
                        buf[2] = (rcvd as u16 >> 8) as u8;
                        buf[3] = rcvd as u8;
                        match tx.send((&buf[0..rcvd+4]).to_vec()).await {
                            Ok(()) => debug!("sent RTCP data to client"),
                            Err(e) => warn!("unable to send RTCP data, error{}",  e.to_string()),
                        }
                    },
                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue, // try again
                    Err(ref e) if e.kind() == ErrorKind::TimedOut => break,
                    Err(e) => return Err(e),
                }
            }
            return Ok(len)
        },
        Err(e) => return Err(e),
    }
}