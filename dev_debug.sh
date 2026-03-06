#!/bin/bash

cargo run -- --debug 2>&1 | tee console.log
