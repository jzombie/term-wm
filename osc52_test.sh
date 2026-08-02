#!/usr/bin/env bash
# OSC 52 copy test — emits a sequence that places "hello" on the host terminal's clipboard.
# Run in the SSH session, then Cmd+V in a Mac app (e.g. TextEdit).
printf '\033]52;c;aGVsbG8=\007\n'
