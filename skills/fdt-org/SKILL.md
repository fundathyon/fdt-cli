---
name: fdt-org
description: La organización de Foundathyon con `fdt`: plan, consumo y cuotas (`fdt org usage`), miembros, y tokens para agentes y CI (`fdt token`). Actívala cuando la persona pregunte cuánto lleva usado, qué incluye su plan, por qué algo falla con `quota.*` o exit 6, quién está en la organización, o pida un token para GitHub Actions, otro agente o un script.
metadata:
  version: "0.1.0"
  owner: fundathyon
---

# fdt-org: plan, cuotas, miembros y tokens

Reglas base (sesión, `--json`, códigos de salida, seguridad): skill `fdt-basics`.

## Plan y consumo

```sh
fdt org show --json     # nombre, id, my_role (owner|admin|member)
fdt org usage --json    # consumo del periodo frente al plan
```

`fdt org usage --json` trae `plan_name`, `plan_slug`, `period` (`AAAA-MM`) y un
bloque por producto. Cada contador es `{used, limit, over, overage_allowed, limit_key}`;
`limit: -1` significa sin límite.

| Bloque | Contadores |
|---|---|
| `deploy` | `services`, `services_memory_mb`, `databases`, `custom_domains` |
| `accounts` | `accounts_apps`, `accounts_users`, `mau`, `api_keys`, `oauth_providers`, `roles`, `webhooks` |
| `platform` | `projects`, `services`, `members`, `api_requests` |
| `crons` | `jobs`, `execution_minutes` (y `crons_paused`) |
| `monitoring` | `monitors` |
| `registry_storage` | `used_bytes`, `limit_bytes` (imágenes del registry) |

Al responder "¿cuánto llevo?" resume solo lo que la persona usa (lo que tiene
`used > 0` o está cerca del límite), con `used / limit`, y di el plan y el periodo.

## Cuando algo falla por cuota (exit 6, `error.scope` = `quota.*`)

1. `fdt org usage --json` y busca el contador con `used >= limit`.
2. Explica cuál es, en números, y qué lo consume (`fdt status --json`,
   `fdt db list --json`; las apps de Accounts se ven en el panel).
3. Para ahí. Las salidas son de la persona: subir de plan en el panel de
   Foundathyon o liberar algo que ya no use.
   **Nunca** borres, pauses ni reduzcas memoria de nada para hacer sitio sin
   que lo pida con el nombre exacto del recurso.

Errores habituales:

| `error.scope` | Qué pasa |
|---|---|
| `quota.deploy.services`, `quota.deploy.databases` | Deploy lleno: servicios o bases |
| `quota.deploy.memory` | La memoria total de los servicios superaría el plan |
| `quota.deploy.service_memory` | La memoria pedida para un servicio supera el máximo por servicio |
| `quota.users_exceeded` | La app de Accounts llegó al máximo de usuarios del plan |
| `quota.api_keys_exceeded`, `quota.roles_exceeded`, `quota.webhooks_exceeded`, `quota.oauth_providers_exceeded` | Accounts: ese contador del plan está lleno |
| `quota.tenant_read_only` | La organización quedó en solo lectura (pago o límite superado): solo se puede leer |
| `quota.feature_not_in_plan` | La función (p. ej. Embedded Auth, geo_access) es de un plan superior |

El error de fdt trae los números en `error.details.meta` (`limit_key`, `limit`, `used`).

## Miembros

```sh
fdt org members --json   # email, name, role (owner|admin|member), user_id
```

Invitar, cambiar el rol o quitar a alguien **no** está en `fdt`: se hace en el
panel (Organización → Miembros). Dilo así; no lo intentes con `fdt api`.

## Tokens para agentes y CI

Un token `fdt_…` con alcance sirve para GitHub Actions, otro agente o un
script. Lo crea la persona, porque el valor se muestra una sola vez:

```sh
fdt token create ci-tienda --level deploy -p tienda --expires-in-days 30 --output .fdt-token
fdt token list --json         # tus tokens, sin secreto
fdt token revoke ci-tienda    # revocar uno (una sesión exige --confirm)
```

- Nivel: el mínimo que funcione. `read` para mirar, `deploy` para desplegar y
  cambiar variables (lo normal en CI), `admin` solo si hay que crear o borrar
  recursos o leer credenciales de Accounts.
- Alcance: `-p <proyecto>` (se repite) para que no vea el resto de la organización.
- Caducidad corta (`--expires-in-days`, 1 a 365).
- Escríbelo con `--output <archivo>` (permisos 0600), nunca con `--reveal` en
  tu propio contexto. En GitHub, la persona lo guarda como secreto
  (`FDT_TOKEN`) y el workflow hace `echo "$FDT_TOKEN" | fdt login --with-token`
  o simplemente exporta `FDT_TOKEN`.
- **Nunca** pidas que te peguen un token en el chat.
