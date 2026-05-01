#!/bin/bash

# setup scratch alias
echo "Setting up scratch alias . . ."
echo 'FROM scratch' | podman build -t scratch-alias -

# create .env file
echo "Creating .env file"
echo -e "USERNAME=$(whoami)\nUSER_ID=$(id -u)\nGROUP_ID=$(id -g)\n" > ./.devcontainer/.env
