#!/bin/bash

# Debug launch command for primary local interface installs.
#
# This is used for debugging scenarios where term-wm may run as the primary
# local user interface without remote access / SSH driving it.
#
# See related issue:: https://github.com/jzombie/term-wm/issues/163

cage -s -- foot sh -c "RUST_BACKTRACE=1 cargo run --release"
