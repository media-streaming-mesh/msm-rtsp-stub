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
use crate::dp::dp_send;

use log::{debug, error, trace};

use std::io::{Error, ErrorKind, Result};

use tokio::net::{TcpListener, TcpStream};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;

/// read command from client
async fn client_read(reader: &OwnedReadHalf) -> Result<(bool, Vec<u8>)> {
    loop {
        // wait until we can read from the stream
        match reader.readable().await {
            Ok(()) => {
                let mut buf = [0u8; 4096];
        
                match reader.try_read(&mut buf) {
                    Ok(0) => return Err(Error::new(ErrorKind::ConnectionReset,"client closed connection")),
                    Ok(bytes_read) => {
                        if buf[0] == 36 {
                            debug!("RTP Data");
                            return Ok((true, buf[1..bytes_read].to_vec()))
                        }
                        else {
                            debug!("RTSP Command");
                            return Ok((false, buf[..bytes_read].to_vec()))
                        }
                    },
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
async fn client_write(writer: &OwnedWriteHalf, response: String) -> Result<usize> {
    trace!("writing response {} to client", response);
    loop {
        match writer.writable().await {
            Ok(()) => {
                match writer.try_write(response.as_bytes()) {
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
async fn client_cp_recv(mut rx: mpsc::Receiver<String>, writer: OwnedWriteHalf) -> Result<usize> {
    let mut written_back = 0;

    while let Some(message) = rx.recv().await {
        trace!("received {} from CP", message);
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
async fn client_handler(local_addr: String, remote_addr: String, client_stream: TcpStream, rx: mpsc::Receiver<String>) -> Result<()> {

    // Need socket to flush messages immediately 
    match client_stream.set_nodelay(true) {
        Ok(()) => {
            // split socket into sender/receiver so can hand sender to separate thread
            let (reader, writer) = client_stream.into_split();

            // Spawn thread to receive messages from CP and send to client
            tokio::spawn(async move {
                match client_cp_recv(rx, writer).await {
                    Ok(written) => {
                        debug!("Disconnected: wrote {} bytes", written);
                    },
                    Err(e) => error!("Error: {}", e),
                }
            });

            loop {
                // read from client socket
                match client_read(&reader).await {
                    Ok((interleaved, data)) => {
                        if interleaved {
                            trace!("Sending data to DP");
                            match dp_send(data).await {
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
                    }
                    Err(e) => return Err(e.into()),
                }
            }

            trace!("leaving client handler");
            return Ok(())
        },
        Err(e) => {
            error!("unable to set nodelay");
            return Err(e.into())
        },
    }
}

/// manage outbound client connection from beginning to end
pub async fn client_outbound(remote_addr: String, rx: mpsc::Receiver<String>) -> Result<String> { 
    match TcpStream::connect(remote_addr.clone()).await {
        Ok(client_stream) => {
            match client_stream.local_addr() {
                Ok(address) => {
                    let local_addr = address.to_string();
                    tokio::spawn(async move {
                        match client_handler(local_addr, remote_addr, client_stream, rx).await {
                            Ok(()) => debug!("Disconnected"),
                            Err(e) => error!("Error: {}", e),
                        }
                    });

                    return Ok(address.to_string())
                },
                Err(e) => return Err(Error::new(ErrorKind::AddrNotAvailable, e.to_string())),
            }
        },
        Err(e) => return Err(e.into()),
    }
}

/// manage inbound client connection from beginning to end...
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

    // Create channel to receive messages from CP thread
    let (tx, rx) = mpsc::channel::<String>(5);

    // Tell CP thread to add client to CP and to hashmap
    match cp_add(tx.clone(), local_addr.clone(), remote_addr.clone()).await {
        Ok(()) => {
            let mut hold_err = false;
            let mut held_err = Error::new(ErrorKind::Other, "not an error - yet!");

            // now process messages to/from the client
            // will only terminate when client closes or error occurs
            match client_handler(local_addr.clone(), remote_addr.clone(), client_stream, rx).await {
                Ok(()) => {},
                Err(e) => { 
                    hold_err = true;
                    held_err = e;
                },
            }   
    
            // Tell CP thread to delete client from CP and from hashmap
            match cp_delete(local_addr.clone(), remote_addr.clone()).await {
                Ok(()) => {
                    if hold_err {
                        return Err(held_err);
                    } else {
                        return Ok(());
                    }
                },
                Err(e) => return Err(Error::new(ErrorKind::NotConnected, e.to_string())),
            }
        },
        Err(e) => return Err(Error::new(ErrorKind::NotConnected, e.to_string())),
    }
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

                        // spawn a green thread per client so can accept more connections
                        tokio::spawn(async move {
                            match client_inbound(stream).await {
                                Ok(()) => debug!("Disconnected"),
                                Err(e) => error!("Error: {}", e),
                            }
                        });
                    },
                    Err(e) => return Err(e),
                }
            }
        },
        Err(e) => return Err(e),
    }
}
