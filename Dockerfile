####################################################################################################
## Builder
####################################################################################################
FROM rust:latest AS builder

RUN rustup target add x86_64-unknown-linux-musl
RUN apt update && apt install -y musl-tools musl-dev
RUN update-ca-certificates

WORKDIR /msm-rtsp-stub

COPY ./ .

RUN cargo build --release 
RUN strip /msm-rtsp-stub/target/release/msm_rtsp_stub 

####################################################################################################
## Final image
####################################################################################################
FROM scratch

WORKDIR /msm-rtsp-stub

# Copy our build
COPY --from=builder /msm-rtsp-stub/target/release/msm_rtsp_stub ./

CMD ["/msm-rtsp-stub/msm_rtsp_stub"]

