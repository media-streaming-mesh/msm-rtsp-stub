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

use crate::cp::cp_add;
use crate::cp::cp_delete;
use crate::cp::cp_data;
use crate::dp::dp_demux;
use crate::dp::dp_rtp_recv;
use crate::dp::dp_rtcp_recv;

use log::{debug, error, info, trace};

use std::io::{Error, ErrorKind, Result};

use tokio::net::{TcpListener, TcpStream};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;

const CLIENT_CHANNEL_SIZE: usize = 5;

/// read command from client
async fn client_read(reader: &OwnedReadHalf) -> Result<(bool, usize, Vec<u8>)> {
    loop {
        // wait until we can read from the stream
        match reader.readable().await {
            Ok(()) => {
                let mut buf = [0u8; 65536];
        
                match reader.try_read(&mut buf) {
                    Ok(0) => return Err(Error::new(ErrorKind::ConnectionReset,"client closed connection")),
                    Ok(bytes_read) => return Ok(((buf[0] == 0x24), bytes_read, buf[..bytes_read].to_vec())),
                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue, // try again
                    Err(e) => return Err(e.into()),
                }
            },
            Err(e) => {
                error!("reader didn't become readable");
                return Err(e.into())
            },
        }
    }
}

/// reflect back to client
async fn client_write(writer: &OwnedWriteHalf, response: Vec<u8>) -> Result<usize> {
    trace!("writing {} bytes to client", response.len());
    loop {
        match writer.writable().await {
            Ok(()) => {
                match writer.try_write(&response) {
                    Ok(bytes) => {
                        debug!("{} bytes written", bytes);
                        return Ok(bytes)
                    },
                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue, // try again
                    Err(e) => return Err(e.into()),
                }
            },
            Err(e) => {
                error!("writer didn't become writeable");
                return Err(e.into())
            },
        }
    }
}

/// handle messages for client
async fn client_writer(mut rx: mpsc::Receiver<Vec<u8>>, writer: OwnedWriteHalf) -> Result<usize> {
    let mut written_back = 0;

    while let Some(message) = rx.recv().await {
        trace!("received {} bytes for client", message.len());
        match client_write(&writer, message).await {
            Ok(bytes) => written_back += bytes,
            Err(ref e) if e.kind() == ErrorKind::ConnectionReset => break,
            Err(e) => return Err(e.into()),
        }
        trace!("about to loop again");
    }

    return Ok(written_back);
}

/// handle client connection
async fn client_handler(local_addr: String, remote_addr: String, client_stream: TcpStream) -> Result<()> {

    trace!("client handler for {} {}", local_addr, remote_addr);

    // Need socket to flush messages immediately 
    match client_stream.set_nodelay(true) {
        Ok(()) => {

            trace!("nodelay set for client");

            // split socket into sender/receiver so can hand sender to separate thread
            let (reader, writer) = client_stream.into_split();

            // Create channel to receive messages for client
            let (tx, rx) = mpsc::channel::<Vec<u8>>(CLIENT_CHANNEL_SIZE);

            match cp_add(tx.clone(), local_addr.clone(), remote_addr.clone()).await {
                Ok(()) => {
                    let mut handles = vec![];
                    let rtp_tx = tx.clone();
                    let rtcp_tx = tx.clone();

                    // Spawn thread to receive messages and send to client
                    handles.push(tokio::spawn(async move {
                        trace!("spawning thread to send messages to client");
                        match client_writer(rx, writer).await {
                            Ok(written) => {
                                debug!("Disconnected: wrote total of {} bytes back to client", written);
                            },
                            Err(e) => error!("Error: {}", e),
                        }
                    }));

                    // need to listen for RTP/RTCP messages
                    handles.push(tokio::spawn(async move {
                        trace!("spawning thread for RTP receive");
                        match dp_rtp_recv(rtp_tx).await {
                            Ok(written) => info!("{} RTP bytes read", written),
                            Err(e) => debug!("RTP read error {}", e.to_string()),
                        }
                    }));

                    handles.push(tokio::spawn(async move {
                        trace!("spawning thread for RTCP receive");
                        match dp_rtcp_recv(rtcp_tx).await {
                            Ok(written) => info!("{} RTCP bytes read", written),
                            Err(e) => debug!("RTCP read error {}", e.to_string()),
                        }
                    }));

                    // now receive client messages until disconnect
                    loop {
                        match client_read(&reader).await {
                            Ok((interleaved, length, data)) => {
                                if interleaved {
                                    trace!("Sending data to DP");
                                    match dp_demux(length, data).await {
                                        Ok(written) => trace!("Sent {} bytes to DP", written),
                                        Err(e) => return Err(Error::new(ErrorKind::Other, e.to_string())),
                                    }
                                }
                                else {
                                    // this is control plane data from client
                                    match String::from_utf8(data) {
                                        Ok(request_string) => {
                                            trace!("Client request is {}", request_string);

                                            // Tell CP thread to send data to CP
                                            match cp_data(local_addr.clone(), remote_addr.clone(), request_string).await {
                                                Ok(()) => debug!("written to CP"),
                                                Err(e) => return Err(Error::new(ErrorKind::ConnectionAborted, e.to_string())),
                                            }
                                        },
                                        Err(e) => {
                                            return Err(Error::new(ErrorKind::InvalidData, e))
                                        },
                                    }
                                }
                            },
                            Err(ref e) if e.kind() == ErrorKind::ConnectionReset => {
                                debug!("connection reset");
                                break
                            },
                            Err(e) => return Err(e.into()),
                        }
                    }

                    trace!("waiting for threads to finish");

                    // now kill the threads
                    for handle in &handles {
                        handle.abort();
                    }

                    trace!("threads all finished");
                
                    // Tell CP thread to delete client from CP and from hashmap
                    match cp_delete(local_addr.clone(), remote_addr.clone()).await {
                        Ok(()) => return Ok(()),
                        Err(e) => return Err(Error::new(ErrorKind::NotConnected, e.to_string())),
                    }
                },
                Err(e) => return Err(Error::new(ErrorKind::NotConnected, e.to_string())),
            }
        },
        Err(e) => {
            error!("unable to set nodelay");
            return Err(e.into())
        },
    }
}

/// manage outbound client connection from beginning to end
pub async fn client_outbound(remote_addr: String) -> Result<()> { 
    trace!("client_outbound for {}", remote_addr);
    match TcpStream::connect(remote_addr.clone()).await {
        Ok(client_stream) => {
            match client_stream.local_addr() {
                Ok(address) => {
                    let local_addr = address.to_string();
                    trace!("outbound connected from {}", local_addr);
                    tokio::spawn(async move {
                        match client_handler(local_addr, remote_addr, client_stream).await {
                            Ok(()) => debug!("Outbound client disconnected"),
                            Err(e) => error!("Error: {}", e),
                        }
                    });

                    return Ok(())
                },
                Err(e) => return Err(Error::new(ErrorKind::AddrNotAvailable, e.to_string())),
            }
        },
        Err(e) => return Err(e.into()),
    }
}

/// creat inbound client connection
async fn client_inbound(client_stream: TcpStream) -> Result<()> {
    let local_addr: String;
    let remote_addr: String;
    
    match client_stream.local_addr() {
        Ok(address) => local_addr = address.to_string(),
        Err(e) => return Err(e.into()),
    }

    match client_stream.peer_addr() {
        Ok(address) => remote_addr = address.to_string(),
        Err(e) => return Err(e.into()),
    }

    // handler will run as its own thread (per client)
    tokio::spawn(async move {
        match client_handler(local_addr.clone(), remote_addr.clone(), client_stream).await {
            Ok(()) => debug!("Inbound client disconnected"),
            Err(e) => error!("Error: {}", e),
        }
    });

    return Ok(())
}

/// Client listener
pub async fn client_listener(socket: String) -> Result<()> {
    match TcpListener::bind(socket).await {
        Ok(listener) => {
            debug!("Listening for connections");
            loop {

                // will get socket handle plus IP/port for client
                match listener.accept().await {
                    Ok((stream, client)) => {
                        debug!("connected, client is {}", client.to_string());

                        match client_inbound(stream).await {
                            Ok(()) => debug!("Disconnected"),
                            Err(e) => error!("Error: {}", e),
                        }
                    },
                    Err(e) => return Err(e),
                }
            }
        },
        Err(e) => return Err(e),
    }
}
