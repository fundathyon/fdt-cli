---
name: fdt-deploy
description: Lleva un repo de GitHub a producción en Foundathyon de punta a punta con `fdt`: Dockerfile, proyecto, servicio, workflow de GitHub Actions, variables y secretos, `fdt deploy --wait` y verificación. Actívala cuando la persona diga "despliega esta app", "súbela a Foundathyon", "crea el servicio", "pon esto en producción" o quiera el primer deploy de un repo. Para redeploys de un servicio ya vivo basta fdt-basics.
argument-hint: "[dockerize]"
metadata:
  version: "0.7.0"
  owner: fundathyon
---

# fdt-deploy: repo a producción

Flujo real: el build corre en **GitHub Actions del repo** (workflow con
`fundathyon/deploy-action`), sube la imagen al registry y Foundathyon la
despliega. `fdt` crea el servicio, instala el workflow y lanza/espera el deploy.
Lee antes `fdt-basics` (sesión, `--json`, códigos, seguridad).

Argumento: si la persona invoca `/fdt-deploy dockerize` (o pide "¿está listo mi
repo para Docker?", "revisa mi Dockerfile", "dockeriza esto"), haz **solo** la
sección "Modo dockerize" y para. Sin argumento, sigue el flujo completo.

## Modo dockerize: revisar y recomendar

Solo lee el repo: no crea proyecto ni servicio, no llama a la API, no escribe
archivos ni hace commit hasta que la persona lo apruebe. No hace falta sesión
de `fdt`.

1. **Detecta** leyendo el repo (no inventes; si algo no se puede saber, dilo):
   - Stack y versión: `package.json` (+ lockfile: npm, pnpm, yarn, bun),
     `go.mod`, `requirements.txt`/`pyproject.toml`, `Cargo.toml`, `pom.xml`/
     `build.gradle`, `Gemfile`, `composer.json`; versión de `.nvmrc`,
     `engines`, `go` del go.mod, `python_requires`…
   - Monorepo: workspaces, `apps/*`, `packages/*`, varios `go.mod`. Decide
     qué carpeta es la app y si el build necesita la raíz (paquetes compartidos).
   - Dockerfile: busca `Dockerfile*` y `*.dockerfile` en todo el repo (sin
     `node_modules`, `vendor`, `target`). Anota ruta y build context.
   - Puerto: `listen(`, `.Listen(`, `bind`, `uvicorn --port`, `PORT`, `EXPOSE`,
     scripts `start` del package.json, defaults del framework (Next 3000, Vite
     preview 4173, Flask 5000, Django 8000, Rails 3000). Si lee `$PORT`, dilo.
     Si las fuentes no coinciden, manda lo que de verdad arranca (el código y
     el `CMD`); un `EXPOSE` distinto es un ❌.
   - Host: juzga el comando que corre en el contenedor (el `CMD`, o el que
     vas a proponer si no hay Dockerfile), no `python main.py` ni otro.
     Escuchan en `0.0.0.0` por defecto: `next start`. En `127.0.0.1` por
     defecto (hay que pasar el host): `uvicorn` y `flask run` sin `--host`,
     `vite preview` sin `--host`. Next standalone (`node server.js`) usa
     `HOSTNAME`: pon `ENV HOSTNAME=0.0.0.0`.
   - Salud: rutas tipo `/health`, `/healthz`, `/api/health`, `/status`.
   - Arranque: comando de producción (`start`, binario, `gunicorn`…), no el de dev
     (`next dev`, `npm run dev`, `--reload`, `flask run --debug`).
   - Variables: las que lee el código (`process.env.X`, `os.Getenv`,
     `os.environ`, `.env.example`). Marca cada una: secreta, con default u
     obligatoria. Secreta = el nombre lleva `KEY`, `SECRET`, `TOKEN`,
     `PASSWORD`, o es una URL con credenciales (`DATABASE_URL`, `REDIS_URL`). `PORT` no va: la fija la plataforma. `NEXT_PUBLIC_*` (y
     `VITE_*`) se meten en el JS al hacer el build: si la página no es dinámica,
     van como `ARG` en la etapa de build; son públicas, no secretas.
   - Base de datos o migraciones al arrancar (Prisma, Drizzle, Alembic,
     goose…): ver `fdt-databases`.

2. **Revisa el Dockerfile** si existe, regla por regla (✅/⚠️/❌):
   - Construye la app en modo producción y el `CMD` arranca el comando de
     producción.
   - `EXPOSE` coincide con el puerto que escucha la app.
   - Escucha en `0.0.0.0`.
   - `.dockerignore` existe y deja fuera `.git`, `node_modules`, `.env*`,
     artefactos de build.
   - Sin secretos en `ENV`/`ARG`/`COPY .env`. Si ya hay uno en un archivo
     versionado, está en el historial de git: di que hay que **rotarlo**, no
     solo quitarlo, y que va como secreto del servicio (`fdt env set K --secret
     --stdin`).
   - Dependencias reproducibles: con lockfile, `npm ci` / `pnpm install
     --frozen-lockfile`; copia los manifiestos e instala **antes** de `COPY . .`
     (caché de capas).
   - Imagen base oficial y con versión fija (no `latest`) que coincida con la
     del repo (`.nvmrc`, `.python-version`…); multi-stage si hay paso de build;
     no corre como root si la imagen lo permite.
   - Next.js: `output: "standalone"` en `next.config.*` y `COPY` de `public/`
     solo si la carpeta existe (si no, el build falla).
   - Cabe en memoria: el plan Free/Starter da 256 MB. Di "debería bastar" para
     un solo proceso ligero; avisa si es pesado (Java sin `-Xmx`, Next sin
     standalone, varios procesos) y que solo se sabe midiendo.
   - Rutas: `COPY` relativas al build context que vas a recomendar.

   Sin Dockerfile no repitas regla por regla: la línea "Dockerfile" ya dice
   que falta y lleva un resumen de una línea del que propones; la revisión es
   solo lo demás que le falta al repo (`.dockerignore`, `standalone`, ruta de
   salud…).

3. **Responde con este informe**, en el idioma de la persona. Corto: en
   "Revisión" solo lo que no pasa, ❌ si rompe el deploy o la seguridad (no
   arranca, puerto u host mal, modo dev, secreto, `latest`, falta
   `.dockerignore`), ⚠️ si es una mejora (caché, root, `npm ci`); nada de ✅
   ni de reglas que no aplican. Si el Dockerfile está mal, escribe "hoy → debe":

   ```
   Stack:          Next.js 15 (pnpm), Node 22
   Dockerfile:     ./Dockerfile (existe) | no existe → propongo ./Dockerfile
   Build context:  .            (monorepo: --dockerfile apps/web/Dockerfile --build-context .)
   Puerto:         3000 (lee $PORT; next start)   → --port 3000   | EXPOSE 8080 → 3000 ❌
   Host:           0.0.0.0 ✅    | CMD con --host 127.0.0.1 → 0.0.0.0 ❌
   Health:         /api/health  → --health-path /api/health  | no hay: propongo crearla
   Arranque:       node server.js   | hoy npm run dev ❌
   Memoria:        256 MB debería bastar
   Variables:      DATABASE_URL (secreta), NEXT_PUBLIC_API_URL (build), LOG_LEVEL (default info)
   Repo:           acme/tienda-web en GitHub (git remote) ✅; la GitHub App se comprueba en el panel | sin remoto: hay que crearlo y subirlo
   Revisión:
     ❌ ENV STRIPE_KEY=… en el Dockerfile → quitarlo, rotar la llave, fdt env set --secret
     ⚠️ npm install → npm ci
   ```

   Cierra con:
   - Los flags para `fdt service create`: siempre `--port` y `--health-path`;
     `--dockerfile`/`--build-context` solo si no son `./Dockerfile` y `.`;
     `--memory` solo si no basta con 256.
   - El orden: el Dockerfile tiene que estar en la rama (commit y push) antes
     de crear el servicio, porque el workflow lanza el primer build con ese push.
   - Una pregunta con los archivos exactos que tocarías: ¿escribo/arreglo el
     `Dockerfile`, el `.dockerignore` (y `next.config.*`, la ruta de salud…)?

4. **Si dice que sí**: escribe o corrige esos archivos (si ya hay servicio,
   parte de la plantilla de `fdt service setup <svc> --json`,
   `data.setup.recommended_dockerfile`). `.dockerignore` con `.git`, `.env*`
   (y `!.env.example` si la app lo lee) más lo del stack: Node `node_modules`,
   `.next`, `dist`; Python `__pycache__`, `.venv`; Go/Rust el binario o
   `target`. Si Docker está disponible, compruébalo
   con `docker build -f <dockerfile> <context>` y, si hay tiempo, `docker run
   -p <puerto>:<puerto>` + un GET a la ruta de salud; di si no pudiste. Commit
   y push solo con permiso explícito; recuerda que con el workflow ya instalado
   ese push dispara un build.

Requisito: la organización tiene la GitHub App instalada y el repo accesible
(se vincula en el panel; no hay comando). Si `service create` falla con
exit 5 por el repo/app, díselo a la persona.

## 1. Preparar

```sh
fdt context --json && fdt whoami --json     # level debe ser admin para crear
fdt org usage --json                        # ¿hay cupo de servicios y memoria?
```

Antes de nada, el código tiene que estar en un repo de GitHub con la GitHub
App de Foundathyon instalada en la organización (eso se hace en el panel). Si
la carpeta no es un repo de git o no tiene remoto en GitHub, para: pide a la
persona el `owner/repo`, que lo cree y suba el código, y que instale la App.
No crees el servicio hasta entonces.

Mira el repo: ¿hay `Dockerfile`? ¿qué puerto escucha la app (busca `listen`,
`bind`, `PORT`, `EXPOSE`; pásalo con `--port`, el defecto es 3000)? ¿ruta de
salud? ¿qué variables lee (`.env.example`, `process.env`, `os.environ`)? Si la
persona pide "configura las variables" sin decir cuáles, lista las que lee el
código y pregúntale los valores; no inventes ninguna.
Sin Dockerfile, escribe uno mínimo (imagen oficial del lenguaje, `EXPOSE` del
puerto, `CMD` que arranca la app) y súbelo (commit y push, con permiso) ANTES
del paso 2: el workflow que instala `service create` dispara el primer build
con ese push. Tras crear el servicio, compáralo con la plantilla de
`fdt service setup` (paso 3).

Si hay que fijar variables antes del primer build (llaves, `DATABASE_URL`),
crea el servicio con `--workflow-mode manual`, fija las variables (paso 4) y
luego instala el workflow (`fdt service install-workflow --mode commit`), que
lanza el build ya con todo.

## 2. Proyecto y servicio

```sh
fdt project create tienda --if-not-exists --json
fdt service create tienda-web --project tienda --repo acme/tienda-web \
  --health-path /health --if-not-exists --dry-run --json   # revisa url y service_memory_mb_options
fdt service create tienda-web --project tienda --repo acme/tienda-web \
  --health-path /health --if-not-exists --json
git pull                                     # el commit del workflow dejó tu copia local atrás
fdt link --project tienda -s tienda-web      # .fdt.toml; lo demás ya no necesita flags
```

- Crea el servicio como el panel (`POST …/apps/{app}/deploy/configure`). El
  argumento es la **primera parte del subdominio**: Core le añade el sufijo de
  la organización (`tienda-web` → slug `tienda-web-acme`). `--dry-run` trae la
  URL exacta en `data.url` (`url_certain: true`); sin argumento Core genera
  `<repo>-<8 al azar>` y no se sabe hasta crear. Lee siempre `data.url` y
  `data.slug` de la respuesta.
- Al servicio se le llama luego por su **nombre** (el de la app: el nombre del
  repo, `tienda-web`; cámbialo con `--name`) o por su **slug** (`tienda-web-acme`).
- Subdominio: `a-z0-9-`, sin guion en los extremos (exit 2 `deploy.subdomain_invalid`
  antes de llamar a la API). Nombres como `api` o `admin` son de Foundathyon, pero el argumento es solo un prefijo y el resultado lleva el sufijo de la organización (`api-acme`), así que no chocan.
- Por defecto: `--port 3000`, `--memory 256` (alias `--size`; 256 es el máximo
  de Free y Starter), la rama por defecto del repo. Si `--memory` no cabe en el
  plan sale con 6 `quota.deploy.service_memory` sin enviar nada. El puerto
  debe ser el que escucha el contenedor.
- `--workflow-mode`: `commit` (por defecto, como el panel: Core hace commit del
  workflow en la rama y guarda el secreto del registry; **ese push dispara el
  primer build**), `pull_request` (abre un PR; el build llega al fusionarlo) o
  `manual` (no toca GitHub; luego `fdt service install-workflow`). Por eso el
  **Dockerfile debe estar subido antes de crear el servicio**; si aún no lo
  está, usa `--workflow-mode manual` y sigue el paso 3. Tras `commit`, haz
  `git pull`. `data.workflow.status` dice qué pasó.
- `--no-auto-deploy`: el workflow no corre en cada push a la rama; solo con `fdt deploy`.
- Monorepo: `--dockerfile <ruta>` y `--build-context <carpeta>`.
- Una app tiene un solo servicio: si ya existe, `create` sale con 1
  (`fdt.service_exists`: conflicto con lo existente, no uso incorrecto); con `--if-not-exists` lo devuelve (`created: false`).
- Estado inicial: `awaiting_first_deploy`.
- exit 6 `quota.deploy.services|service_memory|memory`: para e informa (ver `fdt-basics`).

## 3. Dockerfile y workflow

```sh
fdt service setup tienda-web --json
```

Lee `data.setup.recommended_dockerfile` (`template_id`, `warnings`) y el
`content` de esa plantilla en `data.setup.dockerfiles[]`; el puerto ya va
puesto. Si el repo no tiene Dockerfile, escribe ese contenido. Next.js exige
`output: "standalone"` en su config.

**Orden importante**: el workflow se dispara con cada push a la rama. Si el
servicio ya se creó con el modo por defecto (`commit`), el workflow ya está en
la rama y el Dockerfile debía estar antes; si el primer build falló por eso,
sube el Dockerfile y lanza `fdt deploy tienda-web --rebuild --wait`. Con
`manual`, haz commit y push del Dockerfile **antes** de instalar el workflow.
Pide permiso a la persona para el commit/push si no lo pidió.

`fdt service setup` solo lee. Instalar es otro comando:

```sh
fdt service install-workflow tienda-web --mode commit --dry-run --json   # revisa
fdt service install-workflow tienda-web --mode commit --json         # commit directo a la rama
fdt service install-workflow tienda-web --mode pull_request --json   # abre un PR
```

Sin `--mode` usa el modo del servicio (o `commit` si es `manual`), y
avisa por stderr si pasas otro. Salida: `data.workflow.status` = `committed` o `pull_request_open` (con
`data.workflow.pull_request_url`: la persona debe fusionarlo). Con `--mode
commit` el push dispara un build y tu copia local queda atrás: **haz `git pull`**
(fdt lo avisa por stderr). Si GitHub lo rechaza (exit 1, `error.details.workflow.error`:
`branch_protected`, `permission_missing`…), usa `--mode pull_request` o
`fdt service setup tienda-web --write-workflow` (escribe `.github/workflows/foundathyon.yml`
local; luego commit y push) y pide la secret `FOUNDATHYON_REGISTRY_TOKEN` a la persona.

No rotes el token del registry salvo que `diagnose` diga `registry_token`:
`fdt service rotate-registry-token tienda-web --confirm tienda-web-acme --output <archivo>` revoca
el anterior y rompe el workflow hasta actualizar el secreto (o reinstalar el workflow).
`--confirm` (también en `service delete` y `service pause`) exige el **slug** completo, con el
sufijo de la organización: léelo con `fdt service show tienda-web --json` (campo `slug`);
si te equivocas, `error.hint` trae el valor exacto.

## 4. Variables y secretos

```sh
fdt env set NODE_ENV=production LOG_LEVEL=info -s tienda-web
fdt env set STRIPE_KEY --secret --stdin -s tienda-web < ruta/al/archivo   # tú no lo lees
fdt env set STRIPE_KEY --secret --from-env STRIPE_KEY -s tienda-web      # de una variable de entorno local
fdt env set STRIPE_KEY --secret --from-file ruta/al/archivo -s tienda-web
fdt env import .env.production -s tienda-web --secret-key STRIPE_KEY --dry-run   # luego sin --dry-run
fdt env list -s tienda-web --json     # secretos enmascarados
```

- Valores no secretos pueden ir en `KEY=VALUE`. **Un secreto nunca en la línea de comandos** (`KEY=VALUE --secret`, `service create --secret K=V`): queda en el historial y en tu contexto, y fdt lo avisa. Usa `--stdin`, `--from-env` o `--from-file` (en `service create`: `--secret-from-env KEY[=VAR]` / `--secret-from-file KEY=RUTA`), o pide a la persona que ejecute `fdt env set KEY --secret` en su terminal (pregunta sin eco).
- Bases de datos: skill `fdt-databases` (`--from-db`).
- Cada cambio redespliega si ya hay imagen (la salida trae `redeploys` y `deployment`; con `--dry-run` también `redeploys`); fija las variables **antes del primer deploy**. Con `--wait` espera a que ese reinicio (`env_change`) quede `live` (exit 7 si falla).
- El servicio va con `-s`; `fdt env set tienda-web-acme FOO=1` también vale (el slug lleva guion: no puede ser una variable).

## 5. Deploy y verificación

```sh
fdt deploy tienda-web --dry-run --json
fdt deploy tienda-web --wait --timeout 15m --json
```

- `--dry-run` trae `data.prediction`: `would` = `workflow` (sin imagen o con `--rebuild`) o `redeploy_or_workflow` (Core decide según la cabeza de la rama; `certain: false`).
- Sin imagen aún, `deploy` lanza el workflow (`data.dispatched: true`, `reason: no_image|new_commits`) y `--wait` espera a que llegue la imagen y se despliegue. `--rebuild` fuerza un build nuevo.
- `--rebuild --wait` cuando el build produce **la misma imagen** que ya corre: el registry no avisa y Core no crea deployment. fdt lo detecta (run nuevo terminado bien en `fdt service diagnose` y su imagen = la del deployment live) y sale con 0, `data.unchanged: true`, `data.unchanged_reason: "same_image"` y `data.deployment` = el live. Si la imagen del commit ya existía pero no es la que corre, sale con 1 y `error.hint` dice cómo desplegarla (`fdt deploy` sin `--rebuild`). Si el build falla, exit 7 con la URL del run.
- Si con `--wait` aparecen varios deployments a la vez (otro deploy en paralelo), la salida trae `ambiguous: true` y `candidates`: confirma con `fdt deploys tienda-web --json` cuál es el tuyo.
- Éxito: exit 0, `data.deployment.status` = `live`. `superseded` = otro deploy más nuevo lo reemplazó; revisa `fdt deploys tienda-web --json`.
- Exit 7 (falló): `error.details.commands` trae `fdt deploys <svc> <id>` y `fdt logs <svc> --since <inicio>`; luego `fdt-troubleshoot`. Exit 8 (`timeout`): `error.message` dice qué vio del build (en cola, corriendo, terminó sin imagen nueva).
- Verifica: `fdt status --json`, `fdt logs tienda-web --since 5m --json` y, si hay `health_path`, un GET a `data.url` con curl.

Redeploy sin build: `fdt redeploy tienda-web --wait --json` repite **la imagen que corre** (tras un rollback, la del rollback); `fdt deploy` vuelve a la cabeza de la rama. Volver a una imagen anterior: `fdt rollback tienda-web --to <deployment> --wait --json` (solo un deployment `live` o `superseded`: uno `failed` sale con 2; o `--tag`, `--digest`; pide admin).

Borrar el servicio (`fdt service delete … --confirm <slug>`, con permiso) no toca GitHub: en el repo quedan `.github/workflows/foundathyon.yml` y el secreto `FOUNDATHYON_REGISTRY_TOKEN` (revocado), y con auto deploy cada push correrá el workflow y fallará. La salida (y el `--dry-run`) trae `data.github_leftovers.remove` con los comandos (`git rm …`, `gh secret delete …`); pide permiso antes de ejecutarlos.

## 6. Opcional: login en la app

Si la persona quiere usuarios, registro o login: la app de Accounts la crea
ella en el panel (`fdt` no opera Accounts). Pídele el `app_id`, la publishable
key y `ACCOUNTS_BASE_URL`, guárdalos con `fdt env set`, y que ella guarde la
secret key con `fdt env set ACCOUNTS_SECRET_KEY --secret -s <servicio>` en su
terminal (nunca en el frontend ni por el chat). El código, con la skill
`accounts-integration`.

## Si ya existe

Todo es idempotente: `--if-not-exists` devuelve lo existente; `service setup`
sin flags solo lee. Antes de crear nada, `fdt status --json` por si el
servicio ya está.
