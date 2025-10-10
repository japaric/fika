alias t := test

test:
  cargo test --target armv7-unknown-linux-musleabi

clippy:
  cargo clippy -- -D warnings

fmt:
  rustup toolchain install nightly-2025-09-14 --profile minimal --component rustfmt
  cargo +nightly-2025-09-14 fmt --check

setup:
  cp src/pre-commit.bash .git/hooks/pre-commit

pre-commit-check:
  git diff --quiet || exit 1
  just t
  just clippy
  just fmt
