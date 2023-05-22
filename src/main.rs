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

use futures::future::join_all;
use http::Uri;
use log::{debug, info, error};
use std::str::FromStr;
use tokio::signal::unix::{signal, SignalKind};
use tokio_util::sync::CancellationToken;
use simple_logger;
use envmnt;

#[tokio::main (flavor="current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = CancellationToken::new();
    let mut sigterm = signal(SignalKind::terminate())?;

    // We prefer to use MSM_LOG_LVL rather than RUST_LOG
    envmnt::set("RUST_LOG", envmnt::get_or("MSM_LOG_LVL", "WARN"));

    match simple_logger::init_with_env() {
        Ok(()) => {
            let rtsp_port = envmnt::get_u16("RTSP_PROXY_PORT", 8554);
                    
            match Uri::from_str(&envmnt::get_or("MSM_CONTROL_PLANE", "http://127.0.0.1:9000")) {
                Ok(control_plane) => {
                    let mut handles = vec![];
                    // spawn a task for the client communication
                    let listener_token = token.clone();
                    handles.push(tokio::spawn(async move {
                        match client_listener(format!(":::{}", rtsp_port).to_string(), listener_token).await {
                            Ok(()) => info!("Disconnected!"),
                            Err(e) => error!("Error: {}", e),
                        }
                    }));
                    
                    // create the identity string for the client
                    let node_name = envmnt::get_or("MSM_NODE_NAME", "node");
                    let namespace = envmnt::get_or("MSM_POD_NAMESPACE", "namespace");
                    let pod_name = envmnt::get_or("MSM_POD_NAME", "pod");
                    let identity_string = [node_name, namespace, pod_name].join(":");
                    debug!("identity string is {}", identity_string);

                    // spawn a task for the CP communication
                    let connector_token = token.clone();
                    handles.push(tokio::spawn(async move {
                        match cp_connector(control_plane, identity_string, connector_token).await {
                            Ok(()) => info!("Disconnected!"),
                            Err(e) => error!("Error: {}", e),
                        }
                    }));
                    
                    // wait for both green threads to finish (not gonna happen) or sigterm
                    tokio::select! {
                        _ = join_all(handles) => {
                            info!("handles joined");
                        }
                        _ = sigterm.recv() => {
                            info!("terminated!");
                        }
                    }
                }
                Err(e) => {
                    error!("unable to parse control plane URI {}", e.to_string());
                }
            }
        },
        Err(e) => {
            error!("unable to log: {}", e.to_string());
        }
    }

    // kill all tasks
    token.cancel();

    Ok(())
}