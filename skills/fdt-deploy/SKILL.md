---
name: fdt-deploy
description: Lleva un repo de GitHub a producción en Foundathyon de punta a punta con `fdt`: Dockerfile, proyecto, servicio, workflow de GitHub Actions, variables y secretos, `fdt deploy --wait` y verificación. Actívala cuando la persona diga "despliega esta app", "súbela a Foundathyon", "crea el servicio", "pon esto en producción" o quiera el primer deploy de un repo. Para redeploys de un servicio ya vivo basta fdt-basics.
metadata:
  version: "0.3.0"
  owner: fundathyon
---

# fdt-deploy: repo a producción

Flujo real: el build corre en **GitHub Actions del repo** (workflow con
`fundathyon/deploy-action`), sube la imagen al registry y Foundathyon la
despliega. `fdt` crea el servicio, instala el workflow y lanza/espera el deploy.
Lee antes `fdt-basics` (sesión, `--json`, códigos, seguridad).

Requisito: la organización tiene la GitHub App instalada y el repo accesible
(se vincula en el panel; no hay comando). Si `service create` falla con
exit 5 por el repo/app, díselo a la persona.

## 1. Preparar

```sh
fdt context --json && fdt whoami --json     # level debe ser admin para crear
fdt org usage --json                        # ¿hay cupo de servicios y memoria?
```

Mira el repo: ¿hay `Dockerfile`? ¿qué puerto escucha la app? ¿ruta de salud?
Sin Dockerfile, créalo en el paso 3 con la plantilla recomendada.

## 2. Proyecto y servicio

```sh
fdt project create tienda --if-not-exists --json
fdt service create api --project tienda --repo acme/tienda-api \
  --port 3000 --memory 512 --health-path /health --if-not-exists --dry-run --json   # revisa
fdt service create api --project tienda --repo acme/tienda-api \
  --port 3000 --memory 512 --health-path /health --if-not-exists --json
fdt link --project tienda --service api      # .fdt.toml; lo demás ya no necesita flags
```

- `api` es el slug (3-32, `a-z0-9-`). La URL la decide Core (`--dry-run` la anticipa por stderr); léela de `data.url`.
- `--memory` en MB (256, 512 o 1024 según plan; `--size` es alias). El puerto debe ser el que escucha el contenedor.
- Monorepo: `--dockerfile <ruta>` y `--build-context <carpeta>`. Rama: `--branch` (por defecto `main`).
- Estado inicial: `awaiting_first_deploy`.
- exit 6 `quota.deploy.services|service_memory|memory`: para e informa (ver `fdt-basics`).

## 3. Dockerfile y workflow

```sh
fdt service setup api --json
```

Lee `data.setup.recommended_dockerfile` (`template_id`, `warnings`) y el
`content` de esa plantilla en `data.setup.dockerfiles[]`; el puerto ya va
puesto. Si el repo no tiene Dockerfile, escribe ese contenido. Next.js exige
`output: "standalone"` en su config.

**Orden importante**: el workflow se dispara con cada push a la rama. Haz
commit y push del Dockerfile **antes** de instalar el workflow, o el primer
build fallará. Pide permiso a la persona para el commit/push si no lo pidió.

`fdt service setup` solo lee. Instalar es otro comando:

```sh
fdt service install-workflow api --mode pull_request --dry-run --json   # revisa
fdt service install-workflow api --mode pull_request --json   # por defecto: abre un PR
fdt service install-workflow api --mode commit --json         # commit directo a la rama
```

Salida: `data.workflow.status` = `committed` o `pull_request_open` (con
`data.workflow.pull_request_url`: la persona debe fusionarlo). Con `--mode
commit` el push dispara un build y tu copia local queda atrás: **haz `git pull`**
(fdt lo avisa por stderr). Si GitHub lo rechaza (exit 1, `error.details.workflow.error`:
`branch_protected`, `permission_missing`…), usa `--mode pull_request` o
`fdt service setup api --write-workflow` (escribe `.github/workflows/foundathyon.yml`
local; luego commit y push) y pide la secret `FOUNDATHYON_REGISTRY_TOKEN` a la persona.

No rotes el token del registry salvo que `diagnose` diga `registry_token`:
`fdt service rotate-registry-token api --confirm api --output <archivo>` revoca
el anterior y rompe el workflow hasta actualizar el secreto (o reinstalar el workflow).

## 4. Variables y secretos

```sh
fdt env set NODE_ENV=production LOG_LEVEL=info --service api
fdt env set STRIPE_KEY --secret --stdin --service api < ruta/al/archivo   # tú no lo lees
fdt env set STRIPE_KEY --secret --from-env STRIPE_KEY --service api      # de una variable de entorno local
fdt env set STRIPE_KEY --secret --from-file ruta/al/archivo --service api
fdt env import .env.production --service api --secret-key STRIPE_KEY --dry-run   # luego sin --dry-run
fdt env list --service api --json     # secretos enmascarados
```

- Valores no secretos pueden ir en `KEY=VALUE`. **Un secreto nunca en la línea de comandos** (`KEY=VALUE --secret`, `service create --secret K=V`): queda en el historial y en tu contexto, y fdt lo avisa. Usa `--stdin`, `--from-env` o `--from-file` (en `service create`: `--secret-from-env KEY[=VAR]` / `--secret-from-file KEY=RUTA`), o pide a la persona que ejecute `fdt env set KEY --secret` en su terminal (pregunta sin eco).
- Bases de datos: skill `fdt-databases` (`--from-db`).
- Cada cambio redespliega si ya hay imagen (la salida trae `redeploys` y `deployment`; con `--dry-run` también `redeploys`); fija las variables **antes del primer deploy**.

## 5. Deploy y verificación

```sh
fdt deploy api --dry-run --json
fdt deploy api --wait --timeout 15m --json
```

- `--dry-run` trae `data.prediction`: `would` = `workflow` (sin imagen o con `--rebuild`) o `redeploy_or_workflow` (Core decide según la cabeza de la rama; `certain: false`).
- Sin imagen aún, `deploy` lanza el workflow (`data.dispatched: true`, `reason: no_image|new_commits`) y `--wait` espera a que llegue la imagen y se despliegue. `--rebuild` fuerza un build nuevo.
- Si con `--wait` aparecen varios deployments a la vez (otro deploy en paralelo), la salida trae `ambiguous: true` y `candidates`: confirma con `fdt deploys api --json` cuál es el tuyo.
- Éxito: exit 0, `data.deployment.status` = `live`. `superseded` = otro deploy más nuevo lo reemplazó; revisa `fdt deploys api --json`.
- Exit 7 (falló) u 8 (`timeout`, sigue en curso): `fdt-troubleshoot`.
- Verifica: `fdt status --json`, `fdt logs api --since 5m --json` y, si hay `health_path`, un GET a `data.url` con curl.

Redeploy sin build: `fdt redeploy api --wait --json`. Volver a una imagen anterior: `fdt rollback api --to <deployment> --wait --json` (o `--tag`, `--digest`; pide admin).

## 6. Opcional: login en la app

Si la persona quiere usuarios, registro o login, usa la skill
`accounts-integration` (Foundathyon Accounts; no la copies aquí). Las claves
`secret_key` van como secretos del servicio (`--stdin`), nunca en el frontend.

## Si ya existe

Todo es idempotente: `--if-not-exists` devuelve lo existente; `service setup`
sin flags solo lee. Antes de crear nada, `fdt status --json` por si el
servicio ya está.
