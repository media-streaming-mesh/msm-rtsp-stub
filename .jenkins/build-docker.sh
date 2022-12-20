#!/bin/bash -x
# Syntax build-docker.sh [-i|--image imagename]

PROJECT=msm-rtsp-stub
DOCKER_IMAGE=${PROJECT}:latest

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

docker build -t ${DOCKER_IMAGE} -f Dockerfile .
