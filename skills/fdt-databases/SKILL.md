---
name: fdt-databases
description: Crea una base de datos (Postgres o MongoDB) en Foundathyon con `fdt`, la liga a un proyecto y la conecta a un servicio con `fdt env set --from-db` sin que la contraseña pase por el contexto del agente. Actívala cuando la persona pida una base de datos, "conecta la app a Postgres/Mongo", DATABASE_URL o MONGO_URL, o migraciones contra la base de producción. Explica URL interna vs pública.
metadata:
  version: "0.3.1"
  owner: fundathyon
---

# fdt-databases

Lee antes `fdt-basics`. Regla central: **la contraseña no pasa por ti**.
`fdt db url` falla (exit 2) sin `--reveal`, y `fdt db credentials` sin
`--reveal` enmascara la contraseña (y ni siquiera la pide a la API). No uses
`--reveal` salvo petición explícita de la persona. Leer la contraseña
(`db credentials --reveal`, `db url --reveal`, `env set --from-db`) exige un
token de nivel `deploy` o más: con uno `read` sale exit 4 `cli.token_level`.

## Crear, ligar, conectar

```sh
fdt org usage --json                                   # cupo: deploy.databases {used, limit}
fdt service show tienda-web --json                     # el proyecto del servicio al que se conecta
fdt db create tienda-pg --project tienda --wait --if-not-exists --dry-run --json   # primero en seco
fdt db create tienda-pg --project tienda --wait --if-not-exists --json     # --engine mongo para MongoDB
fdt db show tienda-pg --json                           # sin contraseña
fdt env set DATABASE_URL --from-db tienda-pg -s tienda-web
fdt env list -s tienda-web --json                      # DATABASE_URL aparece enmascarada
```

- `--wait` (por defecto hasta 10 min) termina con `data.status` = `ready`. `failed` → exit 1 con `error.message`; `timeout` → `fdt db show tienda-pg --json` y espera, no recrees.
- Nombre: 3-32, `a-z0-9-`. Motor: `postgres` (por defecto) o `mongo`, si la organización lo ofrece (`engines` en `fdt db list --json`; si no, exit 2 antes de enviar). `--db-name` / `--db-user` eligen el nombre interno y el usuario (por defecto `fd_<org>_<nombre>`).
- `--wait` que agota `--timeout` sale con 8: la base sigue creándose.
- Si la base ya existe sin proyecto: `fdt db link tienda-pg --project tienda` (`db unlink` la saca; la base sigue existiendo).
- `--from-db` guarda la cadena como **secreto** y nunca la imprime. Con `--dry-run` no lee la contraseña (vale con un token `read` y no deja lectura de secreto en la auditoría). Para Mongo, nombra la variable que use la app (`MONGO_URL`, `MONGODB_URI`…).
- `env set` redespliega el servicio si ya tiene imagen: avisa si es producción.
- Sin cupo (`used >= limit`, o exit 6 `quota.deploy.databases`): no crees.
  Ofrece reutilizar una base existente (`fdt db list --json`, `fdt db link`)
  o que la persona suba de plan en el panel.

## Interna o pública

| | Interna (por defecto) | Pública (`--public`) |
|---|---|---|
| Quién la usa | Servicios de Foundathyon de la misma org (`internal_host`, p. ej. `k3f9x2qa-acme` o `db-<id>`) | Tu máquina, CI, herramientas externas |
| Cuándo | **Siempre para servicios desplegados** | Migraciones o inspección desde fuera |
| Requisito | ninguno | que la base tenga acceso público (`public_host` no vacío en `db show`) |

```sh
fdt env set DATABASE_URL --from-db tienda-pg -s tienda-web            # interna: lo normal
fdt env set DATABASE_URL --from-db tienda-pg --public -s tienda-web   # solo si el servicio debe salir por internet; evítalo
```

Sin acceso público, `--public` falla con exit 1 y "usa la cadena interna".
No hay comando para activar el acceso público: es del panel.

## Usar la base desde la máquina sin ver la clave

- `fdt db connect tienda-pg`: abre `psql`/`mongosh` ya conectado, pero **necesita terminal interactiva**: pídeselo a la persona.
- Migraciones: **tú no las corres contra producción** (no lees la URL). La
  receta es que corran al arrancar el servicio, con la `DATABASE_URL` interna:
  1. Añade las migraciones al repo con la herramienta del proyecto (Prisma,
     Alembic, golang-migrate, Knex…), idempotentes.
  2. Haz que el contenedor las ejecute antes de la app: en el `Dockerfile`,
     `CMD ["sh", "-c", "<migrar> && exec <arrancar la app>"]` (o un entrypoint).
  3. `fdt env set DATABASE_URL --from-db <base> -s <servicio>` **antes** del
     deploy: ese `env set` reinicia la imagen vieja, que aún no migra.
  4. Commit y push (con permiso), `fdt deploy <servicio> --wait`, y confirma en
     `fdt logs <servicio> --since 10m --json` que la migración corrió.
  Si la persona prefiere migrar desde su máquina, la base necesita acceso
  público (`public_host` en `fdt db show`; si está vacío se activa en el panel)
  y ella ejecuta en su terminal `DATABASE_URL=$(fdt db url <base> --reveal --public) <migrar>`.

## Borrar

`fdt db delete <base> --confirm <nombre exacto>` (también con `--dry-run`) destruye los datos y no hay
respaldos ni restauración en `fdt`. Solo con permiso explícito de la persona
para esa base, tras `--dry-run`.

## Cambiar usuario o contraseña (Postgres)

```sh
fdt db set-credentials tienda-pg --password-from-env NEW_PG_PASSWORD --confirm tienda-pg --dry-run --json
fdt db set-credentials tienda-pg --password-from-env NEW_PG_PASSWORD --confirm tienda-pg --json
fdt env set DATABASE_URL --from-db tienda-pg -s tienda-web     # las apps no se actualizan solas
```

Corta las sesiones abiertas: las apps fallan hasta que actualices su
`DATABASE_URL`. Solo con permiso explícito. También `--user <nuevo>` y
`--password-stdin`. No en Mongo.

## Lo que `fdt` no hace hoy

Respaldos, restauración, activar acceso público. Dilo; no lo improvises con `fdt api`.
