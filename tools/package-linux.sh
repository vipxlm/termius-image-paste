#!/usr/bin/env bash
set -euo pipefail

# Build on Ubuntu 24.04 x86_64. Keep Cargo and packaging intermediates on Linux
# storage even when the checkout is mounted from Windows through /mnt/.
project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
build_dir="${CARGO_TARGET_DIR:-${HOME}/.cache/termius-build}"
output_dir="${project_root}/dist"
skip_build=false
case "${1:-}" in
  "") ;;
  --skip-build) skip_build=true ;;
  *) printf 'Usage: %s [--skip-build]\n' "$0" >&2; exit 2 ;;
esac
if [[ "$(uname -s)" != Linux || "$(uname -m)" != x86_64 ]]; then
  printf 'This package targets Ubuntu 24.04 or later on x86_64.\n' >&2
  exit 1
fi
if command -v cargo >/dev/null 2>&1; then cargo_bin="$(command -v cargo)";
elif [[ -x "${HOME}/.cargo/bin/cargo" ]]; then cargo_bin="${HOME}/.cargo/bin/cargo";
else printf 'Rust Cargo is required.\n' >&2; exit 1; fi
for package_tool in dpkg-deb dpkg-shlibdeps python3 ldd install sha256sum; do
  command -v "$package_tool" >/dev/null 2>&1 || { printf 'Missing tool: %s\n' "$package_tool" >&2; exit 1; }
done
mkdir -p -- "$build_dir" "$output_dir"
build_dir="$(realpath -- "$build_dir")"
if [[ "$skip_build" == false ]]; then
  CARGO_TARGET_DIR="$build_dir" "$cargo_bin" build --locked --bins --release \
    --features tauri/custom-protocol --manifest-path "${project_root}/src-tauri/Cargo.toml"
fi
linux_binary="${build_dir}/release/termius-image-paste"
[[ -x "$linux_binary" ]] || { printf 'Release executable is missing: %s\n' "$linux_binary" >&2; exit 1; }
package_version="$(python3 -c 'import sys,tomllib; print(tomllib.load(open(sys.argv[1],"rb"))["package"]["version"])' "${project_root}/src-tauri/Cargo.toml")"
package_work="$(mktemp -d "${build_dir}/package.XXXXXX")"
trap 'rm -rf -- "$package_work"' EXIT
package_stage="${package_work}/debian/termius-image-paste"
mkdir -p -- "${package_work}/debian" "${package_stage}/DEBIAN"
ldd "$linux_binary" > "${package_work}/ldd.txt"
if grep -q 'not found' "${package_work}/ldd.txt"; then
  cat "${package_work}/ldd.txt" >&2
  printf 'The executable has unresolved runtime libraries.\n' >&2
  exit 1
fi
cat > "${package_work}/debian/control" <<EOF
Source: termius-image-paste
Section: utils
Priority: optional
Maintainer: Termius Image Paste <noreply@example.invalid>

Package: termius-image-paste
Architecture: amd64
Description: Termius screenshot upload helper
EOF
install -Dm755 -- "$linux_binary" "${package_stage}/usr/bin/termius-image-paste"
library_dependencies="$(cd "$package_work" && dpkg-shlibdeps -O -e debian/termius-image-paste/usr/bin/termius-image-paste | sed -n 's/^shlibs:Depends=//p')"
[[ -n "$library_dependencies" ]] || { printf 'Cannot determine ELF runtime dependencies.\n' >&2; exit 1; }
install -Dm644 -- "${project_root}/src-tauri/icons/icon.png" \
  "${package_stage}/usr/share/icons/hicolor/256x256/apps/termius-image-paste.png"
install -Dm644 -- "${project_root}/LICENSE" \
  "${package_stage}/usr/share/doc/termius-image-paste/copyright"
if [[ -f "${output_dir}/THIRD_PARTY_NOTICES-linux-x86_64.txt" ]]; then
  install -Dm644 -- "${output_dir}/THIRD_PARTY_NOTICES-linux-x86_64.txt" \
    "${package_stage}/usr/share/doc/termius-image-paste/THIRD_PARTY_NOTICES.txt"
fi
mkdir -p -- "${package_stage}/usr/share/applications"
cat > "${package_stage}/usr/share/applications/termius-image-paste.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=Termius 贴图助手
Comment=截图上传到 SSH 服务器并取用远程路径
Exec=termius-image-paste
Icon=termius-image-paste
Terminal=false
Categories=Utility;Network;
EOF
cat > "${package_stage}/DEBIAN/control" <<EOF
Package: termius-image-paste
Version: ${package_version}
Section: utils
Priority: optional
Architecture: amd64
Maintainer: Termius Image Paste <noreply@example.invalid>
Depends: ${library_dependencies}, openssh-client (>= 1:9.0p1), libxtst6
Description: Termius screenshot upload helper for Ubuntu 24.04 x86_64
 Upload screenshots through strictly verified SSH connections. Windows and
 Linux X11 support guarded automatic input; Wayland uses in-window upload
 and explicit manual retrieval of the remote image path.
EOF
portable_path="${output_dir}/TermiusImagePaste-linux-x86_64"
deb_path="${output_dir}/termius-image-paste_${package_version}_amd64.deb"
install -m755 -- "$linux_binary" "$portable_path"
dpkg-deb --build --root-owner-group "$package_stage" "$deb_path"
(cd "$output_dir" && sha256sum -- "$(basename -- "$portable_path")" "$(basename -- "$deb_path")") > "${output_dir}/SHA256SUMS-linux.txt"
printf '\nLinux ELF: %s\nDebian package: %s\n' "$portable_path" "$deb_path"
printf 'Runtime dependencies: %s, openssh-client >= 9.0, libxtst6\n' "$library_dependencies"
