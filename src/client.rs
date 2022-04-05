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

use crate::cp;

use std::io::{Error, ErrorKind, Result};

use tokio::net::{TcpListener, TcpStream};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;

/// read command from client
async fn client_read(reader: &OwnedReadHalf) -> Result<(bool, Vec<u8>)> {
    loop {
        // wait until we can read from the stream
        reader.readable().await?;

        let mut buf = [0u8; 4096];

        match reader.try_read(&mut buf) {
            Ok(0) => return Err(Error::new(ErrorKind::ConnectionReset,"client closed connection")),
            Ok(bytes_read) => {
                if buf[0] == 36 {
                    println!("RTP Data");
                    return Ok((true, buf[1..bytes_read].to_vec()))
                }
                else {
                    println!("RTSP Command");
                    return Ok((false, buf[..bytes_read].to_vec()))
                }
            },
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue, // try again
            Err(e) => return Err(e.into()),
        }
    }
}

/// reflect back to client
async fn client_write(writer: &OwnedWriteHalf, response: String) -> Result<usize> {
    loop {
        writer.writable().await?;

        match writer.try_write(response.as_bytes()) {
            Ok(bytes) => return Ok(bytes),
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue, // try again
            Err(e) => return Err(e.into()),
        }
    }
}

/// handle messages for client
async fn client_cp_recv(mut rx: mpsc::Receiver<String>, writer: OwnedWriteHalf) -> Result<usize> {
    let mut written_back = 0;

    while let Some(message) = rx.recv().await {
        println!("received {} from CP", message);
        match client_write(&writer, message).await {
            Ok(bytes) => written_back += bytes,
            Err(ref e) if e.kind() == ErrorKind::ConnectionReset => break,
            Err(e) => return Err(e.into()),
        }
    }

    return Ok(written_back);
}

/// handle client connection
async fn client_handler(local_addr: String, remote_addr: String, client_stream: TcpStream, rx: mpsc::Receiver<String>) -> Result<()> {

    // split the client socket into sender/receiver
    let (reader, writer) = client_stream.into_split();

    // Spawn a thread to receive the messages
    tokio::spawn(async move {
        match client_cp_recv(rx, writer).await {
            Ok(written) => {
                println!("Disconnected: wrote {} bytes", written);
            },
            Err(e) => println!("Error: {}", e),
        }
    });

    loop {
        // read from client
        match client_read(&reader).await {
            Ok((interleaved, data)) => {
                if interleaved {    
                    // Send data to DP proxy over UDP
                    // need to convert from string to bytes somewhere...
                    // match send_interleaved(dp_handle, data) {}
                }
                else {
                    match String::from_utf8(data) {
                        Ok(request_string) => {
                            println!("Client request is {}", request_string);
                            match cp::cp_data(local_addr.clone(), remote_addr.clone(), request_string).await {
                                Ok(()) => println!("written to CP"),
                                Err(e) => {
                                    println!( "API send Error {:?}", e);
                
                                    return Err(Error::new(ErrorKind::ConnectionAborted, e.to_string()))
                                },
                            }
                        },
                        Err(e) => {
                            return Err(Error::new(ErrorKind::InvalidData, e))
                        },
                    }
                }
            },
            Err(ref e) if e.kind() == ErrorKind::ConnectionReset => break,
            Err(e) => return Err(e.into()),
        }
    }

    return Ok(())
}

/// manage outbound client connection from beginning to end
pub async fn client_outbound(local_addr: String, remote_addr: String, rx: mpsc::Receiver<String>) -> Result<()> { 
    match TcpStream::connect(remote_addr.clone()).await {
        Ok(client_stream) => return client_handler(local_addr.clone(), remote_addr.clone(), client_stream, rx).await,
        Err(e) => return Err(e),
    }
}

/// manage inbound client connection from beginning to end...
async fn client_inbound(client_stream: TcpStream) -> Result<()> {
    let local_addr = client_stream.local_addr()?.to_string();
    let remote_addr = client_stream.peer_addr()?.to_string();

    // Create channel to receive messages from CP
    let (tx, rx) = mpsc::channel::<String>(5);

    // register client with CP
    match cp::cp_register(tx.clone(), local_addr.clone(), remote_addr.clone()).await {
        Ok(()) => {
            let mut hold_err = false;
            let mut held_err = Error::new(ErrorKind::Other, "not an error - yet!");

            // now process messages to/from the client
            match client_handler(local_addr.clone(), remote_addr.clone(), client_stream, rx).await {
                Ok(()) => {},
                Err(e) => { 
                    hold_err = true;
                    held_err = e;
                },
            }   
    
            // delete client from CP
            match cp::cp_deregister(local_addr.clone(), remote_addr.clone()).await {
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
            println!("Listening for connections");
            loop {

                // will get socket handle plus IP/port for client
                match listener.accept().await {
                    Ok((stream, client)) => {

                        println!("connected, client is {:?}", client);

                        // spawn a green thread per client so can accept more connections
                        tokio::spawn(async move {
                            match client_inbound(stream).await {
                                Ok(()) => println!("Disconnected"),
                                Err(e) => println!("Error: {}", e),
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