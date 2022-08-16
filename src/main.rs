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
use log::{trace, info, error};
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

use simple_logger;
use envmnt;

#[tokio::main (flavor="current_thread")]
async fn main() {

    // We prefer to use LOG_LVL rather than RUST_LOG
    envmnt::set("RUST_LOG", envmnt::get_or("LOG_LVL", "WARN"));

    match simple_logger::init_with_env() {
        Ok(()) => {
            let rtp_port = envmnt::get_u16("RTP_PROXY_PORT", 8050);
            let rtsp_port = envmnt::get_u16("RTSP_PROXY_PORT", 8554);
            
            match IpAddr::from_str(&envmnt::get_or("MSM_DATA_PLANE", "127.0.0.1")) {
                Ok(data_plane_ip) => {
                    let data_plane = SocketAddr::new(data_plane_ip, rtp_port);
                    
                    match Uri::from_str(&envmnt::get_or("MSM_CONTROL_PLANE", "http://127.0.0.1:9000")) {
                        Ok(control_plane) => {
                            let mut handles = vec![];
                            // spawn a green thread for the client communication
                            handles.push(tokio::spawn(async move {
                                match client_listener(format!(":::{}", rtsp_port).to_string()).await {
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
                        },
                        Err(e) => {
                            error!("unable to parse control plane URI {}", e.to_string());
                        }
                    }
                },
                Err(e) => {
                    error!("unable to parse data plane IP address {}", e.to_string());
                }
            }
        },
        Err(e) => {
            error!("unable to log: {}", e.to_string());
        }
    }
}