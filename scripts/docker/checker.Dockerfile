# The image used by scripts/docker-cargo-check.sh.
#
# rust:1-slim has no clippy, and installing it on every run costs a
# download each time. Bake it in once and the image is cached by Docker.
FROM rust:1-slim

# clippy is the whole point of the Linux check — CI runs
# `cargo clippy --workspace --all-targets -- -D warnings`.
RUN rustup component add clippy && rustup component add rustfmt

WORKDIR /w
