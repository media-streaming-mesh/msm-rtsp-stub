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

use msm_rtsp_stub::client::client_listener;
use msm_rtsp_stub::cp::cp_connector;
use msm_rtsp_stub::dp::dp_init;

use futures::future::join_all;
use http::Uri;
use log::{trace, debug, info, error, Level};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use simple_logger;

use clap::Parser;

/// Simple program to greet a person
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
        /// Log level
        #[clap(long)]
        level: Option<Level>,

        /// Listening port
        #[clap(long)]
        port: Option<u16>,

        /// CP URI
        #[clap(long)]
        control_plane: Option<http::uri::Uri>,

        /// DP socket
        #[clap(long)]
        data_plane: Option<std::net::SocketAddr>,
}

#[tokio::main (flavor="current_thread")]
async fn main() {

    let args = Args::parse();
    let mut level = Level::Error;
    let mut port: u16 = 8554;
    let mut control_plane: Uri = "http://127.0.0.1:9000".parse().unwrap();
    let mut data_plane = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8050);

    match args.level {
        Some(log_level) => level = log_level,
        None => {},
    }

    // seemed like a reasonable choice at the time
    simple_logger::init_with_level(level).unwrap();

    match args.port {
        Some(listen_port) => port = listen_port,
        None => debug!("no valid port parameter given - using default"),
    }

    match args.control_plane {
        Some(cp) => control_plane = cp,
        None => debug!("no valid control_plane parameter given - using default"),
    }

    match args.data_plane {
        Some(dp) => data_plane = dp,
        None => debug!("no valid data_plane parameter given - using default"),
    }

    let mut handles = vec![];
    // spawn a green thread for the client communication
    handles.push(tokio::spawn(async move {
        match client_listener(format!(":::{}", port).to_string()).await {
            Ok(()) => info!("Disconnected!"),
            Err(e) => error!("Error: {}", e),
        }
    }));

    // spawn a green thread for the CP communication
    handles.push(tokio::spawn(async move {
        match cp_connector(control_plane).await {
            Ok(()) => info!("Disconnected!"),
            Err(e) => error!("Error: {}", e),
        }
    }));

    match dp_init(data_plane).await {
        Ok(()) => trace!("Connected to DP"),
        Err(e) => error!("Error connecting to DP: {}", e),
    }

    // wait for all green threads to finish (not gonna happen)
    join_all(handles).await;
}