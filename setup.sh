#!/bin/sh
# Sets up fdt in one go on macOS and Linux: installs the CLI (with install.sh, so the SHA256 is
# checked), signs in and installs every agent skill globally (~/.claude/skills, ~/.agents/skills).
#
#   curl -fsSL https://github.com/fundathyon/fdt-cli/releases/latest/download/setup.sh | sh
#
# Variables (all optional), on top of the ones install.sh reads (FDT_INSTALL_DIR,
# FDT_INSTALL_VERSION, FDT_RELEASES_URL):
#   FDT_TOKEN       an fdt_… token: signs in with it instead of the browser (agents, CI)
#   FDT_NO_BROWSER  1 = do not open the browser; only print the URL and the code
#   FDT_SKIP_LOGIN  1 = do not sign in
#   FDT_SKIP_SKILLS 1 = do not install the skills
#   FDT_AGENTS      only these agents for the skills: claude, codex, cursor, copilot (comma-separated)
set -eu

releases="${FDT_RELEASES_URL:-https://github.com/fundathyon/fdt-cli/releases}"
releases="${releases%/}"
dir="${FDT_INSTALL_DIR:-$HOME/.local/bin}"
version="${FDT_INSTALL_VERSION:-}"

die() {
  echo "fdt: $*" >&2
  exit 1
}

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

tmp="$(mktemp -d 2>/dev/null || mktemp -d -t fdt)"
trap 'rm -rf "$tmp"' EXIT
trap 'exit 1' INT TERM

# 1. The CLI, with the installer of the same release.
echo "==> 1/3 Instalando fdt"
fetch "${base}/install.sh" "${tmp}/install.sh" || die "no se pudo descargar ${base}/install.sh."
FDT_INSTALL_DIR="$dir" FDT_INSTALL_VERSION="$version" FDT_RELEASES_URL="$releases" sh "${tmp}/install.sh" ||
  die "no se pudo instalar fdt."
# Call it by path: the install dir may not be in PATH yet.
fdt="${dir}/fdt"
[ -x "$fdt" ] || die "no se encontró ${fdt} después de instalar."

# 2. Sign in, unless there already is a session.
echo
echo "==> 2/3 Iniciando sesión"
if [ "${FDT_SKIP_LOGIN:-}" = "1" ]; then
  echo "Omitido (FDT_SKIP_LOGIN=1)."
elif [ -n "${FDT_TOKEN:-}" ]; then
  # Reads it from stdin so the token never shows up in the process list.
  token="$FDT_TOKEN"
  (unset FDT_TOKEN && printf '%s\n' "$token" | "$fdt" login --with-token --no-input) ||
    die "el token de FDT_TOKEN no sirvió para iniciar sesión."
  unset token
elif "$fdt" whoami --no-input >/dev/null 2>&1; then
  echo "Ya tienes sesión: no hace falta volver a entrar (fdt whoami)."
else
  # Under `curl … | sh` stdin is the script itself: the browser login needs the terminal.
  if ! (exec </dev/tty) 2>/dev/null; then
    die "no hay terminal para el login en el navegador; exporta FDT_TOKEN con un token fdt_… o ejecuta luego: ${fdt} login"
  fi
  if [ "${FDT_NO_BROWSER:-}" = "1" ]; then
    "$fdt" login --no-browser </dev/tty || die "no se completó el login; vuelve a intentarlo con: ${fdt} login"
  else
    "$fdt" login </dev/tty || die "no se completó el login; vuelve a intentarlo con: ${fdt} login"
  fi
fi

# 3. Every skill, for the user (not this folder).
echo
echo "==> 3/3 Instalando las skills para agentes (global)"
if [ "${FDT_SKIP_SKILLS:-}" = "1" ]; then
  echo "Omitido (FDT_SKIP_SKILLS=1)."
elif [ -n "${FDT_AGENTS:-}" ]; then
  "$fdt" skills add all --global --agent "$FDT_AGENTS" --no-input || die "no se pudieron instalar las skills."
else
  "$fdt" skills add all --global --no-input || die "no se pudieron instalar las skills."
fi

# 4. Welcome, and prompts to start with in an agent.
if [ -t 1 ]; then
  b="$(printf '\033[1m')" c="$(printf '\033[36m')" d="$(printf '\033[2m')" r="$(printf '\033[0m')"
else
  b="" c="" d="" r=""
fi

if [ "${FDT_SKIP_SKILLS:-}" = "1" ]; then
  agent_note="Instala antes las skills con: fdt skills add all --global"
else
  agent_note="Ya tiene las skills de Foundathyon: sabe desplegar, crear bases de datos, leer
logs e integrar el login de usuarios con Foundathyon Accounts."
fi

echo
echo "${b}✔ Todo listo. Empieza a usar Foundathyon.${r}"
case ":${PATH}:" in
  *":${dir}:"*) ;;
  *) echo "${d}(Abre una terminal nueva o agrega ${dir} a tu PATH para usar \`fdt\` sin la ruta completa.)${r}" ;;
esac
cat <<EOF

${b}Desde la terminal${r}
  fdt use <org>      elige tu organización
  fdt link           ata este repo a un proyecto y un servicio
  fdt status         servicios, deploys y bases de datos

${b}Con tu agente${r} (Claude Code, Codex, Cursor o Copilot)
${agent_note}
Abre tu proyecto, inicia el agente y pega uno de estos prompts:

${c}› Despliega esta app en Foundathyon: prepara el Dockerfile, crea el proyecto
  y el servicio, configura las variables y deja la URL funcionando.${r}

${c}› Crea una base de datos Postgres en Foundathyon, conéctala a mi servicio
  con DATABASE_URL y corre las migraciones.${r}

${c}› Crea una app nueva: una API con registro y login de usuarios usando
  Foundathyon Accounts (crea la app en Accounts, usa sus llaves y maneja
  el refresh token), y despliégala en Foundathyon.${r}

${c}› Revisa el estado de mis servicios en Foundathyon y dime si algo falla;
  si un deploy falló, encuentra la causa en los logs.${r}

${d}Docs: https://docs.foundathyon.com · Ayuda: hello@getfoundathyon.com${r}
EOF
