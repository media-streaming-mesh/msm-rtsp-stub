# msm-rtsp-stub
RTSP Sidecar Stub Proxy written in Rust

* Terminates client RTSP connections
* Sends RTSP commands to the CP proxy over gRPC
* Sends interleaved RTP data to the DP proxy over UDP
