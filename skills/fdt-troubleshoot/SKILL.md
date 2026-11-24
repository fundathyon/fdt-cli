---
name: fdt-troubleshoot
description: Diagnostica un deploy que falló o un servicio caído en Foundathyon con `fdt`: `service diagnose`, `deploys`, logs de los jobs del build en GitHub Actions y `logs` de ejecución. Incluye árbol de decisión por código de salida y `error.scope` (también `quota.*` y `cli.*`). Actívala cuando `fdt deploy --wait` salga con 7 u 8, un servicio no levante, se quede en awaiting_first_deploy, devuelva 5xx, o la persona pregunte "por qué no despliega" o "por qué está caído".
metadata:
  version: "0.3.0"
  owner: fundathyon
---

# fdt-troubleshoot

Solo lectura hasta entender la causa. Lee antes `fdt-basics`.

## Orden fijo

```sh
fdt service diagnose tienda-web --json      # 1. checks en orden; --refresh si cambió el repo hace <60 s
fdt deploys tienda-web --json               # 2. historial: status, source, error_code
fdt deploys tienda-web <id> --json          # 3. detalle (<id>: cualquier prefijo no ambiguo): data.deployment + data.build (run y jobs de Actions)
fdt deploys tienda-web <id> --job <job_id> --tail 300 --json   # 4. log del job fallido (data.text)
fdt logs tienda-web --tail 200 --since 15m --json              # 5. logs del contenedor: data.lines[] {at, stream, text}
fdt logs tienda-web --since 2026-11-23T10:00:00Z --until 2026-11-23T10:15:00Z --json   # una ventana concreta
```

`diagnose` devuelve `summary` (`action_required|warning|unknown|pending|ok`),
`next_step` (el `id` del primer check roto) y `checks[]` `{id, status, code, details}`.
**Arregla `next_step` primero**: los checks posteriores dependen de él.

## Qué hacer según el check

| `id` / `code` | Causa | Arreglo |
|---|---|---|
| `repo_access`: `installation_missing`, `repo_not_accessible`, `repo_archived` | la GitHub App no ve el repo | persona: panel de GitHub / Foundathyon |
| `workflow_present`: `not_found`, `no_workflows_dir` | falta el workflow | `fdt service install-workflow tienda-web --mode pull_request` (o `--mode commit` y luego `git pull`) |
| `workflow_present`: `pull_request_open` | el PR del workflow no está fusionado | la persona lo fusiona (`details.pull_request_url`) |
| `workflow_present`: `install_failed` | GitHub rechazó la instalación (`details.error_code`) | `fdt service install-workflow tienda-web --mode pull_request` o `fdt service setup tienda-web --write-workflow` + commit |
| `workflow_present`: `branch_not_found` | la rama del servicio no existe | `fdt service update tienda-web --branch <rama>` |
| `workflow_inputs`: `*_mismatch`, `inputs_missing`, `token_not_a_secret` | el YAML no coincide con el servicio | `fdt service setup tienda-web --write-workflow` y commit (reemplaza el archivo) |
| `workflow_trigger`: `branch_mismatch`, `no_push_trigger` | el `on:` no dispara con la rama | reescribir el workflow como arriba |
| `dockerfile_present`: `not_found`, `dockerfile_mismatch` | Dockerfile no está donde dice el servicio | crearlo, o `fdt service update tienda-web --dockerfile <ruta> --build-context <dir>` |
| `registry_token`: `never_created` / `created_never_used` | el secreto del repo no tiene el token | pedir permiso y `fdt service rotate-registry-token tienda-web --confirm tienda-web --output <archivo>` (revoca el anterior; la persona pone el archivo en el secreto `FOUNDATHYON_REGISTRY_TOKEN`), o reinstalar el workflow, que también guarda el secreto |
| `workflow_run`: `failed` | falló el build | pasos 3 y 4 de arriba |
| `workflow_run`: `never_ran` | Actions deshabilitado o sin push | `fdt deploy tienda-web --wait` lanza el workflow |
| `image_pushed`: `run_ok_but_no_image` | el build pasó pero no llegó imagen | revisa el log del job (login al registry) |
| `deployment`: `failed` | falló al arrancar | `details.phase` + `error_code` (abajo) |

`deployment.details.phase`: `pulling` (imagen/registry), `starting` (el
proceso muere: mira `fdt logs`), `health_check` (puerto o `--health-path`
incorrectos), `routing`. `error_code: internal` es de la plataforma, no del repo: avisa a la persona.

Causas típicas en `health_check`/`starting`: puerto distinto al de
`fdt service show tienda-web` (`fdt service update tienda-web --port N`), falta una
variable (`fdt env list -s tienda-web --json`), la app escucha en `127.0.0.1`
en vez de `0.0.0.0`, memoria corta (`--memory 1024`).

Tras corregir: `fdt deploy tienda-web --wait --timeout 15m --json` (o `fdt redeploy tienda-web --wait`
si solo cambiaron variables y la imagen es buena). **Un intento por causa
corregida**; no entres en bucle de redeploys.

Otras salidas, con permiso de la persona (piden token `admin`):
`fdt service restart tienda-web` (mismo contenedor, sin deployment; no sirve si está
pausado), `fdt rollback tienda-web --to <deployment-bueno> --wait --json` (volver a
una imagen que funcionaba), `fdt service resume tienda-web --wait` (si `status` es
`paused`).

## Árbol por código de salida

- **7** deploy falló: `deploys <svc> <id>` → `error_code`/`phase` → tabla de arriba.
- **8 `timeout`**: sigue en curso. `fdt deploys tienda-web --json`; no lances otro deploy encima.
- **1 `api`/`network`**: reintenta una vez; si persiste, `fdt doctor --json`.
- **2**: falta flag o nombre ambiguo; `error.hint`.
- **3**: sin sesión → persona. **4**: sin permiso → `error.scope` y `error.hint` (abajo), `fdt whoami --json` (`fdt-basics`).
- **5**: no existe → `fdt service list --json`, `--org`, `--project`. Con un token limitado a proyectos puede existir fuera de su alcance (el mensaje lo dice).
- **6 `quota.*`**: `fdt org usage --json`.

## `error.scope`

| scope | Qué es | Qué hacer |
|---|---|---|
| `quota.deploy.services` | tope de servicios | informa; no borres servicios para hacer sitio |
| `quota.deploy.memory` | memoria total del plan | baja `--memory` de otro servicio solo si la persona lo decide |
| `quota.deploy.service_memory` | memoria máxima por servicio | usa un valor menor |
| `quota.deploy.databases` | tope de bases | informa |
| `deploy.workflow_missing` | el workflow no está en la rama o no acepta `workflow_dispatch` | fusionar el PR o reinstalar: `fdt service install-workflow <svc> --mode pull_request` |
| `deploy.subdomain_invalid`, `deploy.slug_reserved`, `deploy.slug_taken` | subdominio de `service create` mal formado, de Foundathyon o en uso | otro subdominio (`--dry-run` muestra la URL final) |
| `fdt.service_exists` | `service create` de una app que ya tiene servicio | `--if-not-exists` lo devuelve; `fdt service update` lo cambia |
| `fdt.<kind>` (`fdt.not_found`, `fdt.ambiguous`, `fdt.confirmation_required`, `fdt.missing_input`, `fdt.usage`) | lo decidió fdt sin llamar a la API | `error.hint` |
| sin `scope`, 404 y "el gateway no la expone" | la ruta no llegó a Core (`fdt api` con una ruta mal escrita o no publicada) | revisa la ruta; es relativa a …/api/v1 |
| `deploy.*_not_found` | org/servicio inexistente | exit 5: lista y revisa `--org` |
| `cli.token_level` | el nivel del token no alcanza (p. ej. `deploy` intentando `rollback`, o `read` leyendo credenciales de una base: "needs a token of level deploy") | la persona usa un token del nivel que dice `error.hint` o su sesión |
| `cli.token_scope` | el token no cubre esa org o proyecto, o está limitado a proyectos y la acción es de toda la org | token de esa org sin `-p`, o la sesión |
| `cli.token_forbidden` | nada con un token `fdt_` lo permite (o un `scoped` gestionando tokens) | se hace en el panel o con la sesión |
| `cli.invalid_token` | token revocado (también en cascada al revocar la sesión que lo creó), caducado o mal formado: exit 3 | la persona crea otro o hace `fdt login` |
| `cli.invalid_project` | `project_ids` con un id que no es uuid o de otra org (422) | `fdt project list` y usa sus ids |
| `deploy.service_paused`, `deploy.not_paused` | restart de un pausado / resume de uno activo | `fdt service show tienda-web --json` y elige |

Para cualquier scope nuevo: lee `error.message` y `error.hint`; si no resuelven, informa con el JSON del error. Sin cuota tampoco se "arregla" con otra cuenta o `fdt api`.

## Informe

Entrega: causa (check/código/fase), evidencia (línea del log, sin secretos), qué
cambiaste, y qué falta de la persona. Los logs pueden contener secretos de la
app: cita solo la línea relevante.
