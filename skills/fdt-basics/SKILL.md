---
name: fdt-basics
description: Reglas base para operar la infraestructura de una organización en Foundathyon con el CLI `fdt` (proyectos, servicios, deploys, logs, variables, bases de datos). Actívala siempre que la persona pida desplegar, ver el estado o los logs de un servicio, cambiar variables de entorno, crear o conectar una base de datos, o cualquier cosa de Foundathyon, aunque no diga "fdt". Cubre comprobar sesión y contexto, --json, códigos de salida, niveles de token y qué nunca hacer sin permiso explícito.
metadata:
  version: "0.3.0"
  owner: fundathyon
---

# fdt: reglas base

`fdt` opera una organización de Foundathyon. Es un cliente de la API de Core:
lo que no tiene comando se hace con `fdt api`, no con curl.

Skills hermanas: `fdt-deploy` (repo a producción), `fdt-databases` (bases),
`fdt-troubleshoot` (algo falla).

## Antes de tocar nada

```sh
fdt context --json     # a dónde apunta esta carpeta (sin red): org, proyecto, servicio, origen del token
fdt whoami --json      # usuario, token.kind (session|scoped), token.level (read|deploy|admin), caducidad
fdt status --json      # servicios con su último deploy y bases
fdt doctor --json      # si algo de lo anterior falla: API, sesión, org, .fdt.toml, llavero
```

- Sin sesión (exit 3): pide a la persona que ejecute `fdt login` o exporte
  `FDT_TOKEN`. **Nunca** le pidas que pegue un token en el chat.
- Sin organización activa: `fdt use <org>` o `--org <org>` en cada comando.
- Si `fdt context` no muestra proyecto o servicio y el repo es el de la tarea:
  `fdt link --project <p> --service <s>` (escribe `.fdt.toml`).

## Siempre

- `--json` en todo; lee **stdout** como datos. Mensajes y errores van a stderr
  (los errores también en JSON: `error.kind`, `error.scope`, `error.hint`).
- `--no-input`: nunca esperes un prompt; pasa todo por flags.
- Nombre, slug o id valen. Si el nombre es ambiguo (exit 2, `error.details.candidates`) usa el id.
- Antes de mutar algo que no tengas claro: `--dry-run` (muestra método, ruta y cuerpo sin secretos, no envía).
- Al reintentar un `create`: `--if-not-exists` (devuelve el existente, no duplica).
- Esperar es explícito: `deploy --wait --timeout 15m`, `redeploy|rollback|service resume --wait`, `db create --wait`.
- Nada imprime una contraseña o un token sin `--reveal` (`db url`, `db credentials`, `token create`, `service rotate-registry-token`).

## Códigos de salida

| Código | Significa | Qué hacer |
|---|---|---|
| 0 | bien | seguir |
| 1 | error de API o de red (`kind`: `api`, `network`) | lee `error.message`; reintenta una vez |
| 2 | uso incorrecto: falta un flag, nombre ambiguo, falta `--confirm` | corrige los flags; `error.hint` dice cuál |
| 3 | sin sesión | pide a la persona `fdt login` / `FDT_TOKEN` |
| 4 | sin permiso (`forbidden`; `error.scope` `cli.token_level`, `cli.token_scope` o `cli.token_forbidden`) | `error.hint` dice qué nivel o alcance falta; mira `fdt whoami --json` y "Niveles de token" |
| 5 | no existe | `fdt <recurso> list --json`; revisa `--org` y `--project` |
| 6 | cuota del plan (`error.scope` = `quota.*`) | `fdt org usage --json`; informa y para: no borres cosas ni busques atajos |
| 7 | el deploy falló (`--wait`) | skill `fdt-troubleshoot` |
| 8 | se acabó `--timeout` de `--wait` (`kind`: `timeout`) | sigue en curso: consulta `fdt deploys <svc> --json`; no repitas la mutación |

## Niveles de token

`fdt whoami --json` → `token.level`:

- `read`: solo lee lo que no es secreto (status, logs, `env list`, `deploys`, `db list|show`, `db credentials` sin `--reveal`).
- `deploy`: además `deploy`, `redeploy`, `env set|unset|import`, `service update` y las credenciales de bases (`db credentials --reveal`, `db url --reveal`, `env set --from-db`). No crea ni borra recursos, ni hace `rollback`, `service restart|pause|resume` o `service install-workflow` (esos piden `admin`).
- `admin`: todo lo que puede su dueño en esa org, también las lecturas con secretos de la org (invitaciones, tokens del registry, secretos de CI, credenciales de apps de Accounts o Cronify).
- Si `error.message` dice "needs a token of level X", `error.hint` ya trae el `--level X` que hace falta.
- `token.kind` = `session` (login de una persona) vale en todas sus orgs; `scoped` solo en la suya y quizá solo en ciertos proyectos (`project_ids`). Un token limitado a proyectos ve los listados filtrados (stderr lo avisa) y no puede crear proyectos, servicios ni bases, ni ver miembros (exit 4, `cli.token_scope`).

Ante un 403 / exit 4: no intentes otra ruta ni `fdt api` para saltarlo. Di qué
nivel hace falta y que la persona cree un token con `fdt token create <nombre>
--level <nivel> -p <proyecto> --expires-in-days 30 --output <archivo>` (lo
ejecuta ella: el token se muestra una sola vez y exige `--output` o `--reveal`).

Tokens y sesiones: un token creado con una sesión de `fdt login` lleva
`created_by_token_id` (`fdt token list --json`). Revocar esa sesión
(`fdt token revoke <id>` o desde el panel) revoca también esos tokens
(`cascade_revoked` en la salida); `fdt logout` revoca solo la sesión. Un token
`scoped` no lista tokens, pero puede revocarse a sí mismo por id
(`fdt token revoke <token.id de fdt whoami>`).

## Seguridad: reglas que no se negocian

1. **Borrar**: `project|service|db delete` exigen `--confirm <nombre exacto>`.
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
fdt logs api --since 10m --json            # data.lines[] {at, stream, text}; también --until o fechas RFC 3339
fdt env list --service api --json          # secretos enmascarados
fdt env set LOG_LEVEL=debug --service api --dry-run --json
fdt deploys api --json                     # historial; status, error_code
fdt org usage --json                       # consumo frente al plan
```
