# Installed Binary

```sh
TERM_WM_LOG_FILE=/tmp/term-wm-daemon.log RUST_BACKTRACE=1 term-wm --daemon
```

otherwise

# 1. build
cargo build -p term-session

# 2. kill any stale generation if "gateway endpoint busy" (session_server.rs:2029)
./target/debug/term-session --gateway term-wm/administrator/gateway-da1546b5 stop --force
# or: TERM_WM_NAMESPACE=term-wm ./target/debug/term-session stop --force

# 3. launch the same generation that crashed, detached, with logs
TERM_WM_LOG_FILE=/tmp/term-wm-daemon.log RUST_BACKTRACE=1 \
  ./target/debug/term-session --daemon --gateway term-wm/administrator/gateway-da1546b5

# tail in another shell
tail -F /tmp/term-wm-daemon.log
./target/debug/term-session --gateway term-wm/administrator/gateway-da1546b5 ls
