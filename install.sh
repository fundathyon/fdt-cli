#!/bin/sh
# Installs the latest fdt CLI from GitHub Releases, checking its SHA256.
#   curl -fsSL https://github.com/fundathyon/fdt-cli/releases/latest/download/install.sh | sh
# FDT_INSTALL_DIR overrides the destination (default ~/.local/bin).
set -eu

repo="fundathyon/fdt-cli"
dir="${FDT_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)" in
  Linux) os="unknown-linux-musl" ;;
  Darwin) os="apple-darwin" ;;
  *) echo "fdt: sistema no soportado: $(uname -s) (en Windows usa install.ps1)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *) echo "fdt: arquitectura no soportada: $(uname -m)" >&2; exit 1 ;;
esac
asset="fdt-${arch}-${os}"
base="https://github.com/${repo}/releases/latest/download"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "Descargando ${asset}…"
curl -fsSL "${base}/${asset}" -o "${tmp}/fdt"
curl -fsSL "${base}/SHA256SUMS" -o "${tmp}/SHA256SUMS"

expected="$(awk -v a="$asset" '$2 == a || $2 == "*"a { print $1 }' "${tmp}/SHA256SUMS")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "${tmp}/fdt" | awk '{ print $1 }')"
else
  actual="$(shasum -a 256 "${tmp}/fdt" | awk '{ print $1 }')"
fi
if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
  echo "fdt: la suma SHA256 no coincide con SHA256SUMS; no se instala." >&2
  exit 1
fi

mkdir -p "$dir"
chmod 755 "${tmp}/fdt"
mv "${tmp}/fdt" "${dir}/fdt"
echo "fdt instalado en ${dir}/fdt: $("${dir}/fdt" --version)"
case ":$PATH:" in
  *":${dir}:"*) ;;
  *) echo "Agrega ${dir} a tu PATH, p. ej.: echo 'export PATH=\"${dir}:\$PATH\"' >> ~/.zshrc" ;;
esac
