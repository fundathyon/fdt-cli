---
name: foundathyon
description: Punto de entrada para todo lo que tenga que ver con Foundathyon (plataforma para crear, desplegar y operar apps), aunque la persona no nombre ningún comando. Explica qué es cada producto (Deploy, bases de datos, Accounts para login de usuarios, Cronify), cómo empezar con `fdt` y qué skill usar para cada petición. Actívala cuando la persona mencione Foundathyon, pida crear una app desde cero, "empezar con Foundathyon", "qué puedo hacer en Foundathyon", o una tarea que cruce varios productos (una app con login y base de datos desplegada).
metadata:
  version: "0.1.0"
  owner: fundathyon
---

# Foundathyon

Foundathyon es una plataforma para crear y operar apps. Todo se maneja con el
CLI `fdt`, que habla con la API de la plataforma. Tú eres la forma principal
de usarla: la persona pide en lenguaje natural y tú ejecutas `fdt`.

## Mapa

```
Organización (plan, miembros, tokens)          → skill fdt-org
└── Proyecto
    ├── Servicio de deploy (app de un repo)    → skill fdt-deploy (y fdt-troubleshoot)
    ├── Base de datos Postgres o MongoDB       → skill fdt-databases
    └── App de Accounts (usuarios y login)     → skill accounts-integration (escribir el código)
                                                 la app se crea y configura en el panel
```

Reglas comunes a todo (sesión, `--json`, códigos de salida, permisos y qué no
hacer nunca): skill `fdt-basics`. Léela antes de ejecutar cualquier `fdt`.

## Qué skill usar

| La persona pide | Skill |
|---|---|
| Desplegar un repo, "súbelo", el primer deploy, redeploy, rollback | `fdt-deploy` |
| Algo falló, un servicio caído, un 5xx, exit 7 u 8 | `fdt-troubleshoot` |
| Una base de datos, `DATABASE_URL`, migraciones | `fdt-databases` |
| Usuarios, registro, login, OAuth, roles: el código de la app (signup, signin, refresh, middleware) | `accounts-integration` |
| Plan, consumo, cuotas (exit 6), miembros, tokens para CI | `fdt-org` |

## Antes de empezar

```sh
fdt whoami --json     # exit 3: pide a la persona `fdt login` (nunca un token por el chat)
fdt context --json    # organización, proyecto y servicio de esta carpeta
fdt status --json     # qué hay ya: servicios, deploys, bases
```

Si `fdt` no está instalado, la persona lo instala con:
`curl -fsSL https://github.com/fundathyon/fdt-cli/releases/latest/download/setup.sh | sh`.
Tras `fdt update`, las skills se ponen al día con `fdt skills update -g`.

## Una app desde cero

Cuando la persona pide "crea una app con login y despliégala" (o parecido),
sigue este orden y di en cada paso qué vas a crear:

1. **Organización y plan.** `fdt org usage --json`: confirma que caben un
   servicio, una base y una app de Accounts más. Si no, para y díselo (`fdt-org`).
2. **Proyecto.** `fdt project create <nombre> --if-not-exists --json` y
   `fdt link --project <nombre>` en el repo.
3. **Código.** La app en el repo, con un `Dockerfile` (`fdt-deploy`, paso 1).
   Si lleva login, escríbelo con `accounts-integration`.
4. **Accounts**, si hay usuarios: la app de Accounts la crea la persona en el
   panel (proyecto → nuevo servicio → Accounts); `fdt` no crea ni cambia apps de
   Accounts. Pídele el `app_id`, la publishable key y la URL base del servicio
   de Accounts (`ACCOUNTS_BASE_URL`, la muestra el panel); nunca le pidas la
   secret key por el chat.
5. **Base de datos**, si hace falta: `fdt db create <nombre> --engine postgres
   -p <proyecto> --wait` (`fdt-databases`).
6. **Servicio y deploy**: `fdt-deploy`. Crea el servicio con
   `--workflow-mode manual` para fijar las variables antes del primer build, y
   conecta lo anterior sin ver secretos:
   `fdt env set DATABASE_URL --from-db <base>`. Lo de Accounts:
   `fdt env set ACCOUNTS_APP_ID=<id> ACCOUNTS_PUBLISHABLE_KEY=<pk> ACCOUNTS_BASE_URL=<url> -s <servicio>`,
   y la secret key la guarda la persona en su terminal con
   `fdt env set ACCOUNTS_SECRET_KEY --secret -s <servicio>` (la pide sin eco). Después `fdt service install-workflow --mode
   commit` lanza el primer build ya con todo, y `fdt deploy --wait` lo sigue.
7. **Verificar**: la URL del servicio responde y, si hay login, un signup y un
   signin de prueba funcionan. Entrega la URL y lo que creaste.

## Lo que no se hace desde aquí

Crear el repo en GitHub y subir el código (pide permiso para cada commit y
push), instalar la GitHub App en la organización, invitar miembros, pagar o subir de
plan, y dominios propios se hacen en el panel de Foundathyon. Dilo así y sigue
con lo demás. Si un comando no existe, no lo simules con `fdt api` ni con
`curl` a ciegas: di que hoy no se puede.

Docs: https://docs.foundathyon.com · Soporte: hello@getfoundathyon.com
