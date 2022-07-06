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
	cargo +nightly build

####################################################################################################
## Final image
####################################################################################################
FROM ubuntu

WORKDIR /

# Copy our build
COPY --from=builder /msm-rtsp-stub/target/debug/msm_rtsp_stub .

ENTRYPOINT ["/msm_rtsp_stub", "--level", "DEBUG", "--control-plane", "http://10.96.3.1:9000"]

