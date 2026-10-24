# Releases

Set the version in `[workspace.package]` in `Cargo.toml`, then run `cargo check --workspace` to update `Cargo.lock`. Commit the change and push a matching tag:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The Release workflow runs the checks, builds the packages, and opens a draft release. Review the notes and publish it on GitHub. Use tags such as `v0.2.0-rc.1` for prereleases.

To get a build without tagging, run the Build workflow manually and download its artifacts.

## Packages

CLI and MCP archives are built for Linux x86_64, Windows x86_64, and macOS x86_64/arm64. The Linux desktop archive also includes the app, icon, and `install.sh`.

The desktop build uses Arch Linux and requires GTK 4.22+, GtkSourceView 5, Pango 1.56+, and compatible glibc/C++ libraries. Extract the archive and run `./install.sh` to install it for your user. Install Java 21 or newer to decompile Minecraft.

Each archive comes with a SHA-256 checksum. Published stable releases appear in the app's **Setup → About → Check for updates**.

## Local build

Release builds use optimization level 3, Thin LTO, and one codegen unit. Debug information is stripped from the binaries.

```sh
cargo build --release --locked --target x86_64-unknown-linux-gnu --workspace
python3 scripts/package-release.py --target x86_64-unknown-linux-gnu --desktop
```

Packages are written to `dist/`. Omit `--desktop` when packaging just the CLI and MCP binaries.
