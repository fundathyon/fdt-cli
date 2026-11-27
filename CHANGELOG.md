# Changelog

Cambios de `fdt` por versión. Las notas de cada release en GitHub se generan
al publicar el tag; aquí queda lo que cambia para quien usa el CLI.

## [Unreleased]

- `fdt-deploy` (0.7.1): el modo `dockerize` ya no ejecuta Docker. Antes, si Docker estaba disponible, el agente corría `docker build` y `docker run` para probar el Dockerfile, y podía intentar arrancar Docker. El build real corre en GitHub Actions; si la persona quiere probar en local, el agente le da el comando ([#21](https://github.com/fundathyon/fdt-cli/pull/21)).

## [0.1.6] - 2026-11-26

- `fdt-deploy` (0.7.0) tiene modo `dockerize` (`/fdt-deploy dockerize`): el agente solo lee el repo y dice dónde está o dónde va el Dockerfile, el build context, el puerto, el host, la ruta de salud, el comando de arranque y las variables; revisa el Dockerfile regla por regla, propone los flags de `fdt service create` y solo escribe el Dockerfile y el `.dockerignore` si la persona dice que sí ([#18](https://github.com/fundathyon/fdt-cli/pull/18)).

## [0.1.5] - 2026-11-25

- Skills nuevas `foundathyon` (punto de entrada y app desde cero) y `fdt-org` (plan, cuotas, miembros y tokens); `fdt-basics` y `fdt-deploy` (0.6.1), `fdt-databases` (0.3.1) y `fdt-troubleshoot` (0.5.1) remiten a ellas y recogen lo que encontraron agentes de prueba: receta de migraciones al arrancar el servicio, variables antes del primer build (`--workflow-mode manual`), repo en GitHub como requisito, detectar el puerto y las variables, `fdt status` como primer paso y cuándo vale `fdt api`. `accounts-integration` a 0.33.1: en la plataforma la app se crea en el panel y la secret key la guarda la persona (`fdt` no opera Accounts) ([fundathyon/accounts#215](https://github.com/fundathyon/accounts/pull/215)) ([#15](https://github.com/fundathyon/fdt-cli/pull/15)).
- Si macOS tiene pendiente el aviso de acceso al llavero (pasa tras instalar o actualizar fdt), fdt ya no se queda colgado sin decir nada: a los 3 s avisa por stderr de que hay que aceptarlo o usar `FDT_TOKEN`, y a los 60 s sale con 3 ([#15](https://github.com/fundathyon/fdt-cli/pull/15)).
- `evals/agent-prompts.md`: cómo probar las skills con un agente y los cuatro prompts de `setup.sh` ([#15](https://github.com/fundathyon/fdt-cli/pull/15)).

## [0.1.4] - 2026-11-25

- `setup.sh`: instala el CLI, inicia sesión y deja todas las skills en tu usuario en un solo comando (`curl -fsSL …/releases/latest/download/setup.sh | sh`); al terminar da la bienvenida y sugiere prompts para empezar con tu agente (desplegar, crear una base de datos, una app con Foundathyon Accounts, revisar deploys); se publica con cada release ([#12](https://github.com/fundathyon/fdt-cli/pull/12)).
- `skills add|update` ya no se cortan a la mitad si chocan con una skill que no instaló fdt: revisan antes de escribir, saltan la carpeta con el mismo nombre o la skill que declara el mismo `name:`, avisan dónde está e instalan el resto (`skipped` en `--json`) ([#12](https://github.com/fundathyon/fdt-cli/pull/12)).
- Nueva skill `accounts-integration` (0.33.0, de `fundathyon/accounts`): el agente sabe integrar signup, signin, tokens, OAuth, magic link, MFA, webhooks y RBAC de Foundathyon Accounts; `scripts/sync-accounts-skill.sh` la actualiza desde un checkout de accounts ([#12](https://github.com/fundathyon/fdt-cli/pull/12)).

## [0.1.3] - 2026-11-24

- `deploy --rebuild --wait` ya no espera hasta el `--timeout` cuando el build produce la misma imagen que corre: lo detecta con el run y la imagen de `…/diagnostics` y termina con 0 y `unchanged: true`; si el build falla, sale con 7 con la URL del run, y el timeout dice qué vio del build ([#9](https://github.com/fundathyon/fdt-cli/pull/9)).
- `deploy|redeploy|rollback|service resume --wait` con exit 7: el error trae los comandos exactos para ver por qué (`fdt deploys <svc> <id>`, `fdt logs <svc> --since <inicio>`) en `error.details.commands` ([#9](https://github.com/fundathyon/fdt-cli/pull/9)).
- `redeploy` dice qué imagen va a redesplegar y avisa si hay una más nueva (tras un rollback) y cómo volver a ella (`fdt deploy`); la ayuda y las skills explican redeploy frente a deploy ([#9](https://github.com/fundathyon/fdt-cli/pull/9)).
- `rollback --to` rechaza un deployment que no terminó bien (failed, cancelled o en curso) con exit 2 ([#9](https://github.com/fundathyon/fdt-cli/pull/9)).
- `service delete` (también `--dry-run`) avisa de que en el repo de GitHub quedan `.github/workflows/foundathyon.yml` y el secreto `FOUNDATHYON_REGISTRY_TOKEN`, con los comandos para quitarlos (`github_leftovers`) ([#9](https://github.com/fundathyon/fdt-cli/pull/9)).
- `env set|unset|import --wait` espera al reinicio `env_change`; `env set|unset` aceptan el slug del servicio primero cuando no puede ser una variable ([#9](https://github.com/fundathyon/fdt-cli/pull/9)).
- `status`, `service list|show` y `logs` avisan de un servicio pausado (su URL responde 502); los errores de uso dicen qué valor falló y `--since` explica los formatos aceptados ([#9](https://github.com/fundathyon/fdt-cli/pull/9)).
- Skills `fdt-basics`, `fdt-deploy` y `fdt-troubleshoot` a 0.5.0 ([#9](https://github.com/fundathyon/fdt-cli/pull/9)).
