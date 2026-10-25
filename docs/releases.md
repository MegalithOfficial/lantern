# Releases

Set the version in `[workspace.package]` in `Cargo.toml`, then run `cargo check --workspace` to update `Cargo.lock`. Commit the change and push a matching tag:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The Release workflow runs the checks, builds the packages, and opens a draft release. Review the notes and publish it on GitHub. Use tags such as `v0.2.0-rc.1` for prereleases.

To get a build without tagging, run the Build workflow manually and download its artifacts.

## Packages

Desktop archives, including the CLI and MCP server, are built for Linux x86_64, Windows x86_64 (MSVC), and macOS arm64. Each platform also has a smaller CLI/MCP-only archive. Intel Macs are not built: the configured ONNX Runtime download does not support x86_64 macOS.

All desktop packages require GTK 4.22+, GtkSourceView 5, and Pango 1.56+ installed separately. They are not self-contained installers. Java 21 or newer is required to decompile Minecraft.

- **Linux:** The desktop build uses Arch Linux and needs compatible glibc/C++ libraries. Extract the archive and run `./install.sh` to install it for your user.
- **Windows:** Install the [Microsoft Visual C++ x64 Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist). Extract the archive, open PowerShell in that directory, run `. ./setup-windows.ps1`, then `./lantern-ui.exe`. Dot-source the script in each new shell; GTK is downloaded only on the first run. It installs the pinned, SHA-256-verified [gvsbuild 2026.8.0 GTK4 stack](https://github.com/wingtk/gvsbuild/releases/tag/2026.8.0) under `%LOCALAPPDATA%\Lantern\gtk`. Use `-GtkRoot C:\path\to\gtk` to choose another location.
- **macOS (Apple Silicon):** Run `brew install gtk4 gtksourceview5`, extract the archive, and launch `./lantern-ui` from Terminal. This is a command-line launchable desktop binary, not a signed or notarized `.app` bundle. Install Java so that `java` is on `PATH`, or set `JAVA_HOME` before launching.

Each archive comes with a SHA-256 checksum. Published stable releases appear in the app's **Setup → About → Check for updates**.

## Local build

Release builds use optimization level 3, Thin LTO, and one codegen unit. Debug information is stripped from the binaries.

```sh
cargo build --release --locked --target x86_64-unknown-linux-gnu --workspace
python3 scripts/package-release.py --target x86_64-unknown-linux-gnu --desktop
```

Packages are written to `dist/`. Omit `--desktop` when packaging just the CLI and MCP binaries.

On macOS, first run `brew install gtk4 gtksourceview5 pkgconf`, then use `aarch64-apple-darwin` as the target in the same build and packaging commands. Homebrew runtime libraries must remain installed at their original paths.

On Windows, install Rust with the `x86_64-pc-windows-msvc` toolchain and Visual Studio's **Desktop development with C++** tools. GTK's [MSVC setup](https://gtk-rs.org/gtk4-rs/stable/latest/book/installation_windows.html) uses gvsbuild rather than MinGW libraries. In PowerShell:

```powershell
. ./scripts/setup-windows.ps1
cargo build --release --locked --target x86_64-pc-windows-msvc --workspace
python scripts/package-release.py --target x86_64-pc-windows-msvc --desktop
```

The Check workflow checks, lints, and tests the UI on native Windows and macOS runners as well as Linux. Windows and macOS also launch the first-run UI in an isolated temporary directory and verify its rendered snapshot. The Build workflow repeats this launch check for release binaries before packaging them.
