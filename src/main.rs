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

use futures::future::join_all;

use self::msm_cp::msm_control_plane_client::MsmControlPlaneClient;
use self::msm_cp::{Endpoints, Request};

use std::io::{Error, ErrorKind, Result};
use std::net::SocketAddr;
use std::str::FromStr;

use tokio::net::{TcpListener, TcpStream};
use tonic::transport::Channel;

use void::Void;


/// read command from client
async fn client_read(stream: &TcpStream) -> Result<(bool, Vec<u8>)> {
    loop {
        // wait until we can read from the stream
        stream.readable().await?;

        let mut buf = [0u8; 4096];

        match stream.try_read(&mut buf) {
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

/// Connect to CP
async fn cp_connect(source_addr: String, dest_addr: String) -> Result<MsmControlPlaneClient<Channel>> {

    match MsmControlPlaneClient::connect("http://127.0.0.1:50051").await {
        Ok(mut cp_handle) => {

            println!("connected to {:?}", cp_handle);

            let request = tonic::Request::new(
                Endpoints {
                    source: source_addr,
                    dest: dest_addr,
                }
            );

            match cp_handle.client_connect(request).await {
                Ok(_) => {
                    return Ok(cp_handle)
                },
                Err(e) => {
                    println!( "API connect Error {:?}", e);
        
                    return Err(Error::new(ErrorKind::ConnectionAborted, e.to_string()))
                },
            }
        },
        Err(e) => return Err(Error::new(ErrorKind::NotConnected, e.to_string())),
    }

}

/// Send request to CP
async fn cp_send(mut cp_handle: MsmControlPlaneClient<Channel>, request_string: String) -> Result<String> {
    let request = tonic::Request::new(
        Request {
            request: request_string,
        }
    );

    match cp_handle.client_request(request).await {
        Ok(response) => {
            let message = response.into_inner().response;

            println!("API returned {:?}", message);

            return Ok(message);
        },
        Err(e) => {
            println!( "API send Error {:?}", e);

            return Err(Error::new(ErrorKind::ConnectionAborted, e.to_string()))
        },
    }
}

/// manage client connection from beginning to end...
async fn handle_client(client_stream: TcpStream) -> Result<usize> {
    let mut written_back = 0;
    let source_addr = client_stream.peer_addr()?.to_string();
    let dest_addr = client_stream.local_addr()?.to_string();

    println!("connecting to control plane");

    match cp_connect(source_addr, dest_addr).await {
        Ok(cp_handle) => {
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
                            match String::from_utf8(data) {
                                Ok(request_string) => {
                                    match cp_send(cp_handle.clone(), request_string).await {
                                        Ok(response) => {
                                            match client_write(&client_stream, response).await {
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
        },
        Err(e) => return Err(Error::new(ErrorKind::NotConnected, e.to_string())),
    }

    return Ok(written_back)
}

/// Client listener
async fn client_listener(socket: SocketAddr) -> Result<Void> {
    match TcpListener::bind(socket).await {
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
                    Err(e) => return Err(e),
                }
            }
        },
        Err(e) => return Err(e),
    }
}

/// CP listener
async fn cp_listener(_socket: SocketAddr) -> Result<Void> {
    Err(Error::new(ErrorKind::NotConnected, "unexpected"))
}

#[tokio::main (flavor="current_thread")]
async fn main() {

    let mut handles = vec![];
    // spawn a green thread for the client listener
    handles.push(tokio::spawn(async move {
        match client_listener(SocketAddr::from_str(":::8554").unwrap()).await {
            Ok(written) => println!("Disconnected: wrote {} bytes", written),
            Err(e) => println!("Error: {}", e),
        }
    }));

    // spawn a green thread for the CP listener
    handles.push(tokio::spawn(async move {
        match cp_listener(SocketAddr::from_str(":::9000").unwrap()).await {
            Ok(written) => println!("Disconnected: wrote {} bytes", written),
            Err(e) => println!("Error: {}", e),
        }
    }));

    // wait for all green threads to finish (not gonna happen)
    join_all(handles).await;
}