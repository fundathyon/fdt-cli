# Pruebas de las skills con un agente

La meta de las skills es que una persona use Foundathyon casi siempre a través
de su agente. Estas pruebas lo comprueban: un agente con solo las skills de
`skills/` recibe un prompt real y tiene que llegar al final **sin mandar a la
persona al panel** ni adivinar.

Los cuatro prompts son los que `setup.sh` sugiere al terminar.

## Cómo correrlas

1. Compila (`cargo build`) y copia `target/debug/fdt` a una carpeta temporal.
2. Inicia sesión con una organización de pruebas (`fdt login`), nunca con una
   de clientes.
3. Por cada prompt, en una carpeta vacía (o con una app mínima), lanza un
   agente con estas reglas: lee `skills/foundathyon/SKILL.md` y sigue su
   enrutado; solo comandos de lectura y mutaciones con `--dry-run`; nada de
   `--reveal`, `login`, `token` ni `link`.
4. Pídele que informe: qué skills leyó y en qué orden, la secuencia de
   comandos, cada punto donde una skill fue ambigua, errónea o contradijo la
   salida real del CLI, si habría terminado sin el panel y qué cambiaría.
5. Corrige las skills (o el CLI) y repite el prompt que falló.

En una organización de pruebas se puede quitar el modo seguro para el prompt 1
y el 3 y comprobar el deploy de punta a punta.

## Prompts y lo que tiene que pasar

| # | Prompt | Skills | Debe pasar |
|---|---|---|---|
| 1 | Despliega esta app en Foundathyon: prepara el Dockerfile, crea el proyecto y el servicio, configura las variables y deja la URL funcionando. | `foundathyon` → `fdt-deploy` | Dockerfile, `project create --if-not-exists`, `service create`, `env set`, `deploy --wait`, la URL responde. Lo único fuera del agente: la GitHub App si no está instalada. |
| 2 | Crea una base de datos Postgres en Foundathyon, conéctala a mi servicio con DATABASE_URL y corre las migraciones. | `foundathyon` → `fdt-databases` | Revisa la cuota, `db create --wait`, `env set DATABASE_URL --from-db`, migraciones sin que la contraseña pase por el agente. |
| 3 | Crea una app nueva: una API con registro y login de usuarios usando Foundathyon Accounts (crea la app en Accounts, usa sus llaves y maneja el refresh token), y despliégala en Foundathyon. | `foundathyon` → `accounts-integration` → `fdt-deploy` | No crea la app por `curl POST /v1/apps`: pide a la persona que la cree en el panel y le pase `app_id`, publishable key y `ACCOUNTS_BASE_URL`; la secret key la guarda ella con `fdt env set --secret`. Código con signup/signin/refresh y deploy. |
| 4 | Revisa el estado de mis servicios en Foundathyon y dime si algo falla; si un deploy falló, encuentra la causa en los logs. | `foundathyon` → `fdt-troubleshoot` (+ `fdt-org` si hay cuota) | Solo lectura; informe con servicios, último deploy, causa de fallos y cuota relevante. |
