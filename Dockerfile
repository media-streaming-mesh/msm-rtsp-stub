# syntax=docker/dockerfile:experimental

####################################################################################################
## Builder
####################################################################################################
FROM rust:latest AS builder

RUN rustup default nightly && rustup update

RUN update-ca-certificates

WORKDIR /msm-rtsp-stub

COPY ./ .

RUN	--mount=type=cache,target=/usr/local/cargo/registry \
	--mount=type=cache,target=/msm-rtsp-stub/target \ 
	cargo +nightly build && cp target/debug/msm_rtsp_stub .

####################################################################################################
## Final image
####################################################################################################
FROM ubuntu

WORKDIR /

# Copy our build
COPY --from=builder /msm-rtsp-stub/msm_rtsp_stub .

ENTRYPOINT ["/msm_rtsp_stub", "--level", "DEBUG", "--control-plane", "http://10.96.3.1:9000", "--data-plane", "172.18.0.2:8050"]

