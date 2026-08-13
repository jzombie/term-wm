# term-wm — a cross-platform terminal window manager.
#
# Build:
#   docker build -t term-wm .
#
# Run (the TUI needs a TTY):
#   docker run -it term-wm
#   docker run -it term-wm -- htop

# --- Build stage ---
FROM rust:1.97-alpine3.24 AS build
WORKDIR /src/term-wm/

# Copy the manifests + lockfile and the sources needed to resolve them.
# (`src/lib.rs` embeds README.md; crate assets like help.md ship inside crates/.)
# The root package's targets (src/main.rs, src/lib.rs) must be present before
# `cargo fetch`, since cargo parses the full manifest.
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY README.md ./
COPY src/ src/

# Cache dependencies: the fetch layer is only rebuilt when the manifests,
# lockfile, or sources above change.
RUN cargo fetch

RUN cargo build --release --bin term-wm

# --- Runtime stage ---
FROM alpine:3.24
# The window manager spawns a shell in every window; install bash (the default
# shell fallback) and set SHELL so spawned panes use it. htop ships as a
# convenient `--run htop` demo command.
RUN apk add --no-cache bash htop
COPY --from=build /src/term-wm/target/release/term-wm /bin/term-wm

# Spawned shells start in this working directory.
WORKDIR /root
ENV SHELL=/bin/bash \
    TERM=xterm-256color

ENTRYPOINT [ "term-wm" ]
