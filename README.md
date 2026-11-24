# fdt

El CLI de Foundathyon. Maneja desde la terminal la infraestructura de una
organización: proyectos, servicios de deploy, deploys, logs, variables de
entorno y bases de datos. Está pensado para personas y para agentes de IA
(Claude Code, Codex, Cursor): `--json` en todo, códigos de salida estables,
nunca pregunta sin terminal y los secretos no salen salvo que se pidan.

`fdt` es un cliente delgado sobre la API de Core, detrás del api-gateway
público (`https://api.foundathyon.com/core/api/v1`).

## Instalar

```sh
# Linux y macOS (instala en ~/.local/bin; FDT_INSTALL_DIR para otro lugar)
curl -fsSL https://github.com/fundathyon/fdt-cli/releases/latest/download/install.sh | sh

# Windows (instala en %LOCALAPPDATA%\fdt\bin y lo agrega al PATH del usuario)
irm https://github.com/fundathyon/fdt-cli/releases/latest/download/install.ps1 | iex

# Desde el código
cargo install --path .
```

Los instaladores comprueban el SHA256 contra `SHA256SUMS` de la release.
`fdt update` reemplaza el binario por la última release (`fdt update --check`
solo avisa). Autocompletado: `fdt completions zsh > ~/.zfunc/_fdt`.

## Primeros pasos

```sh
fdt login                 # muestra un código, abre el panel y espera tu aprobación
fdt use acme              # organización por defecto de esta máquina
cd ~/code/tienda-api
fdt link --project tienda --service api   # escribe .fdt.toml en la raíz del repo
fdt status                # servicios, último deploy y bases del proyecto
```

Llevar un repo a producción:

```sh
fdt service create api --project tienda --repo acme/tienda-api --size 512 --port 3000 --if-not-exists
fdt service install-workflow api --mode pull_request   # workflow de GitHub Actions + secreto del registry
fdt db create tienda-pg --project tienda --wait --if-not-exists
fdt env set DATABASE_URL --from-db tienda-pg --service api   # la URL nunca se imprime
fdt deploy api --wait
fdt logs api --since 10m
fdt service diagnose api      # si algo no levanta
```

## Sesión y tokens

- **Personas**: `fdt login` usa el login por dispositivo: muestra un código `ABCD-EFGH`, abre
  `<panel>/cli?code=…` y consulta hasta que lo apruebas. `--no-browser` solo
  muestra la URL.
- **Agentes y CI**: exporta `FDT_TOKEN=fdt_…` (nunca se guarda), o
  `fdt login --with-token < archivo` para guardarlo. Crea uno con
  `fdt token create ci-tienda --level deploy -p tienda --expires-in-days 30 --output .fdt-token`
  (el token sale una sola vez: `--output` lo escribe con permisos 0600,
  `--reveal` lo imprime; sin ninguno de los dos no se crea).
- Los tokens van al **llavero del sistema** (Keychain en macOS, Credential
  Manager en Windows). Sin llavero (Linux, contenedores, `FDT_NO_KEYRING=1`)
  van a `~/.fdt/credentials.toml` con permisos `0600`. `~/.fdt/config.toml`
  solo guarda la organización activa y dónde está cada token, nunca el token.
- Un token `session` sirve en todas tus organizaciones; uno `scoped` se guarda
  para su organización y la deja como activa.
- `fdt logout` revoca el token en el servidor y lo borra de la máquina. Solo
  ese: los tokens que creaste con esa sesión siguen vivos.
- Un token creado con una sesión de `fdt login` lleva `created_by_token_id`
  (`fdt token list`). Revocar la sesión (`fdt token revoke <id>` desde otra
  sesión, o desde el panel) revoca también esos tokens; la salida trae
  `cascade_revoked`. Un token `scoped` no lista tokens, pero se puede revocar
  a sí mismo por id (`fdt token revoke <id>`, el `token.id` de `fdt whoami`).

Niveles de un token `scoped`: `read` (solo lectura, sin secretos), `deploy`
(lectura, `deploy`, `redeploy`, variables, `service update` y las credenciales
de bases: `db credentials --reveal`, `db url`, `env set --from-db`) y `admin`
(todo lo que puede su dueño en esa organización: también `rollback`, `service
restart|pause|resume|install-workflow`, crear y borrar, y las lecturas con
secretos de la organización, como invitaciones, tokens del registry o secretos
de CI). `db credentials` sin `--reveal` no pide la contraseña, así que funciona
con `read`. Un token limitado a
proyectos ve los listados filtrados (fdt lo avisa) y no puede crear
proyectos, servicios ni bases ni listar miembros. Los 403 de un token traen
`error.scope` `cli.token_level`, `cli.token_scope` o `cli.token_forbidden` y
un `hint` con el nivel o alcance que falta. `--org <id>` de una organización
que el token no cubre llega a Core y sale con 4 (`cli.token_scope`), no con 5.

## Contexto

Cada comando trabaja sobre una organización, y algunos sobre un proyecto o un
servicio. Se eligen en este orden:

1. Flags: `--org` (o `FDT_ORG`), `--project`, el servicio como argumento o `--service`.
2. `.fdt.toml` del repo (`fdt link`), buscado desde la carpeta actual hacia arriba.
3. La organización activa (`fdt use`).
4. Si perteneces a una sola organización, esa.

Todo acepta nombre, slug o id. Si un nombre coincide con varios recursos, el
comando falla con código 2 y lista los candidatos. `fdt context` muestra el
contexto sin llamar a la API; `fdt doctor` comprueba todo.

## Comandos

| Comando | Qué hace |
|---|---|
| `fdt login` · `logout` · `whoami` | Sesión: login por dispositivo o con token; quién eres y qué permite el token |
| `fdt token create\|list\|revoke` | Tokens `fdt_…` con organización, proyectos, nivel y caducidad |
| `fdt use <org>` | Organización por defecto de la máquina |
| `fdt link` · `unlink` | Ata la carpeta a org/proyecto/servicio en `.fdt.toml` |
| `fdt context` · `status` · `doctor` | Contexto local, resumen con servicios y bases, diagnóstico de la instalación |
| `fdt org list\|show\|usage\|members` | Organizaciones, detalle, consumo frente al plan, miembros |
| `fdt project list\|show\|create\|rename\|delete` | Proyectos (`rename` cambia nombre y/o `--description`) |
| `fdt service list\|show\|create\|update\|delete` | Servicios de deploy |
| `fdt service setup` | Solo lectura: workflow, Dockerfiles sugeridos, token vivo; `--write-workflow` lo escribe en el repo |
| `fdt service install-workflow` | Instala workflow y secreto en GitHub (`--mode pull_request\|commit`; commit dispara un build: haz `git pull`) |
| `fdt service rotate-registry-token` | Token `fdr_` nuevo; revoca el anterior. Exige `--confirm` y `--output` o `--reveal` |
| `fdt service restart\|pause\|resume` | Reiniciar sin deployment; pausar (0 contenedores, `--confirm`); reanudar (`--wait`) |
| `fdt service diagnose` | Por qué no despliega: checks del repo, workflow, imagen y deployment |
| `fdt deploy <svc>` · `redeploy` | Lanza un deploy (`--rebuild` fuerza build); `--wait --timeout` sale con 7 si falla y 8 si se acaba el tiempo |
| `fdt rollback <svc>` | Vuelve a una imagen anterior: `--tag`, `--digest` o `--to <deployment>`; `--wait` |
| `fdt deploys <svc> [<id>] [--job N]` | Historial, detalle con el build, log de un job |
| `fdt logs <svc>` | Últimas líneas (`--tail`, `--since`, `--until`: duración o fecha RFC 3339) |
| `fdt env list\|set\|unset\|import\|pull` | Variables; los secretos salen enmascarados |
| `fdt db list\|show\|create\|delete` | Bases Postgres y MongoDB |
| `fdt db url --reveal` · `credentials [--reveal]` | Contraseña y cadenas: solo con `--reveal` (`url` sin él sale con 2) |
| `fdt db set-credentials` | Cambia usuario/contraseña de Postgres (`--password-stdin\|--password-from-env`, `--confirm`) |
| `fdt db link\|unlink\|connect` | Base en un proyecto; abrir `psql`/`mongosh` |
| `fdt api <MÉTODO> <ruta>` | Llamada cruda autenticada (como `gh api`); `{org}`, `{project}`, `{service}` se rellenan |
| `fdt skills` · `completions` · `update` | Skills para agentes, autocompletado, actualizar |

## Salida

- Con `--json`, o cuando stdout no es una terminal, los datos salen como JSON
  en **stdout** con el mismo esquema que el `data` de la API. En una terminal,
  tablas.
- Mensajes y progreso van a **stderr**.
- Los errores salen por **stderr**: como JSON con `--json` o sin terminal,
  así:

  ```json
  {"ok": false, "error": {"kind": "quota", "exit_code": 6, "scope": "quota.deploy.services",
    "status": 403, "message": "Deploy service limit reached",
    "hint": "fdt org usage (y sube de plan en el panel)", "details": {"meta": {"limit": 1, "used": 1}}}}
  ```

  `scope` es el `error.scope` de la API; `hint`, el comando que lo arregla
  cuando se sabe.

## Códigos de salida

| Código | Significa | `error.kind` |
|---|---|---|
| 0 | Bien | |
| 1 | Error (API, red) | `error`, `api`, `network`, `cancelled` |
| 2 | Uso incorrecto: flag que falta, nombre ambiguo, falta `--confirm` | `usage`, `missing_input`, `ambiguous`, `confirmation_required` |
| 3 | Sin sesión o el login no se completó | `unauthenticated`, `login_failed` |
| 4 | Sin permiso | `forbidden` |
| 5 | No existe | `not_found` |
| 6 | Cuota del plan (`error.scope` empieza por `quota.`) | `quota` |
| 7 | El deploy falló (`--wait`) | `deploy_failed` |
| 8 | Se acabó el `--timeout` de `--wait` (sigue en curso) | `timeout` |

## Uso por agentes

Reglas que `fdt` cumple para que un agente lo use sin supervisión:

- **Nunca pregunta** sin terminal o con `--no-input`: si falta un dato falla
  con código 2 y dice qué flag falta.
- **Borrar cuesta**: `project|service|db delete` exigen `--confirm <nombre
  exacto>` (en una terminal se puede teclear). Borrar una organización no
  existe en el CLI.
- **`--dry-run`** en las mutaciones muestra la petición (método, ruta,
  cuerpo sin secretos) sin enviarla.
- **Repetir no rompe**: `create --if-not-exists` devuelve el recurso que ya
  existe.
- **Secretos fuera del chat**: `fdt env list` enmascara; `fdt env set KEY
  --from-db <base>` cablea una base sin imprimir la URL; `--stdin`,
  `--from-env VAR` y `--from-file RUTA` (en `service create`:
  `--secret-from-env`, `--secret-from-file`) leen un valor sin que quede en el
  historial, y fdt avisa por stderr cuando un secreto llega por la línea de
  comandos. Nada imprime una contraseña o un token sin `--reveal`
  (`db url`, `db credentials`, `token create`, `service rotate-registry-token`;
  los dos últimos aceptan `--output` a un archivo 0600). Los tokens nunca salen
  en errores ni logs.
- **Efectos visibles**: `env set|unset|import` dicen en `redeploys` (también
  con `--dry-run`) si el cambio reinicia el servicio: Core crea un deployment
  `env_change` cuando algo cambia y el servicio tiene imagen, salvo que esté
  pausado. `deploy --dry-run` trae `prediction` (workflow o redeploy, cuando
  se puede saber sin lanzar).
- **Esperar es explícito**: `--wait --timeout` en `deploy`, `redeploy`,
  `rollback`, `service resume` y `db create`; si se acaba el tiempo sale con 8.
  Esperar es por consulta periódica (el gateway corta cada petición a los
  30 s). Core no devuelve el id del run al lanzar el workflow: si aparecen
  varios deployments a la vez, la salida lo marca con `ambiguous: true`.

Skills para agentes (en `skills/`, embebidas en el binario):

```sh
fdt skills                     # disponibles y dónde están instaladas
fdt skills add all             # en .claude/skills y .agents/skills del repo
fdt skills add fdt-basics -g --agent claude   # en ~/.claude/skills
fdt skills update              # reinstala las que puso fdt
fdt skills add all --from ./skills            # desde una carpeta en vez del binario
```

## Variables de entorno

| Variable | Para qué |
|---|---|
| `FDT_TOKEN` | Token a usar (no se guarda) |
| `FDT_ORG` | Organización (como `--org`) |
| `FDT_API_URL` | URL de la API (como `--api-url`) |
| `FDT_NO_INPUT` | Como `--no-input` |
| `FDT_HOME` | Carpeta de configuración (por defecto `~/.fdt`) |
| `FDT_NO_KEYRING` | `1` guarda el token en archivo `0600` en vez del llavero |
| `FDT_RELEASE_REPO` | Repo de releases para `fdt update` (por defecto `fundathyon/fdt-cli`) |
| `NO_COLOR` | Sin colores |

## Lo que todavía no hay

Core aún no tiene rutas para: logs en vivo (`logs -f`; el gateway tampoco
deja pasar streaming), respaldos/restauración de bases, activar el acceso
público de una base, ni desplegar un commit concreto (`deploy` usa la cabeza de
la rama; `rollback` vuelve a imágenes que ya existen). `fdt` no los inventa:
cuando existan se añaden. Mientras, `fdt api` cubre cualquier ruta.

## Desarrollo

```sh
cargo build
cargo clippy --all-targets -- -D warnings
cargo test            # unitarias + tests/cli.rs (el binario real contra un Core simulado con wiremock)
```

`FDT_POLL_UNIT_MS` acorta las esperas (login por dispositivo, `--wait`) en
los tests.

## Licencia

[MIT](LICENSE).
