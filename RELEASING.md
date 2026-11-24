# Publicar una versión de fdt

Un tag `vX.Y.Z` en GitHub dispara `.github/workflows/release.yml`, que compila, empaqueta y publica la release. No se sube nada a mano.

1. En `develop`, sube `version` en `Cargo.toml` a `X.Y.Z` y ejecuta `cargo build` para que `Cargo.lock` lo recoja. Haz merge a `main` (o etiqueta desde `develop`, como prefieras: el workflow solo mira el tag).
2. Comprueba en local: `cargo clippy --all-targets -- -D warnings && cargo test`.
3. Etiqueta y sube el tag:

   ```sh
   git tag -a vX.Y.Z -m "fdt vX.Y.Z"
   git push origin vX.Y.Z
   ```

4. Sigue el run en Actions → release. Hace, en orden:
   - **check**: falla si el tag no es `v` + la versión de `Cargo.toml`; clippy y tests.
   - **build**: un binario por objetivo (Linux musl estático x86_64 y arm64, macOS arm64 y x86_64, Windows x86_64), con `fdt --version` = el tag.
   - **publish**: `SHA256SUMS`, un `.sha256` por archivo, `install.sh`, `install.ps1` y la release con notas generadas.
   - **verify-install**: instala la release recién publicada con `install.sh` (Linux x86_64 y arm64, macOS) e `install.ps1` (PowerShell 7 y 5.1) y comprueba `fdt --version`.

Assets de cada release:

| Plataforma | Archivo |
|---|---|
| macOS Apple silicon | `fdt-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `fdt-x86_64-apple-darwin.tar.gz` |
| Linux x86_64 (cualquier distro) | `fdt-x86_64-unknown-linux-musl.tar.gz` |
| Linux arm64 (cualquier distro) | `fdt-aarch64-unknown-linux-musl.tar.gz` |
| Windows x64 (y ARM64 por emulación) | `fdt-x86_64-pc-windows-msvc.zip` |

Más `<archivo>.sha256`, `SHA256SUMS`, `install.sh` e `install.ps1`. Los nombres los leen `install.sh`, `install.ps1` y `src/update.rs`: si cambias uno, cambia los cuatro.

**Prereleases.** Un tag con sufijo (`v0.2.0-rc.1`, con `version = "0.2.0-rc.1"`) se publica como prerelease: `releases/latest`, los instaladores sin versión fijada y `fdt update` la ignoran. Para probarla: `FDT_INSTALL_VERSION=v0.2.0-rc.1`.

**Si algo falla.** Si falla antes de *publish*, no se publicó nada: corrige, borra el tag (`git push --delete origin vX.Y.Z && git tag -d vX.Y.Z`) y vuelve a etiquetar. Si *publish* creó la release y algo posterior falla, bórrala en GitHub (o `gh release delete vX.Y.Z --cleanup-tag`) antes de reintentar; nunca reutilices un tag que alguien pudo haber instalado: sube a la siguiente versión de parche.
