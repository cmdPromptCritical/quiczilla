# syntax=docker/dockerfile:1.6
#
# Quiczilla container E2E image. One image serves both roles:
#   entrypoint.sh server  -> sshd target with NO worker on PATH, so every
#                            bootstrap exercises the managed-bundle upload.
#   entrypoint.sh client  -> CLI on PATH, test harness in /e2e.
#
# QZ_SOURCE=local   builds the checked-out tree (default, ad hoc revisions).
# QZ_SOURCE=release installs a published archive and verifies SHA256SUMS.
#
# Neither role installs a system MsQuic package: binaries load the
# libmsquic.so placed beside them, which is how release archives and managed
# worker bundles run on clean hosts.

ARG UBUNTU_VERSION=22.04
ARG QZ_SOURCE=local

# ---------------------------------------------------------------------------
FROM ubuntu:${UBUNTU_VERSION} AS build-local
ARG UBUNTU_VERSION
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl wget build-essential pkg-config \
 && wget -q "https://packages.microsoft.com/config/ubuntu/${UBUNTU_VERSION}/packages-microsoft-prod.deb" \
 && dpkg -i packages-microsoft-prod.deb && rm packages-microsoft-prod.deb \
 && apt-get update && apt-get install -y --no-install-recommends libmsquic \
 && rm -rf /var/lib/apt/lists/*
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
ENV PATH=/root/.cargo/bin:${PATH}
WORKDIR /src
COPY . .
# Build the worker first so the CLI deploys this exact sibling worker rather
# than the tracked cli/embedded bootstrap artifact.
RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p quiczilla-worker \
 && cargo build --release -p quiczilla-cli \
 && mkdir -p /out \
 && cp target/release/quiczilla-cli /out/quiczilla \
 && cp target/release/quiczilla-worker /out/quiczilla-worker \
 && runtime="$(dpkg -L libmsquic | grep -E '/libmsquic\.so(\.[0-9]+)*$' | head -n 1)" \
 && cp "$runtime" /out/libmsquic.so

# ---------------------------------------------------------------------------
FROM ubuntu:${UBUNTU_VERSION} AS build-release
ARG DEBIAN_FRONTEND=noninteractive
ARG QZ_RELEASE_TAG
ARG QZ_REPO=cmdPromptCritical/quiczilla
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl jq \
 && rm -rf /var/lib/apt/lists/*
# Optional BuildKit secret `gh_token` supports private repositories:
#   docker build --secret id=gh_token,env=QUICZILLA_GITHUB_TOKEN ...
RUN --mount=type=secret,id=gh_token,required=false <<'EOF'
set -eu
: "${QZ_RELEASE_TAG:?QZ_RELEASE_TAG is required when QZ_SOURCE=release}"
mkdir -p /dl /out
cd /dl
if [ -s /run/secrets/gh_token ]; then
  auth="Authorization: Bearer $(cat /run/secrets/gh_token)"
  api="https://api.github.com/repos/${QZ_REPO}/releases/tags/${QZ_RELEASE_TAG}"
  curl -fsSL -H "$auth" "$api" > release.json
  for asset in quiczilla-linux-x86_64.tar.gz SHA256SUMS; do
    id="$(jq -r --arg n "$asset" '.assets[] | select(.name == $n) | .id' release.json)"
    curl -fsSL -H "$auth" -H 'Accept: application/octet-stream' \
      "https://api.github.com/repos/${QZ_REPO}/releases/assets/${id}" -o "$asset"
  done
else
  base="https://github.com/${QZ_REPO}/releases/download/${QZ_RELEASE_TAG}"
  curl -fsSLO "$base/quiczilla-linux-x86_64.tar.gz"
  curl -fsSLO "$base/SHA256SUMS"
fi
grep ' quiczilla-linux-x86_64.tar.gz$' SHA256SUMS | sha256sum -c -
tar -C /out -xzf quiczilla-linux-x86_64.tar.gz
EOF

# ---------------------------------------------------------------------------
FROM build-${QZ_SOURCE} AS artifacts

# ---------------------------------------------------------------------------
FROM ubuntu:${UBUNTU_VERSION} AS runtime
ARG DEBIAN_FRONTEND=noninteractive
ARG QZ_GIT_COMMIT=unknown
ARG QZ_SOURCE
ARG QZ_RELEASE_TAG=
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates openssh-server openssh-client iproute2 iptables jq \
      python3 time procps sudo tar coreutils findutils netcat-openbsd \
      libnuma1 libssl3 \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -m -s /bin/bash qz \
 && echo 'qz ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/qz \
 && chmod 0440 /etc/sudoers.d/qz \
 && mkdir -p /run/sshd
COPY --from=artifacts /out/ /opt/quiczilla/
RUN chmod 0755 /opt/quiczilla/quiczilla /opt/quiczilla/quiczilla-worker \
 && echo "/opt/quiczilla" > /etc/ld.so.conf.d/quiczilla.conf \
 && ldconfig \
 && printf '{"source":"%s","release_tag":"%s","git_commit":"%s"}\n' \
      "${QZ_SOURCE}" "${QZ_RELEASE_TAG}" "${QZ_GIT_COMMIT}" > /opt/quiczilla/BUILD_INFO.json
ENV LD_LIBRARY_PATH=/opt/quiczilla:${LD_LIBRARY_PATH:-}
COPY tests/e2e/ /e2e/
RUN find /e2e -type f \( -name '*.sh' \) -exec sed -i 's/\r$//' {} + \
 && chmod 0755 /e2e/*.sh /e2e/cases/*.sh /e2e/docker/entrypoint.sh
ENTRYPOINT ["/e2e/docker/entrypoint.sh"]
CMD ["client"]
