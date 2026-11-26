---
name: fdt-basics
description: Reglas base para operar la infraestructura de una organización en Foundathyon con el CLI `fdt` (proyectos, servicios, deploys, logs, variables, bases de datos). Actívala siempre que la persona pida desplegar, ver el estado o los logs de un servicio, cambiar variables de entorno, crear o conectar una base de datos, o cualquier cosa de Foundathyon, aunque no diga "fdt". Cubre comprobar sesión y contexto, --json, códigos de salida, niveles de token y qué nunca hacer sin permiso explícito.
metadata:
  version: "0.6.1"
  owner: fundathyon
---

# fdt: reglas base

`fdt` opera una organización de Foundathyon. Es un cliente de la API de Core:
nunca uses curl contra la API. `fdt api` es para las rutas que una skill nombra
(con el método y la ruta exactos); si no hay comando ni ruta documentada, di que
hoy no se puede en vez de buscarla.

Skills hermanas: `foundathyon` (mapa y app desde cero), `fdt-deploy` (repo a
producción), `fdt-databases` (bases), `accounts-integration` (código del
login con Accounts), `fdt-org` (plan, cuotas, miembros, tokens), `fdt-troubleshoot` (algo falla).

## Antes de tocar nada

```sh
fdt context --json     # a dónde apunta esta carpeta (sin red): org, proyecto, servicio, origen del token
fdt whoami --json      # usuario, token.kind (session|scoped), token.level (read|deploy|admin), caducidad
fdt status --json      # servicios con su último deploy y bases
fdt doctor --json      # si algo de lo anterior falla: API, sesión, org, .fdt.toml, llavero
```

- Si `fdt` no responde y en stderr aparece "Esperando al llavero del sistema":
  macOS pide permiso para leer el token (pasa tras instalar o actualizar fdt).
  Pide a la persona que acepte el aviso ("Permitir siempre") y reintenta; a
  los 60 s fdt sale con 3. En sesiones sin persona delante (CI, agentes en
  segundo plano), la alternativa es que la persona exporte `FDT_TOKEN`.
- Sin sesión (exit 3): pide a la persona que ejecute `fdt login` o exporte
  `FDT_TOKEN`. **Nunca** le pidas que pegue un token en el chat.
- Sin organización activa: `fdt use <org>` o `--org <org>` en cada comando.
- Si `fdt context` no muestra proyecto o servicio y el repo es el de la tarea:
  `fdt link --project <p> --service <s>` (escribe `.fdt.toml`).

## Siempre

- `--json` en todo; lee **stdout** como datos. Mensajes y errores van a stderr
  (los errores también en JSON: `error.kind`, `error.scope`, `error.hint`).
  Con un error stdout queda vacío, también en `fdt api` (el cuerpo va en
  `error.details.body`). `error.scope` es el de Core, o `fdt.<kind>` si lo
  decidió fdt sin llamar (`fdt.not_found`, `fdt.ambiguous`,
  `fdt.confirmation_required`…); una respuesta del gateway sin scope (404
  `page not found`) no trae `scope`.
- El servicio va como argumento o con `-s/--service` (vale en todos los
  comandos de servicio); en `env set|unset|import`, con `-s` (también vale el
  slug primero, `fdt env set tienda-web-acme FOO=1`, porque lleva guion y no
  puede ser un nombre de variable).
- `--no-input`: nunca esperes un prompt; pasa todo por flags.
- Nombre, slug o id valen. Si el nombre es ambiguo (exit 2, `error.details.candidates`) usa el id.
- Antes de mutar algo que no tengas claro: `--dry-run` (muestra método, ruta y cuerpo sin secretos, no envía). Falla donde fallaría la ejecución real: también pide `--confirm`.
- Al reintentar un `create`: `--if-not-exists` (devuelve el existente, no duplica; si hay varios con ese nombre, exit 2 con los candidatos).
- Esperar es explícito: `deploy --wait --timeout 15m`, `redeploy|rollback|service resume --wait`, `env set|unset|import --wait` (espera el reinicio `env_change` si lo hay), `db create --wait`. `service restart` no tiene `--wait`: la API no dice cuándo vuelve a responder; si necesitas esperar, `fdt redeploy --wait`.
- `fdt deploy` despliega la **cabeza de la rama**; `fdt redeploy` repite la **imagen que corre ahora** (después de un `rollback`, la del rollback; lo avisa y su `--dry-run` trae `data.image` y `data.newer_image`). Para volver a la última tras un rollback: `fdt deploy <svc> --wait`.
- Un servicio `paused` tiene 0 contenedores y su URL responde un 502 genérico: `status`, `service list|show` y `logs` lo avisan por stderr. Se reanuda con `fdt service resume <svc> --wait` (pide permiso).
- Nada imprime una contraseña o un token sin `--reveal` (`db url`, `db credentials`, `token create`, `service rotate-registry-token`, `accounts credentials`, `accounts keys create|rotate`).

## Códigos de salida

| Código | Significa | Qué hacer |
|---|---|---|
| 0 | bien | seguir |
| 1 | error de API o de red (`kind`: `api`, `network`), o un conflicto con lo que ya existe (`fdt.service_exists`, `deploy.slug_taken`) | lee `error.message`; reintenta una vez |
| 2 | uso incorrecto: falta un flag, nombre ambiguo, falta `--confirm` | corrige los flags; `error.hint` dice cuál |
| 3 | sin sesión | pide a la persona `fdt login` / `FDT_TOKEN` |
| 4 | sin permiso (`forbidden`; `error.scope` `cli.token_level`, `cli.token_scope` o `cli.token_forbidden`) | `error.hint` dice qué nivel o alcance falta; mira `fdt whoami --json` y "Niveles de token" |
| 5 | no existe | `fdt <recurso> list --json`; revisa `--org` y `--project`. Con un token limitado a proyectos puede existir fuera de su alcance (el mensaje lo dice) |
| 6 | cuota del plan (`error.scope` = `quota.*`) | `fdt org usage --json`; informa y para: no borres cosas ni busques atajos |
| 7 | el deploy falló (`--wait`) | `error.details.commands` trae los comandos exactos (`detail`: `fdt deploys <svc> <id>`, `logs`: `fdt logs <svc> --since <inicio>`); skill `fdt-troubleshoot` |
| 8 | se acabó `--timeout` de `--wait` (`kind`: `timeout`) | no es un fallo confirmado: lee `error.message` (dice qué vio del build) y consulta `fdt service diagnose <svc> --json` / `fdt deploys <svc> --json`; no repitas la mutación |

## Niveles de token

`fdt whoami --json` → `token.level`:

- `read`: solo lee lo que no es secreto (status, logs, `env list`, `deploys`, `db list|show`, `db credentials` sin `--reveal`).
- `deploy`: además `deploy`, `redeploy`, `env set|unset|import`, `service update` y las credenciales de bases (`db credentials --reveal`, `db url --reveal`, `env set --from-db`). No crea ni borra recursos, ni hace `rollback`, `service restart|pause|resume` o `service install-workflow` (esos piden `admin`).
- `admin`: todo lo que puede su dueño en esa org, también las lecturas con secretos de la org (invitaciones, tokens del registry, secretos de CI, credenciales de apps de Accounts o Cronify).
- Si `error.message` dice "needs a token of level X", `error.hint` ya trae el `--level X` que hace falta.
- `token.kind` = `session` (login de una persona) vale en todas sus orgs; `scoped` solo en la suya y quizá solo en ciertos proyectos (`project_ids`). Un token limitado a proyectos ve los listados filtrados (stderr lo avisa) y no puede crear proyectos ni bases, ni ver miembros (exit 4, `cli.token_scope`); sí el servicio de una app de su proyecto. Un `show` que no encuentra algo avisa de que puede existir fuera de su alcance.

Ante un 403 / exit 4: no intentes otra ruta ni `fdt api` para saltarlo. Di qué
nivel hace falta y que la persona cree un token con `fdt token create <nombre>
--level <nivel> -p <proyecto> --expires-in-days 30 --output <archivo>` (lo
ejecuta ella: el token se muestra una sola vez y exige `--output` o `--reveal`).

Tokens y sesiones: un token creado con una sesión de `fdt login` lleva
`created_by_token_id` (`fdt token list --json`). Revocar esa sesión
(`fdt token revoke <id> --confirm <nombre>` o desde el panel) revoca también
esos tokens (`cascade_revoked` en la salida); revocar una sesión, y sobre todo
la que estás usando, exige `--confirm` (pide permiso antes). `fdt logout`
revoca solo la sesión. Un token
`scoped` no lista tokens, pero puede revocarse a sí mismo por id
(`fdt token revoke <token.id de fdt whoami>`).

## Seguridad: reglas que no se negocian

1. **Borrar**: `project|service|db delete` (y `service pause`,
   `service rotate-registry-token`, `db set-credentials`, `token revoke` de una
   sesión) exigen `--confirm <nombre exacto>`, también con `--dry-run`. En los
   comandos de `service` el valor es el **slug** con el sufijo de la
   organización (`tienda-web-acme`, no `tienda-web`): sácalo de
   `fdt service show tienda-web --json` (`slug`); en `db`, el nombre de la base;
   en `project`, el del proyecto. Si fallas, `error.hint` trae el valor exacto.
   Antes de ejecutarlo, **pide permiso explícito a la persona** nombrando el
   recurso exacto y que es irreversible. Un "sí" a otra cosa, o a "limpia todo",
   no cuenta. Haz `--dry-run` primero. No existe borrar una organización.
2. **Secretos**: nunca imprimas ni pegues contraseñas, tokens ni cadenas de
   conexión. `fdt db url` y `fdt db credentials` solo las muestran con
   `--reveal`, y `fdt service rotate-registry-token` escribe el token con
   `--output <archivo>`: no uses `--reveal` salvo que la persona lo pida y sepa
   que el valor entra en tu contexto. Para variables secretas: `--from-db`,
   `--from-env <VARIABLE>`, `--from-file <ruta>` o `--stdin` desde un archivo
   que tú no abres. Un secreto en `KEY=VALUE --secret` queda en el historial
   (fdt avisa por stderr): no lo hagas.
3. **Efectos en producción**: `env set|unset|import` redespliega el servicio si
   ya tiene imagen y no está pausado (la salida y el `--dry-run` lo dicen en
   `redeploys`); `deploy`, `rollback` y `service restart|resume` cambian lo que
   corre; `service pause` lo deja sin contenedores (exige `--confirm`);
   `service update` cambia puerto/memoria. Di qué vas a cambiar y en qué servicio antes de hacerlo
   cuando la persona no lo pidió tal cual.
4. **Cuota**: `service create` y `db create` consumen plan. Comprueba
   `fdt org usage --json` antes de crear varios.
5. **Lo que no existe**: `logs -f` (en vivo), respaldos o restauración de
   bases, activar el acceso público de una base, desplegar un commit concreto
   (`deploy` usa la cabeza de la rama; para volver a una imagen anterior está
   `fdt rollback --tag|--digest|--to`). No los simules con `fdt api` a ciegas;
   dile a la persona que hoy no se puede.

## Recetas cortas

```sh
fdt logs tienda-web --since 10m --json            # data.lines[] {at, stream, text}; también --until o fechas RFC 3339
fdt env list -s tienda-web --json          # secretos enmascarados
fdt env set LOG_LEVEL=debug -s tienda-web --dry-run --json
fdt deploys tienda-web --json                     # historial; status, error_code
fdt org usage --json                       # consumo frente al plan
```
