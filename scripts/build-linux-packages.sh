#!/usr/bin/env bash
# Builds dist/Agentty-<version>-linux-amd64.deb and dist/Agentty-<version>-linux-x86_64.rpm.
#
# Run from any machine with Docker (the release workflow runs it on a hosted Ubuntu runner): the build happens in an
# AlmaLinux 9 x86_64 container (`--platform linux/amd64`; emulated on ARM, so expect a slow first build there). AlmaLinux 9
# has glibc 2.34, so the binary also runs on RHEL 9, Debian 12 and Ubuntu 22.04 and newer. Cargo's registry, the
# toolchain and target/ live in named volumes, so later runs reuse them. nfpm (pinned version and checksum) writes both
# packages from packaging/linux/nfpm.yaml.
#
#   scripts/build-linux-packages.sh           # build both packages
#   scripts/build-linux-packages.sh --check   # then install each into a clean Ubuntu / AlmaLinux container
set -euo pipefail
cd "$(dirname "$0")/.."

NFPM_VERSION=2.47.0
NFPM_SHA256=0660ca602b2d2d2ae4781a06c692b3eeb9d437ffea05b831d76e41f4a3188783  # nfpm_2.47.0_Linux_x86_64.tar.gz
IMAGE=almalinux:9
PLATFORM=linux/amd64

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
deb="Agentty-$version-linux-amd64.deb"
rpm="Agentty-$version-linux-x86_64.rpm"

# ─── Inside the container ───
if [[ "${1:-}" == "--in-container" ]]; then
  export PATH="/root/.cargo/bin:$PATH"
  dnf -y -q install dnf-plugins-core >/dev/null
  dnf config-manager --set-enabled crb
  dnf -y -q install gcc gcc-c++ make cmake clang clang-devel git pkgconf-pkg-config libxkbcommon-devel libxkbcommon-x11-devel \
    wayland-devel libxcb-devel libX11-devel vulkan-loader-devel fontconfig-devel freetype-devel libzstd-devel >/dev/null
  command -v rustup >/dev/null || curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none --no-modify-path
  rustup toolchain install
  # whisper.cpp (voice) is tuned for the CPU that compiles it unless told otherwise; a package built on a
  # runner with AVX-512 would die with an illegal instruction on most desktops. GGML_NATIVE=OFF keeps
  # ggml's portable x86-64 baseline (AVX2/FMA/F16C/BMI2, 2013 on; the app turns voice off on older CPUs,
  # see agentty_bridge::voice::X86_REQUIRED). whisper-rs-sys forwards GGML_* to cmake but does not
  # rebuild when they change, so its cached build in the target volume is dropped first.
  export GGML_NATIVE=OFF
  cargo clean --release -p whisper-rs-sys
  cargo build --release -p agentty-app

  tools=/root/.cargo/nfpm-$NFPM_VERSION
  if [[ ! -x "$tools/nfpm" ]]; then
    archive="$(mktemp -d)/nfpm.tar.gz"
    curl --proto "=https" --tlsv1.2 -fsSL -o "$archive" \
      "https://github.com/goreleaser/nfpm/releases/download/v$NFPM_VERSION/nfpm_${NFPM_VERSION}_Linux_x86_64.tar.gz"
    echo "$NFPM_SHA256  $archive" | sha256sum -c --quiet - || { echo "nfpm checksum mismatch" >&2; exit 1; }
    mkdir -p "$tools"
    tar -xzf "$archive" -C "$tools" nfpm
  fi
  mkdir -p dist
  rm -f "dist/$deb" "dist/$rpm"
  VERSION="$version" "$tools/nfpm" package -f packaging/linux/nfpm.yaml -p deb -t "dist/$deb"
  VERSION="$version" "$tools/nfpm" package -f packaging/linux/nfpm.yaml -p rpm -t "dist/$rpm"
  (cd dist && sha256sum "$deb" "$rpm")
  exit 0
fi

# ─── On the host ───
docker info >/dev/null 2>&1 || { echo "Docker is not running (start Docker Desktop)" >&2; exit 1; }
# git in the container (root) refuses the host-owned checkout ("dubious ownership"), so the commit
# Settings → About shows is read here and handed in; SOURCE_DATE_EPOCH, when set, fixes the date.
build_env=(-e CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-auto}" -e AGENTTY_SOURCE_COMMIT="$(git rev-parse --short=9 HEAD 2>/dev/null || true)")
if [[ -n "${SOURCE_DATE_EPOCH:-}" ]]; then
  build_env+=(-e SOURCE_DATE_EPOCH="$SOURCE_DATE_EPOCH")
fi
docker run --rm --platform "$PLATFORM" \
  -v "$PWD:/src" -w /src \
  -v agentty-linux-amd64-cargo:/root/.cargo \
  -v agentty-linux-amd64-rustup:/root/.rustup \
  -v agentty-linux-amd64-target:/src/target \
  "${build_env[@]}" \
  "$IMAGE" bash scripts/build-linux-packages.sh --in-container

if [[ "${1:-}" == "--check" ]]; then
  # Installs each package with the distribution's package manager (which resolves the declared dependencies) and
  # fails if the binary still misses a shared library.
  probe='ldd /usr/bin/agentty | grep "not found" && exit 1; test -f /usr/share/applications/run.agentty.Agentty.desktop; echo "ok: $(ldd /usr/bin/agentty | wc -l) libraries resolved"'
  docker run --rm --platform "$PLATFORM" -v "$PWD/dist:/dist:ro" ubuntu:22.04 bash -euo pipefail -c "
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq && apt-get install -y -qq --no-install-recommends /dist/$deb >/dev/null
    $probe"
  docker run --rm --platform "$PLATFORM" -v "$PWD/dist:/dist:ro" "$IMAGE" bash -euo pipefail -c "
    dnf -y -q install --setopt=install_weak_deps=False /dist/$rpm >/dev/null
    $probe"
fi
echo "dist/$deb"
echo "dist/$rpm"
