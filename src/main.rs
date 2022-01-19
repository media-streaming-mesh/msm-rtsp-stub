use tokio::net::{TcpListener, TcpStream};
use std::io::{Error, ErrorKind, Result};

/// read command from client
async fn handle_read(stream: &TcpStream) -> Result<String> {
    let mut buf = [0u8 ;4096];

    loop {
        // wait until we can read from the stream
        stream.readable().await?;

        match stream.try_read(&mut buf) {
            Ok(0) => return Err(Error::new(ErrorKind::ConnectionReset,"client closed connection")),
            Ok(_) => {
                if buf[0] == 36 {
                    println!("Dollar!");
                    return Ok(String::from("Dollar\n"));
                }
                else {
                    let request = String::from_utf8_lossy(&buf).to_string();
                    println!("{}", request);
                    return Ok(request);
                }
            },
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => continue, // try again
            Err(e) => return Err(e.into()),
        }
    }
}

/// reflect back to client
async fn handle_write(stream: &TcpStream, response: String) -> Result<usize> {
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
async fn handle_client(stream: TcpStream) -> Result<usize> {
    let mut written = 0;
    loop {
        match handle_read(&stream).await {
            Ok(request) => match handle_write(&stream, request).await {
                Ok(bytes) => written += bytes,
                Err(ref e) if e.kind() == ErrorKind::ConnectionReset => break,
                Err(e) => return Err(e.into()),
            },
            Err(ref e) if e.kind() == ErrorKind::ConnectionReset => break,
            Err(e) => return Err(e.into()),
        }
    }

    return Ok(written)
}

#[tokio::main]
async fn main() {
    match TcpListener::bind("127.0.0.1:8554").await {
        Ok(listener) => {
            println!("Listening for connections");
            loop {

                // will get socket handle plus IP/port for client
                match listener.accept().await {
                    Ok((socket, client)) => {
                        println!("Connected: {}", client.to_string());

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