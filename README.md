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
# macOS y Linux (cualquier distro; x86_64 o arm64). Instala en ~/.local/bin, sin sudo.
curl -fsSL https://github.com/fundathyon/fdt-cli/releases/latest/download/install.sh | sh
```

```powershell
# Windows (PowerShell). Instala en %LOCALAPPDATA%\fdt\bin y lo agrega a tu PATH de usuario.
irm https://github.com/fundathyon/fdt-cli/releases/latest/download/install.ps1 | iex
```

Comprueba con `fdt --version`. Si el instalador avisa de que el directorio no
está en tu `PATH`, ejecuta la línea que te sugiere y abre una terminal nueva.

Opciones, como variables de entorno en los dos instaladores:

| Variable | Para qué |
|---|---|
| `FDT_INSTALL_DIR` | Otro directorio de instalación |
| `FDT_INSTALL_VERSION` | Una versión concreta, p. ej. `v0.1.0` (por defecto la última) |

Los instaladores verifican el SHA256 del archivo contra `SHA256SUMS` de la
release y no instalan nada si no coincide. También puedes descargar a mano el
archivo de tu plataforma desde
[Releases](https://github.com/fundathyon/fdt-cli/releases).

Plataformas: macOS (Apple silicon e Intel), Linux x86_64 y arm64 (binario
estático) y Windows x64. En otras, compílalo:
`cargo install --git https://github.com/fundathyon/fdt-cli`.

**Actualizar:** `fdt update` descarga la última release, verifica su SHA256 y
reemplaza el binario; `fdt update --check` solo avisa.

Autocompletado: `fdt completions zsh > ~/.zfunc/_fdt`.

## Primeros pasos

```sh
fdt login                 # muestra un código, abre el panel y espera tu aprobación
fdt use acme              # organización por defecto de esta máquina
cd ~/code/tienda-web
fdt link --project tienda --service tienda-web   # escribe .fdt.toml (el servicio tiene que ser del proyecto)
fdt status                # servicios, último deploy y bases del proyecto
```

Llevar un repo a producción:

```sh
fdt service create tienda-web --project tienda --repo acme/tienda-web --if-not-exists --dry-run   # URL y memoria que deja el plan
# (el Dockerfile ya debe estar en la rama: el commit del workflow lanza el primer build)
fdt service create tienda-web --project tienda --repo acme/tienda-web --if-not-exists
#   → servicio tienda-web-acme (https://tienda-web-acme.foundathyon.dev), workflow y secreto del registry instalados
git pull                        # el commit del workflow dejó tu copia local atrás
fdt db create tienda-pg --project tienda --wait --if-not-exists
fdt env set DATABASE_URL --from-db tienda-pg -s tienda-web   # la URL nunca se imprime
fdt deploy tienda-web --wait    # el primer build ya lo lanzó el commit del workflow; esto espera o redespliega
fdt logs tienda-web --since 10m
fdt service diagnose tienda-web      # si algo no levanta
```

`service create` hace lo mismo que el panel (`POST …/apps/{app}/deploy/configure`):

- **Subdominio**: el argumento es la primera parte; Core le añade el sufijo
  de la organización (`tienda-web` → `tienda-web-acme`), y `--dry-run`
  muestra la URL exacta. Sin argumento, Core lo genera a partir del repo
  (`<repo>-<8 caracteres al azar>`) y solo se sabe al crearlo. `fdt`
  rechaza antes de llamar un subdominio con caracteres fuera de `a-z 0-9 -`
  o con guion en un extremo; `www`, `api`, `admin`, `app`, `db`… son de
  Foundathyon, pero con el sufijo (`api-acme`) ya no chocan.
- **Por defecto**: `--port 3000`, `--memory 256` (alias `--size`; el máximo
  por servicio en Free y Starter), la rama por defecto del repo y
  `--workflow-mode commit` (como el panel: Core hace commit directo del
  workflow en la rama y guarda el secreto `FOUNDATHYON_REGISTRY_TOKEN`). Ese
  push **lanza el primer build**, así que sube el Dockerfile antes, y luego
  haz `git pull` porque tu copia local queda atrás. `--workflow-mode
  pull_request` abre un PR con el workflow (el primer build llega al
  fusionarlo) y `manual` no toca GitHub (luego `fdt service install-workflow`). La memoria se comprueba contra los
  tamaños del plan (`service_memory_mb_options`) antes de enviar: si no
  cabe, sale con 6 y `quota.deploy.service_memory`.
- **`--no-auto-deploy`**: el workflow no corre en cada push a la rama; solo
  despliega `fdt deploy`.
- Después, el servicio se nombra por su nombre (el de la app, es decir el del
  repo: `tienda-web`; otro con `--name`) o por su slug (`tienda-web-acme`).
- **Una app, un servicio**: si la app ya tiene servicio, `create` falla
  (`fdt.service_exists`, sale con 1: ver Códigos de salida); con `--if-not-exists` lo devuelve, aunque su
  subdominio lleve sufijo. La salida trae `created` y, al crear, `workflow`
  (`status`: `pull_request_open`, `committed`, `failed` o `not_installed`).
- Un token limitado a proyectos puede crear el servicio de una app de su
  proyecto (Core lo autoriza por la app).

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
  (`fdt token list`). Revocar la sesión (`fdt token revoke <id> --confirm
  <nombre>` desde otra sesión, o desde el panel) revoca también esos tokens;
  la salida trae `cascade_revoked`. Revocar una sesión, y sobre todo la que
  estás usando, exige `--confirm <nombre>` (su id si no tiene nombre), también
  con `--dry-run`. Un token `scoped` no lista tokens, pero se puede revocar
  a sí mismo por id (`fdt token revoke <id>`, el `token.id` de `fdt whoami`).
- Si Core responde 429 (`cli.rate_limited`, p. ej. muchos equipos detrás de
  la misma IP), `fdt login` espera lo que diga `Retry-After` (o
  `error.meta.retry_after`) y sigue: mientras espera la aprobación hasta que
  el código caduca, y al pedir el código hasta 4 intentos. Lo avisa por stderr.

Niveles de un token `scoped`: `read` (solo lectura, sin secretos), `deploy`
(lectura, `deploy`, `redeploy`, variables, `service update` y las credenciales
de bases: `db credentials --reveal`, `db url`, `env set --from-db`) y `admin`
(todo lo que puede su dueño en esa organización: también `rollback`, `service
restart|pause|resume|install-workflow`, crear y borrar, y las lecturas con
secretos de la organización, como invitaciones, tokens del registry o secretos
de CI). `db credentials` sin `--reveal` no pide la contraseña, así que funciona
con `read`. Un token limitado a
proyectos ve los listados filtrados (fdt lo avisa, y un `show` que no
encuentra algo dice que puede existir fuera del alcance del token) y no puede
crear proyectos ni bases ni listar miembros. Los 403 de un token traen
`error.scope` `cli.token_level`, `cli.token_scope` o `cli.token_forbidden` y
un `hint` con el nivel o alcance que falta. `--org <id>` de una organización
que el token no cubre llega a Core y sale con 4 (`cli.token_scope`), no con 5.

## Contexto

Cada comando trabaja sobre una organización, y algunos sobre un proyecto o un
servicio. Se eligen en este orden:

1. Flags: `--org` (o `FDT_ORG`), `--project`, el servicio como argumento o
   `-s/--service`. `-s` funciona en todos los comandos de servicio; el
   argumento, donde no se confunde con otro: en `env set|unset` solo si no
   puede ser una variable (`fdt env set tienda-web-acme FOO=1`: el slug lleva
   guion), y nunca en `env import`. En `deploys -s <svc> <id>`
   el argumento es el deployment.
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
| `fdt service list\|show\|create\|update\|delete` | Servicios de deploy (un servicio pausado se avisa: su URL responde 502). `delete` avisa de lo que queda en el repo de GitHub (workflow y secreto) y cómo quitarlo |
| `fdt service setup` | Solo lectura: workflow, Dockerfiles sugeridos, token vivo; `--write-workflow` lo escribe en el repo |
| `fdt service install-workflow` | Instala workflow y secreto en GitHub (`--mode pull_request\|commit`, por defecto el modo del servicio, o `commit` si es `manual`; avisa si es otro; commit dispara un build: haz `git pull`) |
| `fdt service rotate-registry-token` | Token `fdr_` nuevo; revoca el anterior. Exige `--confirm` y `--output` o `--reveal` |
| `fdt service restart\|pause\|resume` | Reiniciar sin deployment (sin `--wait`: la API no dice cuándo vuelve); pausar (0 contenedores, `--confirm`); reanudar (`--wait`) |
| `fdt service diagnose` | Por qué no despliega: checks del repo, workflow, imagen y deployment |
| `fdt deploy <svc>` · `redeploy` | `deploy` despliega la cabeza de la rama (`--rebuild` fuerza build; si da la misma imagen que corre, termina con 0 y `unchanged: true`); `redeploy` repite la imagen que corre ahora (tras un rollback, la del rollback). `--wait --timeout` sale con 7 si falla (con los comandos para ver por qué) y 8 si se acaba el tiempo |
| `fdt rollback <svc>` | Vuelve a una imagen anterior: `--tag`, `--digest sha256:<64 hex>` o `--to <deployment>` (live o superseded); `--wait` |
| `fdt deploys <svc> [<id>] [--job N]` | Historial, detalle con el build, log de un job. `<id>`: cualquier prefijo que no sea ambiguo |
| `fdt logs <svc>` | Últimas líneas (`--tail`, `--since`, `--until`: duración o fecha RFC 3339) |
| `fdt env list\|set\|unset\|import\|pull` | Variables; los secretos salen enmascarados. `list` sin variables lo dice; `unset` avisa de las que no existen; `set\|unset\|import --wait` espera al reinicio |
| `fdt db list\|show\|create\|delete` | Bases Postgres y MongoDB (`--engine` se comprueba contra los motores que ofrece la organización) |
| `fdt db url --reveal` · `credentials [--reveal]` | Contraseña y cadenas: solo con `--reveal` (`url` sin él sale con 2) |
| `fdt db set-credentials` | Cambia usuario/contraseña de Postgres (`--password-stdin\|--password-from-env`, `--confirm`) |
| `fdt db link\|unlink\|connect` | Base en un proyecto; abrir `psql`/`mongosh` |
| `fdt api <MÉTODO> <ruta>` | Llamada cruda autenticada (como `gh api`); `{org}`, `{project}`, `{service}` se rellenan. Con un error HTTP stdout queda vacío y el cuerpo va en `error.details.body` |
| `fdt skills [list\|add\|remove\|update]` · `completions` · `update` | Skills para agentes (`add all`, `remove all`; un nombre que no existe sale con 5), autocompletado, actualizar |

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

  `scope` es el `error.scope` de la API cuando lo trae. Los errores que
  decide `fdt` sin llamar a la API llevan uno estable `fdt.<kind>`
  (`fdt.not_found`, `fdt.ambiguous`, `fdt.confirmation_required`,
  `fdt.missing_input`, `fdt.usage`…); los que `fdt` comprueba en nombre de
  Core llevan el de Core (`deploy.subdomain_invalid`,
  `quota.deploy.service_memory`). Una respuesta sin `error.scope` (un 404 del
  gateway, `404 page not found`) sale sin `scope` y el mensaje dice que la
  ruta puede no estar expuesta. `hint` es el comando que lo arregla cuando se
  sabe.

## Códigos de salida

| Código | Significa | `error.kind` |
|---|---|---|
| 0 | Bien | |
| 1 | Error (API, red) y conflictos con lo que ya existe (`fdt.service_exists`, `deploy.slug_taken`) | `error`, `api`, `network`, `cancelled` |
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
- **Borrar cuesta**: `project|service|db delete`, `service pause`,
  `service rotate-registry-token`, `db set-credentials` y `token revoke` de
  una sesión exigen `--confirm <nombre exacto>` (en una terminal se puede
  teclear; si la entrada se cierra, cancela con 2 en vez de esperar). Qué
  valor: `project delete` el nombre del proyecto; `db delete` y
  `db set-credentials` el nombre de la base; `token revoke` el nombre del
  token (su id si no tiene); `service delete|pause|rotate-registry-token` el
  **slug** del servicio, que lleva el sufijo de la organización
  (`tienda-web-acme`, no `tienda-web`): `fdt service show tienda-web --json`
  y lee `slug` (o `error.hint`, que trae el valor exacto). Borrar
  una organización no existe en el CLI.
- **`--dry-run`** en las mutaciones muestra la petición (método, ruta,
  cuerpo sin secretos) sin enviarla, y falla donde fallaría la de verdad:
  pide el mismo `--confirm`, valida `--digest`, `--engine`, el subdominio y
  la memoria del plan. `env set --from-db --dry-run` no lee la contraseña.
- **Repetir no rompe**: `create --if-not-exists` devuelve el recurso que ya
  existe (`project create` sale con 2 y lista los candidatos si hay varios
  con ese nombre).
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
  `rollback`, `service resume`, `env set|unset|import` y `db create`; si se
  acaba el tiempo sale con 8.
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
| `FDT_RELEASES_URL` | URL base de las releases para `fdt update` y los instaladores (un espejo; por defecto `https://github.com/fundathyon/fdt-cli/releases`) |
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

## Publicar una versión

Sube `version` en `Cargo.toml`, haz merge y empuja un tag `vX.Y.Z` con la misma
versión: el workflow `release` compila, publica y prueba la instalación.
Detalles en [RELEASING.md](RELEASING.md).

## Licencia

[MIT](LICENSE).
