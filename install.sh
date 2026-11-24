#!/bin/sh
# Installs the fdt CLI from GitHub Releases on macOS and Linux, checking its SHA256.
#
#   curl -fsSL https://github.com/fundathyon/fdt-cli/releases/latest/download/install.sh | sh
#
# Variables (all optional):
#   FDT_INSTALL_DIR      destination directory (default ~/.local/bin; never needs sudo)
#   FDT_INSTALL_VERSION  version to install, e.g. v0.1.0 (default: the latest release)
#   FDT_RELEASES_URL     where the releases live (default https://github.com/fundathyon/fdt-cli/releases)
#
#   curl -fsSL …/install.sh | FDT_INSTALL_VERSION=v0.1.0 FDT_INSTALL_DIR="$HOME/bin" sh
set -eu

releases="${FDT_RELEASES_URL:-https://github.com/fundathyon/fdt-cli/releases}"
releases="${releases%/}"
dir="${FDT_INSTALL_DIR:-$HOME/.local/bin}"
version="${FDT_INSTALL_VERSION:-}"

die() {
  echo "fdt: $*" >&2
  exit 1
}

case "$(uname -s)" in
  Linux) os="unknown-linux-musl" ;;
  Darwin) os="apple-darwin" ;;
  MINGW* | MSYS* | CYGWIN*) die "en Windows instala con PowerShell: irm https://github.com/fundathyon/fdt-cli/releases/latest/download/install.ps1 | iex" ;;
  *) die "no hay binario de fdt para $(uname -s); compílalo con: cargo install --git https://github.com/fundathyon/fdt-cli" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *) die "no hay binario de fdt para la arquitectura $(uname -m); compílalo con: cargo install --git https://github.com/fundathyon/fdt-cli" ;;
esac
# A shell running under Rosetta reports x86_64 on Apple silicon: install the native binary.
if [ "$os" = "apple-darwin" ] && [ "$arch" = "x86_64" ] &&
  [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
  arch="aarch64"
fi
asset="fdt-${arch}-${os}.tar.gz"

if [ -n "$version" ]; then
  case "$version" in v*) ;; *) version="v${version}" ;; esac
  base="${releases}/download/${version}"
else
  base="${releases}/latest/download"
fi

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL --retry 3 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q -O "$2" "$1"; }
else
  die "hace falta curl o wget para descargar fdt."
fi
if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | awk '{ print $1 }'; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | awk '{ print $1 }'; }
elif command -v openssl >/dev/null 2>&1; then
  sha256() { openssl dgst -sha256 "$1" | awk '{ print $NF }'; }
else
  die "hace falta sha256sum, shasum u openssl para verificar la descarga."
fi
command -v tar >/dev/null 2>&1 || die "hace falta tar para descomprimir fdt."

tmp="$(mktemp -d 2>/dev/null || mktemp -d -t fdt)"
trap 'rm -rf "$tmp"' EXIT
trap 'exit 1' INT TERM

echo "Descargando ${asset} (${version:-última versión})…"
fetch "${base}/${asset}" "${tmp}/${asset}" ||
  die "no se pudo descargar ${base}/${asset}; ¿existe esa versión?"
fetch "${base}/SHA256SUMS" "${tmp}/SHA256SUMS" ||
  die "no se pudo descargar ${base}/SHA256SUMS."

expected="$(awk -v a="$asset" '$2 == a || $2 == "*"a { print tolower($1); exit }' "${tmp}/SHA256SUMS")"
actual="$(sha256 "${tmp}/${asset}" | tr 'A-F' 'a-f')"
[ -n "$expected" ] || die "SHA256SUMS no trae la suma de ${asset}; no se instala."
[ "$expected" = "$actual" ] || die "la suma SHA256 de ${asset} no coincide con SHA256SUMS; no se instala."

tar -xzf "${tmp}/${asset}" -C "$tmp" fdt || die "${asset} no contiene el binario fdt."
mkdir -p "$dir" || die "no se pudo crear ${dir}; elige otro con FDT_INSTALL_DIR."
[ -w "$dir" ] || die "no tienes permiso de escritura en ${dir}; elige otro con FDT_INSTALL_DIR."
chmod 755 "${tmp}/fdt"
# Copy then rename, so a running fdt is never left half-written.
cp "${tmp}/fdt" "${dir}/.fdt.new" && mv -f "${dir}/.fdt.new" "${dir}/fdt"

installed="$("${dir}/fdt" --version 2>/dev/null)" || die "se instaló ${dir}/fdt pero no arranca en esta máquina."
echo "${installed} instalado en ${dir}/fdt"

case ":${PATH}:" in
  *":${dir}:"*) ;;
  *)
    echo
    echo "${dir} no está en tu PATH. Agrégalo y abre una terminal nueva:"
    case "${SHELL:-}" in
      */fish) echo "  fish_add_path ${dir}" ;;
      */zsh) echo "  echo 'export PATH=\"${dir}:\$PATH\"' >> ~/.zshrc" ;;
      */bash) echo "  echo 'export PATH=\"${dir}:\$PATH\"' >> ~/.bashrc" ;;
      *) echo "  echo 'export PATH=\"${dir}:\$PATH\"' >> ~/.profile" ;;
    esac
    ;;
esac
