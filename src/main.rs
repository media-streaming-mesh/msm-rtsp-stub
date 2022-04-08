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
use log::{trace, info, error};

use simple_logger;

#[tokio::main (flavor="current_thread")]
async fn main() {

    // seemed like a reasonable choice at the time
    simple_logger::init_with_env().unwrap();

    let mut handles = vec![];
    // spawn a green thread for the client communication
    handles.push(tokio::spawn(async {
        match client_listener(":::8554".to_string()).await {
            Ok(()) => info!("Disconnected!"),
            Err(e) => error!("Error: {}", e),
        }
    }));

    // spawn a green thread for the CP communication
    handles.push(tokio::spawn(async {
        match cp_connector("http://127.0.0.1:9000".to_string()).await {
            Ok(()) => info!("Disconnected!"),
            Err(e) => error!("Error: {}", e),
        }
    }));

    match dp_init().await {
        Ok(()) => trace!("Connected to DP"),
        Err(e) => error!("Error connecting to DP: {}", e),
    }

    // wait for all green threads to finish (not gonna happen)
    join_all(handles).await;
}