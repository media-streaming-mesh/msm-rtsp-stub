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

use bytes::BytesMut;

use log::{debug, error, info, trace, warn};

use std::io::{Error, ErrorKind, Result};

use tokio::net::{TcpListener, TcpStream};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;

const CLIENT_CHANNEL_SIZE: usize = 5;

/// read message from client
async fn client_read(reader: &OwnedReadHalf, buf: &mut BytesMut) -> Result<usize> {
    loop {
        // wait until we can read from the stream
        match reader.readable().await {
            Ok(()) => {
                match reader.try_read_buf(buf) {
                    Ok(0) => return Err(Error::new(ErrorKind::ConnectionReset,"client closed connection")),
                    Ok(bytes_read) => return Ok(bytes_read),
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

/// process CP data from client
async fn client_data(local_addr: String, remote_addr: String, data: &mut[u8]) -> Result<()> {
    // from_utf8_lossy means we can handle the case where we have invalid UTF-8
    // but may only be because of incorrectly received data so switch back to from_utf8 once that's fixed?
    let request_string = String::from_utf8_lossy(data).to_string();

    debug!("Client request length {}, request is {}", request_string.len(), request_string);

    // Tell CP thread to send data to CP
    match cp_data(local_addr.clone(), remote_addr.clone(), request_string).await {
        Ok(()) => return Ok(()),
        Err(e) => return Err(Error::new(ErrorKind::ConnectionAborted, e.to_string())),
    }
}
    
/// read client messages until disconnected
async fn client_reader(local_addr: String, remote_addr: String, reader: &OwnedReadHalf) -> Result<usize> {
    let mut bytes_read: usize = 0;
    let mut frag: Vec<u8> = Vec::new();
    loop {
        let mut buf = BytesMut::with_capacity(262168);
        match client_read(&reader, &mut buf).await {
            Ok(mut length) => {
                bytes_read += length;
                let mut data = &mut buf[..];

                // if fragment left over then we want to add new buffer to it
                // if not then we don't want to copy (fast path)
                if frag.len() > 0 {
                    debug!("fragment length is {}, new buffer length is {}", frag.len(), length);
                    length += frag.len();
                    frag.append(&mut buf.to_vec());
                    data = &mut frag[..];
                }

                // attempt to send to DP
                match dp_demux(length, data).await {
                    Ok((fragment, written, optional_remaining_data)) => {
                        trace!("Sent {} bytes to DP", written);
                        match optional_remaining_data {
                            Some(remaining_data) => {
                                if fragment {
                                    debug!("unfinished buffer");
                                    frag = remaining_data.to_vec();
                                } else {
                                    // assume any non-DP data is CP data
                                    match client_data(local_addr.clone(), remote_addr.clone(), remaining_data).await {
                                        Ok(()) => trace!("written to CP"),
                                        Err(e) => return Err(e.into()),
                                    }
                                    frag.clear();
                                }
                            },
                            None => frag.clear(),
                        }
                    },
                    Err(e) => error!("Error sending client data to DP: {}", e.to_string()),
                }
            },
            Err(ref e) if e.kind() == ErrorKind::ConnectionReset => {
                debug!("connection reset");
                break
            },
            Err(e) => return Err(e.into()),
        }
    }

    return Ok(bytes_read)
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
            Err(ref e) => {
                if e.kind() == ErrorKind::ConnectionReset {
                    warn!("Connecton reset by client");
                    break;
                } else {
                    error!("Error writing to client: {}", e);
                }
            },
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

            // add the client flow to the CP
            // in inbound case this will be unsolicited
            // in outbound case the CP has already sent us a request to add the flow
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

                    // now read messages from client until it finishes
                    match client_reader(local_addr.clone(), remote_addr.clone(), &reader).await {
                        Ok(bytes_read) => debug!("read {} bytes from client", bytes_read),
                        Err(e) => return Err(Error::new(ErrorKind::NotConnected, e.to_string())),
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
                            Err(e) => error!("Outbound client error: {}", e),
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
        match client_handler(local_addr, remote_addr, client_stream).await {
            Ok(()) => debug!("Inbound client disconnected"),
            Err(e) => error!("Inbound client error: {}", e),
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
                            Ok(()) => debug!("Inbound client spawned"),
                            Err(e) => error!("Unable to spawn inbound client: {}", e),
                        }
                    },
                    Err(e) => return Err(e),
                }
            }
        },
        Err(e) => return Err(e),
    }
}
