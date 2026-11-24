//! The `fdt` command tree and its dispatch.

use crate::commands::{self, db, deploy, env, misc, org, project, service, session};
use crate::context::Ctx;
use crate::failure;
use crate::ui;
use anyhow::Result;
use clap::{Args, CommandFactory, Parser, Subcommand};
use std::io::IsTerminal;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "fdt",
    version = crate::VERSION,
    about = "La infraestructura de tu organización en Foundathyon desde la terminal: proyectos, servicios, deploys, variables y bases de datos",
    after_help = "Árbol de comandos:\n\
\x20\x20sesión     login [--token|--with-token] [--no-browser] · logout · whoami · token create|list|revoke\n\
\x20\x20contexto   use <org> · link [-p] [-s] · unlink · context · status [-p] · doctor\n\
\x20\x20org        list · show · usage · members\n\
\x20\x20project    list · show · create [--if-not-exists] · rename · delete --confirm\n\
\x20\x20service    list [-p] · show · create [--repo|--app] [--if-not-exists] · update · delete --confirm · setup · diagnose\n\
\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20install-workflow · rotate-registry-token --confirm · restart · pause --confirm · resume [--wait]\n\
\x20\x20deploys    deploy [--rebuild] [--wait --timeout] · redeploy [--wait] · rollback --tag|--digest|--to [--wait]\n\
\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20deploys [<id>] [--job] · logs [--tail --since --until]\n\
\x20\x20env        list · set [--secret] [--stdin|--from-env|--from-file|--from-db] · unset · import <.env> · pull\n\
\x20\x20db         list · show · create [--engine --wait] · delete --confirm · url --reveal · credentials [--reveal]\n\
\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20set-credentials --confirm · link · unlink · connect\n\
\x20\x20otros      api <MÉTODO> <ruta> · completions <shell> · skills [list|add|remove|update] · update [--check]\n\
\x20\x20Las mutaciones aceptan --dry-run. Nada imprime una contraseña o un token sin --reveal. Detalle: fdt <comando> --help.\n\n\
Primeros pasos: fdt login · fdt use <org> · fdt link · fdt status\n\
Agentes y CI: exporta FDT_TOKEN con un token fdt_… (fdt token create) y usa --json.\n\
Códigos de salida: 0 bien, 1 error, 2 uso incorrecto, 3 sin sesión, 4 sin permiso, 5 no existe, 6 cuota del plan, 7 el deploy falló, 8 se acabó el --timeout de --wait.\n\
Skills para agentes: fdt skills."
)]
pub struct Cli {
    /// Salida JSON en stdout (los errores, también en JSON por stderr). Sin terminal ya es JSON.
    #[arg(long, global = true)]
    pub json: bool,
    /// Nunca pregunta: si falta un dato, falla diciendo qué flag falta. Sin terminal es lo mismo.
    #[arg(long, global = true, env = "FDT_NO_INPUT")]
    pub no_input: bool,
    /// Organización (nombre o id) en lugar de la de .fdt.toml o `fdt use`.
    #[arg(long, global = true, env = "FDT_ORG", value_name = "ORG")]
    pub org: Option<String>,
    /// URL de la API de Core (por defecto https://api.foundathyon.com/core/api/v1).
    #[arg(long, global = true, env = "FDT_API_URL", value_name = "URL")]
    pub api_url: Option<String>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Args, Clone, Copy)]
pub struct Wait {
    /// Espera a que termine; sale con 7 si el deploy falla y con 8 si se acaba --timeout.
    #[arg(long)]
    pub wait: bool,
    /// Con --wait, cuánto esperar como máximo (30s, 10m, 1h).
    #[arg(long, value_name = "DURACIÓN", default_value = "15m", value_parser = commands::parse_duration)]
    pub timeout: std::time::Duration,
}

#[derive(Subcommand)]
pub enum Command {
    // ------------------------------------------------------------ sesión
    /// Inicia sesión: login por dispositivo en el navegador, o --token / --with-token para agentes y CI.
    #[command(after_help = "Ejemplos:\n  fdt login\n  fdt login --no-browser\n  echo \"$FDT_TOKEN\" | fdt login --with-token\n\nEl token se guarda en el llavero del sistema (o en ~/.fdt/credentials.toml con permisos 0600 si no hay llavero).\nPara no guardar nada, exporta FDT_TOKEN.")]
    Login {
        /// Guarda este token fdt_… (mejor --with-token: así no queda en el historial del shell).
        #[arg(long, value_name = "TOKEN", conflicts_with = "with_token")]
        token: Option<String>,
        /// Lee el token de stdin.
        #[arg(long)]
        with_token: bool,
        /// No intenta abrir el navegador; solo muestra la URL y el código.
        #[arg(long)]
        no_browser: bool,
    },
    /// Cierra la sesión: revoca el token en el servidor y lo borra de esta máquina.
    Logout,
    /// Quién eres: usuario, token (clase, nivel, alcance, caducidad) y organización activa.
    Whoami,
    /// Tokens fdt_… para agentes y CI.
    Token {
        #[command(subcommand)]
        command: TokenCommand,
    },

    // ------------------------------------------------------------ contexto
    /// Fija la organización por defecto de esta máquina.
    Use {
        /// Nombre o id; sin él se pregunta (en una terminal).
        org: Option<String>,
    },
    /// Ata este repo a una organización, proyecto y servicio (escribe .fdt.toml).
    Link {
        /// Proyecto (nombre o id).
        #[arg(long, short = 'p')]
        project: Option<String>,
        /// Servicio de deploy (slug, nombre o id).
        #[arg(long, short = 's')]
        service: Option<String>,
    },
    /// Borra el .fdt.toml de este repo.
    Unlink,
    /// Hacia dónde apunta esta carpeta (sin llamar a la API): URL, token, org, proyecto, servicio.
    Context,
    /// Resumen del contexto: servicios con su último deploy y bases de datos.
    Status {
        /// Proyecto (por defecto el de .fdt.toml; sin él, toda la organización).
        #[arg(long, short = 'p')]
        project: Option<String>,
    },
    /// Comprueba la instalación: API, sesión, organización, .fdt.toml y llavero.
    Doctor,

    // ------------------------------------------------------------ recursos
    /// Organizaciones.
    Org {
        #[command(subcommand)]
        command: OrgCommand,
    },
    /// Proyectos de la organización.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Servicios de deploy.
    Service {
        #[command(subcommand)]
        command: Box<ServiceCommand>,
    },
    /// Lanza un deploy: redespliega la imagen de la rama si ya existe, o lanza el workflow de GitHub Actions.
    #[command(after_help = "Ejemplos:\n  fdt deploy api --wait --json\n  fdt deploy --wait --timeout 20m   # el servicio de .fdt.toml")]
    Deploy {
        /// Servicio (slug, nombre o id); por defecto el de .fdt.toml.
        service: Option<String>,
        /// Siempre lanza el workflow (build nuevo), sin reutilizar la imagen; saca de la pausa.
        #[arg(long)]
        rebuild: bool,
        #[command(flatten)]
        wait: Wait,
        /// Muestra la petición sin enviarla.
        #[arg(long)]
        dry_run: bool,
    },
    /// Redespliega la imagen actual (o la última que llegó) sin build.
    Redeploy {
        service: Option<String>,
        #[command(flatten)]
        wait: Wait,
        #[arg(long)]
        dry_run: bool,
    },
    /// Vuelve a una imagen anterior del servicio, por tag, digest o deployment.
    #[command(after_help = "Ejemplos:\n  fdt rollback api --tag v1.3.0 --wait\n  fdt rollback api --to 3f2a9c1d      # la imagen de ese deployment (fdt deploys api)\n  fdt rollback api --digest sha256:<64 hex> --dry-run")]
    Rollback {
        service: Option<String>,
        /// Tag de la imagen en el registry.
        #[arg(long, conflicts_with_all = ["digest", "to"])]
        tag: Option<String>,
        /// Digest de la imagen (sha256:<64 hex>).
        #[arg(long, conflicts_with = "to")]
        digest: Option<String>,
        /// Id (o prefijo) de un deployment anterior: se usa su digest.
        #[arg(long, value_name = "DEPLOYMENT")]
        to: Option<String>,
        #[command(flatten)]
        wait: Wait,
        #[arg(long)]
        dry_run: bool,
    },
    /// Historial de deploys de un servicio, o el detalle de uno (con --job, el log de ese job del build).
    Deploys {
        /// Servicio; por defecto el de .fdt.toml.
        service: Option<String>,
        /// Id del deployment (o sus primeros caracteres).
        deployment: Option<String>,
        /// Cuántos mostrar (máximo 50, lo que guarda la API).
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
        /// Con un deployment: id del job de GitHub Actions cuyo log mostrar.
        #[arg(long, requires = "deployment", value_name = "JOB_ID")]
        job: Option<i64>,
        /// Con --job: líneas del log (1-5000).
        #[arg(long, requires = "job", default_value_t = 1000)]
        tail: u32,
    },
    /// Logs de ejecución de un servicio (las últimas líneas; sin seguimiento en vivo).
    Logs {
        service: Option<String>,
        /// Líneas (1-1000).
        #[arg(long, default_value_t = 200)]
        tail: u32,
        /// Desde: una duración hacia atrás (30s, 10m, 2h, 1d) o una fecha RFC 3339.
        #[arg(long, value_parser = commands::parse_instant, value_name = "DURACIÓN|FECHA")]
        since: Option<chrono::DateTime<chrono::Utc>>,
        /// Hasta: una duración hacia atrás o una fecha RFC 3339.
        #[arg(long, value_parser = commands::parse_instant, value_name = "DURACIÓN|FECHA")]
        until: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// Variables de entorno de un servicio. La API reemplaza el conjunto: fdt lee, mezcla y escribe.
    Env {
        #[command(subcommand)]
        command: EnvCommand,
    },
    /// Bases de datos (Postgres, MongoDB).
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    /// Llamada cruda y autenticada a la API, como `gh api`. {org}, {project} y {service} se reemplazan por el contexto.
    #[command(after_help = "Ejemplos:\n  fdt api GET /organizations/{org}/apps\n  fdt api PATCH /organizations/{org}/deploy/services/{service} --data '{\"name\":\"API\"}'\n  fdt api POST /organizations/{org}/projects --input body.json")]
    Api {
        /// GET, POST, PUT, PATCH o DELETE.
        method: String,
        /// Ruta relativa a la URL de la API (p. ej. /organizations/{org}/usage).
        path: String,
        /// Cuerpo JSON en línea.
        #[arg(long, short = 'd', conflicts_with = "input")]
        data: Option<String>,
        /// Cuerpo JSON desde un archivo (- es stdin).
        #[arg(long)]
        input: Option<PathBuf>,
        /// Parámetro de query k=v (se puede repetir).
        #[arg(long = "query", short = 'q', value_name = "K=V")]
        query: Vec<String>,
    },

    // ------------------------------------------------------------ herramienta
    /// Script de autocompletado para tu shell.
    Completions { shell: clap_complete::Shell },
    /// Skills para agentes (Claude Code, Codex, Cursor): lista, instala, quita o actualiza.
    Skills {
        #[command(subcommand)]
        command: Option<SkillsCommand>,
        /// En tu usuario (~/.claude/skills, ~/.agents/skills) en vez de este repo.
        #[arg(short = 'g', long, global = true)]
        global: bool,
        /// Solo estos agentes: claude, codex, cursor, copilot (separados por comas).
        #[arg(long, global = true, value_name = "AGENTES")]
        agent: Option<String>,
        /// Lee las skills de una carpeta (skills/<nombre>/SKILL.md) en vez de las del binario.
        #[arg(long, global = true, value_name = "RUTA")]
        from: Option<PathBuf>,
    },
    /// Actualiza fdt al último binario publicado en GitHub.
    Update {
        /// Solo avisa si hay una versión nueva.
        #[arg(long)]
        check: bool,
    },
}

#[derive(Subcommand)]
pub enum TokenCommand {
    /// Crea un token scoped para una organización (y opcionalmente unos proyectos). Se muestra una sola vez.
    Create {
        /// Nombre para reconocerlo (p. ej. ci-tienda).
        name: String,
        /// read (solo lectura), deploy (lectura, deploys y variables) o admin.
        #[arg(long, default_value = "deploy", value_parser = ["read", "deploy", "admin"])]
        level: String,
        /// Limita el token a este proyecto (se puede repetir).
        #[arg(long = "project", short = 'p')]
        projects: Vec<String>,
        /// Caducidad en días (1-365).
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u16).range(1..=365))]
        expires_in_days: u16,
        /// Imprime el token (se muestra una sola vez).
        #[arg(long)]
        reveal: bool,
        /// Escribe el token en este archivo (permisos 0600) en vez de imprimirlo.
        #[arg(long, short = 'o', value_name = "ARCHIVO")]
        output: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Tus tokens (sin secreto).
    List,
    /// Revoca un token (por id o nombre).
    Revoke {
        token: String,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
pub enum OrgCommand {
    /// Tus organizaciones y tu rol en cada una.
    List,
    /// Detalle de una organización (por defecto la del contexto).
    Show { org: Option<String> },
    /// Consumo frente a los límites del plan.
    Usage { org: Option<String> },
    /// Miembros de la organización.
    Members { org: Option<String> },
}

#[derive(Subcommand)]
pub enum ProjectCommand {
    /// Proyectos de la organización.
    List,
    /// Un proyecto con sus apps y bases de datos.
    Show { project: Option<String> },
    /// Crea un proyecto.
    Create {
        name: String,
        #[arg(long)]
        description: Option<String>,
        /// Si ya existe uno con ese nombre, lo devuelve en vez de fallar.
        #[arg(long)]
        if_not_exists: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Cambia el nombre y/o la descripción de un proyecto.
    Rename {
        project: String,
        /// Nombre nuevo.
        new_name: Option<String>,
        /// Descripción nueva ("" la vacía).
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Borra un proyecto (debe estar sin servicios). Exige --confirm <nombre exacto>.
    Delete {
        project: String,
        #[arg(long, value_name = "NOMBRE")]
        confirm: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Args)]
pub struct ServiceCreate {
    /// Subdominio y slug del servicio (3-32 caracteres a-z, 0-9, -): https://<slug>.<dominio>.
    pub slug: String,
    /// Repo de GitHub (owner/nombre) de la app a desplegar.
    #[arg(long, conflicts_with = "app")]
    pub repo: Option<String>,
    /// App de Core (id, slug o nombre) en vez de --repo.
    #[arg(long)]
    pub app: Option<String>,
    /// Pone la app en este proyecto (idempotente).
    #[arg(long, short = 'p')]
    pub project: Option<String>,
    /// Nombre visible (por defecto el slug).
    #[arg(long)]
    pub name: Option<String>,
    /// Rama que dispara el deploy.
    #[arg(long, default_value = "main")]
    pub branch: String,
    /// Puerto en el que escucha el contenedor.
    #[arg(long, default_value_t = 8080)]
    pub port: u16,
    /// Memoria en MB (256, 512 o 1024, según el plan).
    #[arg(long, alias = "size", default_value_t = 512)]
    pub memory: u32,
    /// Ruta del healthcheck (p. ej. /health).
    #[arg(long)]
    pub health_path: Option<String>,
    /// Dockerfile dentro del repo.
    #[arg(long)]
    pub dockerfile: Option<String>,
    /// Carpeta de contexto del build.
    #[arg(long)]
    pub build_context: Option<String>,
    /// No desplegar solo al llegar una imagen nueva.
    #[arg(long)]
    pub no_auto_deploy: bool,
    /// Cómo instalar el workflow: manual (por defecto), commit o pull_request.
    #[arg(long, value_parser = ["manual", "commit", "pull_request"])]
    pub workflow_mode: Option<String>,
    /// Variable inicial KEY=VALUE (se puede repetir).
    #[arg(long = "env", short = 'e', value_name = "KEY=VALUE")]
    pub env: Vec<String>,
    /// Secreto inicial KEY=VALUE (se puede repetir). Queda en el historial del shell: mejor --secret-from-env/--secret-from-file.
    #[arg(long = "secret", value_name = "KEY=VALUE")]
    pub secrets: Vec<String>,
    /// Secreto inicial leído de una variable de entorno local: KEY o KEY=VARIABLE (se puede repetir).
    #[arg(long = "secret-from-env", value_name = "KEY[=VAR]")]
    pub secrets_from_env: Vec<String>,
    /// Secreto inicial leído de un archivo: KEY=RUTA (se puede repetir).
    #[arg(long = "secret-from-file", value_name = "KEY=RUTA")]
    pub secrets_from_file: Vec<String>,
    /// Si ya existe un servicio con ese slug, lo devuelve en vez de fallar.
    #[arg(long)]
    pub if_not_exists: bool,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct ServiceUpdate {
    pub service: Option<String>,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub branch: Option<String>,
    #[arg(long)]
    pub port: Option<u16>,
    #[arg(long, alias = "size")]
    pub memory: Option<u32>,
    /// Ruta del healthcheck; "" lo quita.
    #[arg(long)]
    pub health_path: Option<String>,
    #[arg(long)]
    pub dockerfile: Option<String>,
    #[arg(long)]
    pub build_context: Option<String>,
    /// true o false.
    #[arg(long, value_name = "BOOL")]
    pub auto_deploy: Option<bool>,
    #[arg(long, value_parser = ["manual", "commit", "pull_request"])]
    pub workflow_mode: Option<String>,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Subcommand)]
pub enum ServiceCommand {
    /// Servicios de la organización o de un proyecto, con URL, estado y último deploy.
    List {
        #[arg(long, short = 'p')]
        project: Option<String>,
    },
    /// Un servicio.
    Show { service: Option<String> },
    /// Crea un servicio para el repo de una app.
    #[command(after_help = "Ejemplo:\n  fdt service create api --project tienda --repo acme/tienda-api --size 512 --port 3000 --if-not-exists --json")]
    Create(ServiceCreate),
    /// Cambia ajustes (nombre, rama, puerto, memoria, healthcheck, build).
    Update(ServiceUpdate),
    /// Borra un servicio. Exige --confirm <slug exacto>.
    Delete {
        service: String,
        #[arg(long, value_name = "SLUG")]
        confirm: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Solo lectura: workflow de GitHub Actions, Dockerfiles sugeridos y estado del token del registry.
    Setup {
        service: Option<String>,
        /// Escribe el workflow en este repo (por defecto .github/workflows/foundathyon.yml).
        #[arg(long, value_name = "RUTA", num_args = 0..=1, default_missing_value = ".github/workflows/foundathyon.yml")]
        write_workflow: Option<PathBuf>,
    },
    /// Instala el workflow y su secreto en el repo de GitHub, por commit o pull request.
    #[command(after_help = "Con --mode commit, Core hace commit en la rama: ese push dispara un build y deja tu copia local atrás (haz git pull).")]
    InstallWorkflow {
        service: Option<String>,
        #[arg(long, value_parser = ["commit", "pull_request"], default_value = "pull_request")]
        mode: String,
        #[arg(long)]
        dry_run: bool,
    },
    /// Crea un token fdr_ nuevo del registry y REVOCA el anterior: el workflow deja de poder publicar
    /// imágenes hasta que actualices el secreto del repo. Exige --confirm <slug>.
    RotateRegistryToken {
        service: String,
        #[arg(long, value_name = "SLUG")]
        confirm: Option<String>,
        /// Imprime el token nuevo.
        #[arg(long)]
        reveal: bool,
        /// Escribe el token nuevo en este archivo (0600).
        #[arg(long, short = 'o', value_name = "ARCHIVO")]
        output: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Reinicia el contenedor con la misma imagen y ajustes (sin deployment nuevo).
    Restart {
        service: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Pausa el servicio: 0 contenedores (deja de responder). Exige --confirm <slug>.
    Pause {
        service: String,
        #[arg(long, value_name = "SLUG")]
        confirm: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Reanuda un servicio pausado con la imagen más reciente.
    Resume {
        service: Option<String>,
        #[command(flatten)]
        wait: Wait,
        #[arg(long)]
        dry_run: bool,
    },
    /// Explica por qué un servicio no despliega: los checks del repo, el workflow, la imagen y el deployment.
    Diagnose {
        service: Option<String>,
        /// Vuelve a leer GitHub sin esperar a la caché de 60 s.
        #[arg(long)]
        refresh: bool,
    },
}

#[derive(Subcommand)]
pub enum EnvCommand {
    /// Variables del servicio; los secretos salen enmascarados (la API nunca devuelve su valor).
    List {
        #[arg(long, short = 's')]
        service: Option<String>,
    },
    /// Crea o cambia variables. Un cambio redespliega el servicio (env_change) si tiene imagen.
    #[command(after_help = "Ejemplos:\n  fdt env set LOG_LEVEL=debug MODE=prod --service api\n  fdt env set STRIPE_KEY --secret --stdin --service api < key.txt\n  fdt env set STRIPE_KEY --secret --from-env STRIPE_KEY --service api\n  fdt env set DATABASE_URL --from-db tienda-pg --service api   # sin imprimir la URL")]
    Set {
        /// KEY=VALUE (o solo KEY con --stdin o --from-db).
        #[arg(required = true, value_name = "KEY=VALUE")]
        assignments: Vec<String>,
        #[arg(long, short = 's')]
        service: Option<String>,
        /// Guarda las variables como secretas.
        #[arg(long)]
        secret: bool,
        /// El valor es la cadena de conexión de esta base (se guarda como secreto, nunca se imprime).
        #[arg(long, value_name = "BASE", conflicts_with_all = ["stdin", "from_env", "from_file"])]
        from_db: Option<String>,
        /// Toma el valor (de una sola KEY) de esta variable de entorno local.
        #[arg(long, value_name = "VARIABLE", conflicts_with_all = ["stdin", "from_file"])]
        from_env: Option<String>,
        /// Toma el valor (de una sola KEY) de este archivo.
        #[arg(long, value_name = "RUTA", conflicts_with = "stdin")]
        from_file: Option<PathBuf>,
        /// Con --from-db, la cadena pública en vez de la interna (db-<id>).
        #[arg(long, requires = "from_db")]
        public: bool,
        /// Lee el valor (de una sola KEY) de stdin.
        #[arg(long)]
        stdin: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Quita variables. Las que no existen se ignoran.
    Unset {
        #[arg(required = true)]
        keys: Vec<String>,
        #[arg(long, short = 's')]
        service: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Sube un archivo .env y lo mezcla con lo que hay.
    Import {
        file: PathBuf,
        #[arg(long, short = 's')]
        service: Option<String>,
        /// Todas las del archivo como secretas.
        #[arg(long)]
        secret: bool,
        /// Solo estas como secretas (se puede repetir).
        #[arg(long = "secret-key", value_name = "KEY")]
        secret_keys: Vec<String>,
        /// Quita las variables que no están en el archivo.
        #[arg(long)]
        replace: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Baja las variables no secretas en formato .env (los secretos van como comentario).
    Pull {
        #[arg(long, short = 's')]
        service: Option<String>,
        /// Archivo de salida (por defecto stdout).
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum DbCommand {
    /// Bases de la organización o de un proyecto: motor, host y estado.
    List {
        #[arg(long, short = 'p')]
        project: Option<String>,
    },
    /// Una base (sin contraseña).
    Show { db: String },
    /// Crea una base Postgres o MongoDB.
    Create {
        /// 3-32 caracteres a-z, 0-9, -.
        name: String,
        #[arg(long, value_parser = ["postgres", "mongo"], default_value = "postgres")]
        engine: String,
        /// Nombre de la base dentro del motor (por defecto fd_<org>_<nombre>).
        #[arg(long)]
        db_name: Option<String>,
        /// Usuario (por defecto igual que --db-name).
        #[arg(long)]
        db_user: Option<String>,
        /// La pone en este proyecto.
        #[arg(long, short = 'p')]
        project: Option<String>,
        #[arg(long)]
        if_not_exists: bool,
        /// Espera a que esté ready.
        #[arg(long)]
        wait: bool,
        #[arg(long, default_value = "10m", value_parser = commands::parse_duration, value_name = "DURACIÓN")]
        timeout: std::time::Duration,
        #[arg(long)]
        dry_run: bool,
    },
    /// Borra una base. Exige --confirm <nombre exacto>.
    Delete {
        db: String,
        #[arg(long, value_name = "NOMBRE")]
        confirm: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Imprime la cadena de conexión CON contraseña (interna por defecto: la que usan los servicios).
    Url {
        db: String,
        /// Necesario: la URL lleva la contraseña.
        #[arg(long)]
        reveal: bool,
        /// La cadena pública (si la base tiene acceso público).
        #[arg(long)]
        public: bool,
    },
    /// Usuario, host y cadenas; la contraseña solo con --reveal. Cada lectura queda en auditoría.
    Credentials {
        db: String,
        /// Muestra la contraseña y las cadenas completas.
        #[arg(long)]
        reveal: bool,
    },
    /// Cambia el usuario y/o la contraseña de una base Postgres. Las apps que la usan dejan de conectar
    /// hasta que actualices su DATABASE_URL (fdt env set DATABASE_URL --from-db). Exige --confirm <nombre>.
    SetCredentials {
        db: String,
        /// Usuario nuevo.
        #[arg(long)]
        user: Option<String>,
        /// Lee la contraseña nueva de stdin.
        #[arg(long, conflicts_with = "password_from_env")]
        password_stdin: bool,
        /// Lee la contraseña nueva de esta variable de entorno.
        #[arg(long, value_name = "VARIABLE")]
        password_from_env: Option<String>,
        #[arg(long, value_name = "NOMBRE")]
        confirm: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Pone una base en un proyecto.
    Link {
        db: String,
        #[arg(long, short = 'p')]
        project: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Quita una base de un proyecto (la base sigue existiendo).
    Unlink {
        db: String,
        #[arg(long, short = 'p')]
        project: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Abre psql o mongosh ya conectado (necesita acceso público y una terminal).
    Connect { db: String },
}

#[derive(Subcommand)]
pub enum SkillsCommand {
    /// Skills disponibles y dónde están instaladas (lo mismo que `fdt skills`).
    List,
    /// Instala skills (all = todas).
    Add {
        #[arg(required = true)]
        skills: Vec<String>,
    },
    /// Quita skills que instaló fdt (nunca una escrita a mano).
    Remove {
        #[arg(required = true)]
        skills: Vec<String>,
    },
    /// Reinstala las skills que fdt instaló aquí.
    Update,
}

/// Runs the CLI; errors become JSON on stderr (agents, `--json`, no terminal) or a short message.
pub async fn main() {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            // Usage errors exit 2; --help and --version exit 0. With --json, as JSON.
            let json = std::env::args().any(|a| a == "--json") || !std::io::stderr().is_terminal();
            let code = error.exit_code();
            if code != 0 && json {
                let message = error.kind().to_string();
                let detail = error.to_string();
                let body = serde_json::json!({"ok": false, "error": {
                    "kind": "usage", "exit_code": 2, "message": message, "detail": detail.trim(),
                    "hint": "fdt <comando> --help",
                }});
                eprintln!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
                std::process::exit(2);
            }
            error.exit();
        }
    };
    let json_errors = cli.json || !std::io::stdout().is_terminal() || !std::io::stderr().is_terminal();
    if let Err(error) = run(cli).await {
        let (report, code) = failure::report(&error);
        if json_errors {
            eprintln!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
        } else {
            let c = ui::Colors::stderr();
            let e = &report["error"];
            let scope = e["scope"].as_str().map(|s| format!(" [{s}]")).unwrap_or_default();
            eprintln!("{} {}{}", c.red("✖"), e["message"].as_str().unwrap_or("error"), c.gray(&scope));
            if let Some(candidates) = e["details"]["candidates"].as_array() {
                for candidate in candidates {
                    eprintln!("  · {}", candidate.as_str().unwrap_or_default());
                }
            }
            if let Some(hint) = e["hint"].as_str() {
                eprintln!("  {} {hint}", c.dim("→"));
            }
        }
        std::process::exit(code);
    }
}

pub async fn run(cli: Cli) -> Result<()> {
    let mut ctx = Ctx::new(cli.json, cli.no_input, cli.org, cli.api_url)?;
    match cli.command {
        Command::Login { token, with_token, no_browser } => session::login(&mut ctx, token, with_token, no_browser).await,
        Command::Logout => session::logout(&mut ctx).await,
        Command::Whoami => session::whoami(&ctx).await,
        Command::Token { command } => match command {
            TokenCommand::Create { name, level, projects, expires_in_days, reveal, output, dry_run } => {
                session::token_create(&ctx, name, level, projects, expires_in_days, reveal, output, dry_run).await
            }
            TokenCommand::List => session::token_list(&ctx).await,
            TokenCommand::Revoke { token, dry_run } => session::token_revoke(&ctx, token, dry_run).await,
        },
        Command::Use { org } => misc::use_org(&mut ctx, org).await,
        Command::Link { project, service } => misc::link(&ctx, project, service).await,
        Command::Unlink => misc::unlink(&ctx),
        Command::Context => misc::context(&ctx),
        Command::Status { project } => misc::status(&ctx, project).await,
        Command::Doctor => misc::doctor(&ctx).await,
        Command::Org { command } => match command {
            OrgCommand::List => org::list(&ctx).await,
            OrgCommand::Show { org: o } => org::show(&ctx, o).await,
            OrgCommand::Usage { org: o } => org::usage(&ctx, o).await,
            OrgCommand::Members { org: o } => org::members(&ctx, o).await,
        },
        Command::Project { command } => match command {
            ProjectCommand::List => project::list(&ctx).await,
            ProjectCommand::Show { project: p } => project::show(&ctx, p).await,
            ProjectCommand::Create { name, description, if_not_exists, dry_run } => {
                project::create(&ctx, name, description, if_not_exists, dry_run).await
            }
            ProjectCommand::Rename { project: p, new_name, description, dry_run } => {
                project::rename(&ctx, p, new_name, description, dry_run).await
            }
            ProjectCommand::Delete { project: p, confirm, dry_run } => project::delete(&ctx, p, confirm, dry_run).await,
        },
        Command::Service { command } => match *command {
            ServiceCommand::List { project: p } => service::list(&ctx, p).await,
            ServiceCommand::Show { service: s } => service::show(&ctx, s).await,
            ServiceCommand::Create(args) => service::create(&ctx, args).await,
            ServiceCommand::Update(args) => service::update(&ctx, args).await,
            ServiceCommand::Delete { service: s, confirm, dry_run } => service::delete(&ctx, s, confirm, dry_run).await,
            ServiceCommand::Setup { service: s, write_workflow } => service::setup(&ctx, s, write_workflow).await,
            ServiceCommand::InstallWorkflow { service: s, mode, dry_run } => service::install_workflow(&ctx, s, mode, dry_run).await,
            ServiceCommand::RotateRegistryToken { service: s, confirm, reveal, output, dry_run } => {
                service::rotate_registry_token(&ctx, s, confirm, reveal, output, dry_run).await
            }
            ServiceCommand::Restart { service: s, dry_run } => service::lifecycle(&ctx, s, "restart", None, None, dry_run).await,
            ServiceCommand::Pause { service: s, confirm, dry_run } => {
                service::lifecycle(&ctx, Some(s), "pause", confirm, None, dry_run).await
            }
            ServiceCommand::Resume { service: s, wait, dry_run } => service::lifecycle(&ctx, s, "resume", None, Some(wait), dry_run).await,
            ServiceCommand::Diagnose { service: s, refresh } => service::diagnose(&ctx, s, refresh).await,
        },
        Command::Deploy { service: s, rebuild, wait, dry_run } => deploy::deploy(&ctx, s, rebuild, wait, dry_run).await,
        Command::Rollback { service: s, tag, digest, to, wait, dry_run } => {
            deploy::rollback(&ctx, s, tag, digest, to, wait, dry_run).await
        }
        Command::Redeploy { service: s, wait, dry_run } => deploy::redeploy(&ctx, s, wait, dry_run).await,
        Command::Deploys { service: s, deployment, limit, job, tail } => {
            deploy::deploys(&ctx, s, deployment, limit, job, tail).await
        }
        Command::Logs { service: s, tail, since, until } => deploy::logs(&ctx, s, tail, since, until).await,
        Command::Env { command } => match command {
            EnvCommand::List { service: s } => env::list(&ctx, s).await,
            EnvCommand::Set { assignments, service: s, secret, from_db, from_env, from_file, public, stdin, dry_run } => {
                env::set(&ctx, env::SetArgs { assignments, service: s, secret, from_db, from_env, from_file, public, stdin, dry_run }).await
            }
            EnvCommand::Unset { keys, service: s, dry_run } => env::unset(&ctx, keys, s, dry_run).await,
            EnvCommand::Import { file, service: s, secret, secret_keys, replace, dry_run } => {
                env::import(&ctx, file, s, secret, secret_keys, replace, dry_run).await
            }
            EnvCommand::Pull { service: s, output } => env::pull(&ctx, s, output).await,
        },
        Command::Db { command } => match command {
            DbCommand::List { project: p } => db::list(&ctx, p).await,
            DbCommand::Show { db: d } => db::show(&ctx, d).await,
            DbCommand::Create { name, engine, db_name, db_user, project: p, if_not_exists, wait, timeout, dry_run } => {
                db::create(&ctx, db::CreateArgs { name, engine, db_name, db_user, project: p, if_not_exists, wait, timeout, dry_run }).await
            }
            DbCommand::Delete { db: d, confirm, dry_run } => db::delete(&ctx, d, confirm, dry_run).await,
            DbCommand::Url { db: d, reveal, public } => db::url(&ctx, d, public, reveal).await,
            DbCommand::Credentials { db: d, reveal } => db::credentials(&ctx, d, reveal).await,
            DbCommand::SetCredentials { db: d, user, password_stdin, password_from_env, confirm, dry_run } => {
                db::set_credentials(&ctx, d, user, password_stdin, password_from_env, confirm, dry_run).await
            }
            DbCommand::Link { db: d, project: p, dry_run } => db::link(&ctx, d, p, dry_run, true).await,
            DbCommand::Unlink { db: d, project: p, dry_run } => db::link(&ctx, d, p, dry_run, false).await,
            DbCommand::Connect { db: d } => db::connect(&ctx, d).await,
        },
        Command::Api { method, path, data, input, query } => misc::api(&ctx, method, path, data, input, query).await,
        Command::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "fdt", &mut std::io::stdout());
            Ok(())
        }
        Command::Skills { command, global, agent, from } => {
            let action = match command {
                None | Some(SkillsCommand::List) => crate::skills::Action::List,
                Some(SkillsCommand::Add { skills }) => crate::skills::Action::Add(skills),
                Some(SkillsCommand::Remove { skills }) => crate::skills::Action::Remove(skills),
                Some(SkillsCommand::Update) => crate::skills::Action::Update,
            };
            crate::skills::run(&ctx, action, crate::skills::Options { global, agents: agent, from })
        }
        Command::Update { check } => misc::update(&ctx, check).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from([&["fdt"], args].concat())
    }

    #[test]
    fn command_tree_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn accepts_the_plan_examples() {
        assert!(parse(&["service", "create", "api", "--project", "tienda", "--repo", "acme/tienda-api", "--size", "512", "--json"]).is_ok());
        assert!(parse(&["env", "set", "DATABASE_URL", "--from-db", "tienda-pg", "--service", "api"]).is_ok());
        assert!(parse(&["deploy", "api", "--wait", "--json"]).is_ok());
        assert!(parse(&["logs", "api", "--since", "10m"]).is_ok());
        assert!(parse(&["service", "diagnose", "api", "--json"]).is_ok());
        assert!(parse(&["deploys", "api", "abc123", "--job", "42"]).is_ok());
        assert!(parse(&["deploys", "api", "--job", "42"]).is_err());
        assert!(parse(&["project", "delete", "tienda", "--confirm", "tienda", "--dry-run"]).is_ok());
        assert!(parse(&["token", "create", "ci", "--level", "deploy", "-p", "tienda", "--expires-in-days", "30"]).is_ok());
        assert!(parse(&["token", "create", "ci", "--level", "owner"]).is_err());
        assert!(parse(&["token", "create", "ci", "--expires-in-days", "400"]).is_err());
        assert!(parse(&["env", "set", "A", "--public"]).is_err());
        assert!(parse(&["skills", "add", "fdt-basics", "-g", "--agent", "claude"]).is_ok());
        assert!(parse(&["deploy", "--timeout", "nope"]).is_err());
        assert!(parse(&["service", "setup", "api", "--write-workflow"]).is_ok());
        assert!(parse(&["service", "setup", "api", "--install-workflow", "commit"]).is_err());
        assert!(parse(&["service", "install-workflow", "api", "--mode", "commit"]).is_ok());
        assert!(parse(&["service", "rotate-registry-token", "api", "--confirm", "api", "-o", "t"]).is_ok());
        assert!(parse(&["service", "pause", "api", "--confirm", "api"]).is_ok());
        assert!(parse(&["service", "resume", "api", "--wait"]).is_ok());
        assert!(parse(&["service", "restart", "api", "--dry-run"]).is_ok());
        assert!(parse(&["rollback", "api", "--tag", "v1", "--digest", "sha256:x"]).is_err());
        assert!(parse(&["rollback", "api", "--to", "abc123", "--wait"]).is_ok());
        assert!(parse(&["deploy", "api", "--rebuild", "--dry-run"]).is_ok());
        assert!(parse(&["logs", "api", "--since", "2026-11-23T10:00:00Z", "--until", "5m"]).is_ok());
        assert!(parse(&["env", "set", "K", "--from-env", "K", "--stdin"]).is_err());
        assert!(parse(&["db", "url", "pg", "--reveal"]).is_ok());
        assert!(parse(&["project", "rename", "tienda", "--description", "x"]).is_ok());
        assert!(parse(&["token", "create", "ci", "--output", "t"]).is_ok());
        assert!(parse(&["api", "GET", "/organizations/{org}/usage", "-q", "a=b"]).is_ok());
    }
}
