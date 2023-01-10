# syntax=docker/dockerfile:experimental
####################################################################################################
## Builder
####################################################################################################
FROM rust:latest AS builder

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
RUN apt-get update
# Install buildx
# ensure all builds runs with Buildkit
ENV DOCKER_BUILDKIT=1
# enable full log of Buildkit builds
ENV BUILDKIT_PROGRESS=plain
# enable Buildx integration with docker
ENV DOCKER_CLI_EXPERIMENTAL=enabled

ARG BUILDX_URL=https://github.com/docker/buildx/releases/download/v0.9.1/buildx-v0.9.1.linux-amd64
RUN mkdir -p $HOME/.docker/cli-plugins && \
wget -O $HOME/.docker/cli-plugins/docker-buildx $BUILDX_URL && \
chmod a+x $HOME/.docker/cli-plugins/docker-buildx
WORKDIR /

# Copy our build
COPY --from=builder /msm-rtsp-stub/msm_rtsp_stub .

ENTRYPOINT ["/msm_rtsp_stub"]

