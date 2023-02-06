#!/bin/bash -x
# Syntax build-docker.sh [-i|--image imagename]
PROJECT=msm-rtsp-stub
DOCKER_IMAGE=${PROJECT}:latest
ARCH=multi-arch-image
while [[ $# -gt 0 ]]
do
    key="${1}"

    case ${key} in
    -i|--image)
        DOCKER_IMAGE="${2}"
        shift;shift
        ;;
    -h|--help)
        less README.md
        exit 0
        ;;
    *) # unknown
        echo Unknown Parameter $1
        exit 4
    esac
done
echo BUILDING DOCKER ${DOCKER_IMAGE}
# enable experimental buildx features
docker version
rm -rf ~/.docker/cli-plugins/
mkdir -p ~/.docker/cli-plugins/
BUILDX_LATEST_BIN_URI=$(curl -s -L https://github.com/docker/buildx/releases/latest | grep 'linux-amd64' | grep 'href' | sed 's/.*href="/https:\/\/github.com/g; s/amd64".*/amd64/g')
curl -s -L ${BUILDX_LATEST_BIN_URI} -o ~/.docker/cli-plugins/docker-buildx
chmod a+x ~/.docker/cli-plugins/docker-buildx
# Get and run the latest docker/binfmt tag to use its qemu parts
BINFMT_IMAGE_TAG=$(curl -s https://registry.hub.docker.com/v2/repositories/docker/binfmt/tags | jq '.results | sort_by(.last_updated)[-1].name' -r)
docker run --rm --privileged docker/binfmt:${BINFMT_IMAGE_TAG}
# create a build instance
# docker run --rm --privileged multiarch/qemu-user-static --reset -p yes
docker buildx rm  ${ARCH} | true
docker buildx create --name=${ARCH} --driver=docker-container --use 
docker buildx build --platform linux/amd64,linux/arm64 -t ${DOCKER_IMAGE} -f Dockerfile .

