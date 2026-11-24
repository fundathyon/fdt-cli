# Changelog

Cambios de `fdt` por versión. Las notas de cada release en GitHub se generan
al publicar el tag; aquí queda lo que cambia para quien usa el CLI.

## [Unreleased]

- `deploy --rebuild --wait` ya no espera hasta el `--timeout` cuando el build produce la misma imagen que corre: lo detecta con el run y la imagen de `…/diagnostics` y termina con 0 y `unchanged: true`; si el build falla, sale con 7 con la URL del run, y el timeout dice qué vio del build ([#9](https://github.com/fundathyon/fdt-cli/pull/9)).
- `deploy|redeploy|rollback|service resume --wait` con exit 7: el error trae los comandos exactos para ver por qué (`fdt deploys <svc> <id>`, `fdt logs <svc> --since <inicio>`) en `error.details.commands` ([#9](https://github.com/fundathyon/fdt-cli/pull/9)).
- `redeploy` dice qué imagen va a redesplegar y avisa si hay una más nueva (tras un rollback) y cómo volver a ella (`fdt deploy`); la ayuda y las skills explican redeploy frente a deploy ([#9](https://github.com/fundathyon/fdt-cli/pull/9)).
- `rollback --to` rechaza un deployment que no terminó bien (failed, cancelled o en curso) con exit 2 ([#9](https://github.com/fundathyon/fdt-cli/pull/9)).
- `service delete` (también `--dry-run`) avisa de que en el repo de GitHub quedan `.github/workflows/foundathyon.yml` y el secreto `FOUNDATHYON_REGISTRY_TOKEN`, con los comandos para quitarlos (`github_leftovers`) ([#9](https://github.com/fundathyon/fdt-cli/pull/9)).
- `env set|unset|import --wait` espera al reinicio `env_change`; `env set|unset` aceptan el slug del servicio primero cuando no puede ser una variable ([#9](https://github.com/fundathyon/fdt-cli/pull/9)).
- `status`, `service list|show` y `logs` avisan de un servicio pausado (su URL responde 502); los errores de uso dicen qué valor falló y `--since` explica los formatos aceptados ([#9](https://github.com/fundathyon/fdt-cli/pull/9)).
- Skills `fdt-basics`, `fdt-deploy` y `fdt-troubleshoot` a 0.5.0 ([#9](https://github.com/fundathyon/fdt-cli/pull/9)).
