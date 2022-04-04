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

#[tokio::main (flavor="current_thread")]
async fn main() {

    let mut handles = vec![];
    // spawn a green thread for the client communication
    handles.push(tokio::spawn(async {
        match client_listener(":::8554".to_string()).await {
            Ok(()) => println!("Disconnected!"),
            Err(e) => println!("Error: {}", e),
        }
    }));

    // spawn a green thread for the CP communication
    handles.push(tokio::spawn(async {
        match cp_connector("http://127.0.0.1:9000".to_string()).await {
            Ok(()) => println!("Disconnected!"),
            Err(e) => println!("Error: {}", e),
        }
    }));

    // wait for all green threads to finish (not gonna happen)
    join_all(handles).await;
}