####################################################################################################
## Builder
####################################################################################################
FROM rust:latest AS builder

RUN update-ca-certificates

WORKDIR /msm-rtsp-stub

COPY ./ .

RUN cargo build --release 
RUN strip /msm-rtsp-stub/target/release/msm_rtsp_stub 

####################################################################################################
## Final image
####################################################################################################
FROM ubuntu

WORKDIR /

# Copy our build
COPY --from=builder /msm-rtsp-stub/target/release/msm_rtsp_stub .

ENTRYPOINT ["/msm_rtsp_stub", "--control-plane", "http://10.96.3.1:9000"]

