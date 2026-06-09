#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/build-local-tap-release.sh [options]

Build the Linux Codex Homebrew tap archive locally with persistent caches.

Options:
  --target <triple>              Rust target (default: x86_64-unknown-linux-musl)
  --cache-dir <dir>              Persistent build/cache dir (default: .codex-local-build)
  --output-dir <dir>             Clean artifact output dir (default: dist/local-tap-release)
  --image <name>                 Local build image name (default: codex-local-tap-release:ubuntu24)
  --no-container                 Run in the current environment instead of a local container
  --rebuild-image                Rebuild the local container image before running
  --clean-cache                  Delete the persistent cache before building
  --publish                      Create/update the matching joshyorko/codex prerelease asset
  --dispatch-homebrew-tools      Trigger joshyorko/homebrew-tools tap-auto-update after build
  --allow-non-tap-head           Allow publish/dispatch when HEAD is not origin/tap-release
  -h, --help                     Show this help

The default path uses podman or docker to build in Ubuntu 24.04 while keeping
Cargo, target, V8, ripgrep, zsh, and musl helper caches under --cache-dir.
The output directory contains only the package directory, tarball, checksum,
and copied codex/bwrap binaries.
EOF
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="x86_64-unknown-linux-musl"
cache_dir="${repo_root}/.codex-local-build"
output_dir="${repo_root}/dist/local-tap-release"
image="codex-local-tap-release:ubuntu24"
use_container="auto"
rebuild_image="false"
clean_cache="false"
publish="false"
dispatch_homebrew_tools="false"
allow_non_tap_head="false"
inside_container="${CODEX_LOCAL_TAP_INSIDE_CONTAINER:-0}"

args=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      target="${2:?--target requires a value}"
      shift 2
      ;;
    --cache-dir)
      cache_dir="${2:?--cache-dir requires a value}"
      shift 2
      ;;
    --output-dir)
      output_dir="${2:?--output-dir requires a value}"
      shift 2
      ;;
    --image)
      image="${2:?--image requires a value}"
      shift 2
      ;;
    --no-container)
      use_container="false"
      shift
      ;;
    --rebuild-image)
      rebuild_image="true"
      args+=("$1")
      shift
      ;;
    --clean-cache)
      clean_cache="true"
      args+=("$1")
      shift
      ;;
    --publish)
      publish="true"
      args+=("$1")
      shift
      ;;
    --dispatch-homebrew-tools)
      dispatch_homebrew_tools="true"
      args+=("$1")
      shift
      ;;
    --allow-non-tap-head)
      allow_non_tap_head="true"
      args+=("$1")
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unexpected argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ "${cache_dir}" != /* ]]; then
  cache_dir="${repo_root}/${cache_dir}"
fi
if [[ "${output_dir}" != /* ]]; then
  output_dir="${repo_root}/${output_dir}"
fi

container_engine() {
  if [[ -n "${CONTAINER_ENGINE:-}" ]]; then
    command -v "${CONTAINER_ENGINE}" >/dev/null 2>&1 || {
      echo "CONTAINER_ENGINE=${CONTAINER_ENGINE} was not found." >&2
      exit 1
    }
    printf '%s\n' "${CONTAINER_ENGINE}"
    return
  fi

  if command -v podman >/dev/null 2>&1; then
    printf '%s\n' podman
  elif command -v docker >/dev/null 2>&1; then
    printf '%s\n' docker
  else
    return 1
  fi
}

run_in_container() {
  local engine="$1"
  local image_exists="false"
  if [[ "${engine}" == "podman" ]]; then
    if "${engine}" image exists "${image}" >/dev/null 2>&1; then
      image_exists="true"
    fi
  elif "${engine}" image inspect "${image}" >/dev/null 2>&1; then
    image_exists="true"
  fi

  if [[ "${rebuild_image}" == "true" || "${image_exists}" != "true" ]]; then
    "${engine}" build -f "${repo_root}/scripts/local-tap-release.Containerfile" -t "${image}" "${repo_root}"
  fi

  local run_args=(
    run
    --rm
    -t
    -e CODEX_LOCAL_TAP_INSIDE_CONTAINER=1
    -e HOST_UID="$(id -u)"
    -e HOST_GID="$(id -g)"
    -v "${repo_root}:/workspace"
    -w /workspace
  )
  if [[ "${engine}" == "podman" ]]; then
    run_args+=(--security-opt label=disable)
  fi

  "${engine}" "${run_args[@]}" "${image}" \
    bash scripts/build-local-tap-release.sh \
      --target "${target}" \
      --cache-dir "${cache_dir/#${repo_root}/\/workspace}" \
      --output-dir "${output_dir/#${repo_root}/\/workspace}" \
      --no-container \
      "${args[@]}"
}

if [[ "${inside_container}" != "1" && "${use_container}" != "false" ]]; then
  if engine="$(container_engine)"; then
    run_in_container "${engine}"
    exit 0
  fi

  if [[ "${use_container}" == "auto" ]]; then
    echo "No podman/docker found; falling back to current environment." >&2
  fi
fi

cd "${repo_root}"

if [[ "${clean_cache}" == "true" ]]; then
  rm -rf "${cache_dir}"
fi

mkdir -p "${cache_dir}" "${output_dir}"
cache_dir="$(cd "${cache_dir}" && pwd)"
output_dir="$(cd "${output_dir}" && pwd)"

export CARGO_HOME="${cache_dir}/cargo-home"
export CARGO_TARGET_DIR="${cache_dir}/cargo-target"
export CARGO_NET_GIT_FETCH_WITH_CLI="${CARGO_NET_GIT_FETCH_WITH_CLI:-true}"
export RUNNER_TEMP="${cache_dir}/runner-temp"
export TMPDIR="${cache_dir}/tmp"
mkdir -p "${CARGO_HOME}/bin" "${CARGO_TARGET_DIR}" "${RUNNER_TEMP}" "${TMPDIR}"

if command -v rustup >/dev/null 2>&1; then
  rustup target add "${target}" >/dev/null
fi

if [[ "${target}" == *-unknown-linux-musl ]]; then
  github_env="${cache_dir}/github-env"
  : > "${github_env}"
  TARGET="${target}" \
    GITHUB_ENV="${github_env}" \
    RUNNER_TEMP="${RUNNER_TEMP}" \
    SKIP_APT_INSTALL="${SKIP_APT_INSTALL:-1}" \
    bash "${repo_root}/.github/scripts/install-musl-build-tools.sh"

  while IFS= read -r line; do
    [[ -z "${line}" ]] && continue
    name="${line%%=*}"
    value="${line#*=}"
    export "${name}=${value}"
  done < "${github_env}"

  export AWS_LC_SYS_NO_JITTER_ENTROPY=1
  target_no_jitter="AWS_LC_SYS_NO_JITTER_ENTROPY_${target}"
  target_no_jitter="${target_no_jitter//-/_}"
  export "${target_no_jitter}=1"
fi

commit="$(git rev-parse HEAD)"
committed_at="$(git show -s --format=%cI HEAD)"
timestamp="$(date -u -d "${committed_at}" +%Y%m%d%H%M%S)"
version="release.${timestamp}.${commit:0:12}"
rust_version="${CODEX_TAP_RUST_VERSION:-}"
if [[ -z "${rust_version}" ]]; then
  rust_version="$(
    git for-each-ref --sort=-creatordate --format='%(refname:short)' refs/tags/rust-v* \
      | sed -nE 's/^rust-v([0-9]+\.[0-9]+\.[0-9]+)$/\1/p' \
      | head -n 1
  )"
fi
if [[ -z "${rust_version}" ]]; then
  echo "Could not resolve stable rust-v* release tag for tap-release packaging." >&2
  exit 1
fi
release_tag="codex-release-${version}"
asset_name="${release_tag}.tar.gz"
package_dir="${output_dir}/package-${target}"
archive_path="${output_dir}/${asset_name}"

cargo_toml_backup="${RUNNER_TEMP}/Cargo.toml.tap-release.bak"
cargo_lock_backup="${RUNNER_TEMP}/Cargo.lock.tap-release.bak"
cp codex-rs/Cargo.toml "${cargo_toml_backup}"
cp codex-rs/Cargo.lock "${cargo_lock_backup}"
restore_stamped_cargo_files() {
  cp "${cargo_toml_backup}" codex-rs/Cargo.toml
  cp "${cargo_lock_backup}" codex-rs/Cargo.lock
}
trap restore_stamped_cargo_files EXIT

python3 scripts/stamp_rust_workspace_version.py "${rust_version}"

python3 scripts/build_codex_package.py \
  --target "${target}" \
  --variant codex \
  --cargo-profile release \
  --package-dir "${package_dir}" \
  --archive-output "${archive_path}" \
  --force

built_version="$("${CARGO_TARGET_DIR}/${target}/release/codex" --version)"
case "${built_version}" in
  *" 0.0.0"*)
    echo "Refusing to package codex 0.0.0" >&2
    exit 1
    ;;
esac
case "${built_version}" in
  *" ${rust_version}") ;;
  *)
    echo "Expected codex ${rust_version}, got ${built_version}" >&2
    exit 1
    ;;
esac

sha256sum "${archive_path}" | tee "${archive_path}.sha256"
cp "${CARGO_TARGET_DIR}/${target}/release/codex" "${output_dir}/codex"
cp "${CARGO_TARGET_DIR}/${target}/release/bwrap" "${output_dir}/bwrap"

if [[ -n "${HOST_UID:-}" && -n "${HOST_GID:-}" && "$(id -u)" == "0" ]]; then
  chown -R "${HOST_UID}:${HOST_GID}" "${cache_dir}" "${output_dir}"
fi

guard_tap_head() {
  local remote_head
  remote_head="$(git rev-parse --verify origin/tap-release 2>/dev/null || true)"
  if [[ "${allow_non_tap_head}" == "true" || -z "${remote_head}" || "${commit}" == "${remote_head}" ]]; then
    return
  fi

  cat >&2 <<EOF
Refusing to publish or dispatch because HEAD is not origin/tap-release.
  HEAD:               ${commit}
  origin/tap-release: ${remote_head}

Run from an updated tap-release checkout, push this commit to tap-release, or
pass --allow-non-tap-head if you intentionally want a local-only release asset.
EOF
  exit 1
}

if [[ "${publish}" == "true" || "${dispatch_homebrew_tools}" == "true" ]]; then
  guard_tap_head
fi

if [[ "${publish}" == "true" ]]; then
  gh_repo="${CODEX_LOCAL_TAP_RELEASE_REPO:-joshyorko/codex}"
  if gh release view "${release_tag}" --repo "${gh_repo}" >/dev/null 2>&1; then
    gh release upload "${release_tag}" "${archive_path}" --repo "${gh_repo}" --clobber
  else
    gh release create "${release_tag}" "${archive_path}" \
      --repo "${gh_repo}" \
      --title "${release_tag}" \
      --notes "Linux Homebrew tap archive for ${gh_repo}@${commit}." \
      --prerelease \
      --latest=false
  fi
fi

if [[ "${dispatch_homebrew_tools}" == "true" ]]; then
  gh workflow run tap-auto-update.yml \
    --repo joshyorko/homebrew-tools \
    --ref main \
    -f slot_id=codex-release-daily
fi

cat <<EOF
Built local Codex tap release asset.
  version:      ${version}
  rust version: ${rust_version}
  release tag:  ${release_tag}
  archive:      ${archive_path}
  sha256:       ${archive_path}.sha256
  package dir:  ${package_dir}
  binaries:     ${output_dir}/codex ${output_dir}/bwrap
  cache dir:    ${cache_dir}
EOF
