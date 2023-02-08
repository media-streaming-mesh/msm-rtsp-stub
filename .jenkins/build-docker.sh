#!/bin/bash -x
# Syntax build-docker.sh [-i|--image imagename]

ARCH=multi-arch-image
PROJECT=msm-rtsp-stub
DOCKER_IMAGE=${PROJECT}:${BUILD_ID}
DOCKER_REGISTRY=dockerhub.cisco.com/eti-sre-docker

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
# remove build instance if it exists
docker buildx rm  ${ARCH} | true 
# create a build instance
docker buildx create --name=${ARCH} --driver=docker-container --use
docker buildx build --platform linux/amd64,linux/arm64 -t ${DOCKER_REGISTRY}/${DOCKER_IMAGE} -o type=registry -f Dockerfile .
