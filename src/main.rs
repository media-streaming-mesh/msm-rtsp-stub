use std::net::{TcpStream, TcpListener, UdpSocket, SocketAddr};
use std::io::{Read, Write};
use std::thread;

struct Peer {
    rtsp: SocketAddr,
    udp: bool,
    rtp: SocketAddr,
    rtcp: SocketAddr,
}

struct Stream {
    upstream: Peer,
    clients: Vec<Peer>,
}

fn handle_read(mut stream: &TcpStream) -> String {
    let mut buf = [0u8 ;4096];
    match stream.read(&mut buf) {
        Ok(_) => {
            if buf[0] == 36 {
                println!("Dollar!");
                return String::from("Dollar\n");
            }
            else {
                let request = String::from_utf8_lossy(&buf).to_string();
                println!("{}", request);
                return request;
            }
        },
        Err(e) => {
            println!("Unable to read stream: {}", e);
            return String::from("Read Error");
        },
    }
}

fn handle_write(mut stream: &TcpStream, response: String) {
    match stream.write(response.as_bytes()) {
        Ok(_) => println!("Sent {}", response),
        Err(e) => println!("Failed sending response: {}", e),
    }
}

fn handle_client(stream: TcpStream) {
    loop {
        let request = handle_read(&stream);
        handle_write(&stream, request);
    }
}

fn main() {
    let listener = TcpListener::bind("127.0.0.1:8554").unwrap();
    println!("Listening for connections on port {}", 8554);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let client = stream.peer_addr();
                match client {
                    Ok(client) => {
                        println!("Connected: {}", client.to_string());
                        thread::spawn(|| {
                            handle_client(stream)
                        });
                    },
                    Err(e) => {
                        println!("Unable to get client IP: {}", e);
                    },
                }
            },
            Err(e) => {
                println!("Unable to connect: {}", e);
            },
        }
    }
}
