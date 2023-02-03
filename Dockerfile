# syntax=docker/dockerfile:experimental
####################################################################################################
## Builder
####################################################################################################
FROM dockerhub.cisco.com/docker.io/rust:latest AS builder

RUN rustup default beta && rustup toolchain install beta --component rustfmt,rust-std,clippy && rustup update

RUN update-ca-certificates

WORKDIR /msm-rtsp-stub

COPY ./ .

RUN	--mount=type=cache,target=/usr/local/cargo/registry \
	--mount=type=cache,target=/msm-rtsp-stub/target \
	cargo build && cp target/debug/msm_rtsp_stub .

####################################################################################################
## Final image
####################################################################################################
FROM ubuntu

WORKDIR /

# Copy our build
COPY --from=builder /msm-rtsp-stub/msm_rtsp_stub .

ENTRYPOINT ["/msm_rtsp_stub"]

