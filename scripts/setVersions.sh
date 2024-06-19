#!/bin/sh
sed -i '0,/version = ".*"/{s//version = "'$1'"/}' */Cargo.toml
