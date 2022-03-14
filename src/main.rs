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
use self::msm_cp::{Endpoints, Request};

use tokio::net::{TcpListener, TcpStream};
use std::io::{Error, ErrorKind, Result};
use std::net::SocketAddr;

/// read command from client
async fn client_read(stream: &TcpStream) -> Result<(bool, String)> {
    let mut buf = [0u8 ;4096];

    loop {
        // wait until we can read from the stream
        stream.readable().await?;

        match stream.try_read(&mut buf) {
            Ok(0) => return Err(Error::new(ErrorKind::ConnectionReset,"client closed connection")),
            Ok(_) => {
                if buf[0] == 36 {
                    println!("RTP Data");
                    return Ok((true, String::from_utf8_lossy(&buf[1..]).to_string()))
                }
                else {
                    println!("RTSP Command");
                    return Ok((false, String::from_utf8_lossy(&buf).to_string()))
                }
            },
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue, // try again
            Err(e) => return Err(e.into()),
        }
    }
}

/// reflect back to client
async fn client_write(stream: &TcpStream, response: String) -> Result<usize> {
    loop {
        stream.writable().await?;

        match stream.try_write(response.as_bytes()) {
            Ok(bytes) => return Ok(bytes),
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue, // try again
            Err(e) => return Err(e.into()),
        }
    }
}

/// manage client connection from beginning to end...
async fn handle_client(client_stream: TcpStream) -> Result<usize> {
    let mut written_back = 0;
    let dest_addr: SocketAddr;
    let source_addr: SocketAddr;

    // Log local/remote endpoints - can't imagine how this would hit an error...
    match client_stream.local_addr() {
        Ok(local_addr) => dest_addr = local_addr,
        Err(e) => return Err(e)

    }

    match client_stream.peer_addr() {
        Ok(peer_addr) => source_addr = peer_addr,
        Err(e) => return Err(e)
    }

    println!("connecting to control plane");

    match MsmControlPlaneClient::connect("http://127.0.0.1:50051").await {
        Ok(mut cp_handle) => {

            println!("connected to {:?}", cp_handle);

            let request = tonic::Request::new(
                Endpoints {
                    source: source_addr.to_string(),
                    dest: dest_addr.to_string(),
                }
            );

            match cp_handle.client_connect(request).await {
                Ok(_) => {
                    // Loop until connection is reset by either end
                    loop {
                        // read from client
                        match client_read(&client_stream).await {
                            Ok((interleaved, data)) => {
                                if interleaved {    
                                    // Send data to DP proxy over UDP
                                    // need to convert from string to bytes somewhere...
                                    // match send_interleaved(dp_handle, data) {}
                                }
                                else {
                                    // Send command to CP proxy over gRPC
                                    let request = tonic::Request::new(
                                        Request {
                                            request: data,
                                        }
                                    );
                        
                                    match cp_handle.client_request(request).await {
                                        Ok(response) => {
                                            let msg = response.into_inner().response;
                        
                                            println!("API returned {:?}", msg);
                        
                                            match client_write(&client_stream, msg.to_string()).await {
                                                Ok(bytes) => written_back += bytes,
                                                Err(ref e) if e.kind() == ErrorKind::ConnectionReset => break,
                                                Err(e) => return Err(e.into()),
                                            }
                                        },
                                        Err(e) => {
                                            println!( "API send Error {:?}", e);
                        
                                        return Err(Error::new(ErrorKind::ConnectionAborted, e.to_string()))
                                        },
                                    }
                                }
                            },
                            Err(ref e) if e.kind() == ErrorKind::ConnectionReset => break,
                            Err(e) => return Err(e.into()),
                        }
                    }
                },
                Err(e) => {
                    println!( "API connect Error {:?}", e);
        
                    return Err(Error::new(ErrorKind::ConnectionAborted, e.to_string()))
                },
            }
        },
        Err(e) => return Err(Error::new(ErrorKind::NotConnected, e.to_string())),
    }

    return Ok(written_back)
}

#[tokio::main (flavor="current_thread")]
async fn main() {
    match TcpListener::bind(":::8554").await {
        Ok(listener) => {
            println!("Listening for connections");
            loop {

                // will get socket handle plus IP/port for client
                match listener.accept().await {
                    Ok((socket, _client)) => {

                        println!("connected, socket is {:?}", socket);

                        // spawn a green thread per client so can accept more connections
                        tokio::spawn(async move {
                            match handle_client(socket).await {
                                Ok(written) => println!("Disconnected: wrote {} bytes", written),
                                Err(e) => println!("Error: {}", e),
                            }
                        });
                    },
                    Err(e) => {
                        println!("Unable to connect: {}", e);
                    },
                }
            }
        },
        Err(e) => println!("Unable to open listener socket: {}", e),
    }
}
