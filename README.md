# msm-rtsp-stub
RTSP Sidecar Stub Proxy written in Rust

* Terminates client RTSP connections
* Exchanges RTSP commands and responses with the CP proxy over gRPC
* Sends interleaved RTP data to the DP proxy over UDP
