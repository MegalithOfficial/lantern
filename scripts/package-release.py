"""Package already-built Lantern binaries and verify release tags."""

import argparse
import hashlib
import os
from pathlib import Path
import shutil
import tarfile
import tempfile
import tomllib
import zipfile


ROOT = Path(__file__).resolve().parent.parent
TARGETS = (
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "aarch64-apple-darwin",
)


def version():
    with (ROOT / "Cargo.toml").open("rb") as manifest:
        return tomllib.load(manifest)["workspace"]["package"]["version"]


def verify_version(current, ref_type, ref_name):
    if ref_type == "tag" and ref_name != f"v{current}":
        raise ValueError(f"Tag {ref_name!r} does not match workspace version v{current}")


def package(target, desktop=False, bin_dir=None, output=None):
    if target not in TARGETS:
        raise ValueError(f"Unsupported target: {target}")
    bin_dir = Path(bin_dir) if bin_dir else ROOT / "target" / target / "release"
    output = Path(output) if output else ROOT / "dist"
    output.mkdir(parents=True, exist_ok=True)
    current = version()
    name = f"lantern-{current}-{target}" + ("-desktop" if desktop else "")
    windows = target.endswith("windows-msvc")
    with tempfile.TemporaryDirectory(prefix="lantern-package-") as temporary:
        stage = Path(temporary) / name
        stage.mkdir()
        for binary in ["lantern", "lantern-mcp"] + (["lantern-ui"] if desktop else []):
            filename = binary + (".exe" if windows else "")
            shutil.copy2(bin_dir / filename, stage / filename)
        for library in bin_dir.iterdir():
            if library.is_file() and (library.suffix in (".dll", ".dylib") or ".so" in library.name):
                # Resolve ort's symlinks so archives never point into the build machine's cache.
                shutil.copy2(library, stage / library.name, follow_symlinks=True)
        for filename in ("LICENSE", "README.md"):
            shutil.copy2(ROOT / filename, stage / filename)
        if desktop:
            licenses = stage / "licenses"
            licenses.mkdir()
            shutil.copy2(ROOT / "crates/lantern-ui/src/theme/icons/LICENSE", licenses / "Lucide.txt")
            for filename in ("OFL-IBMPlex.txt", "OFL-Lilex.txt"):
                shutil.copy2(ROOT / "crates/lantern-ui/src/theme/fonts" / filename, licenses / filename)
            shutil.copy2(ROOT / "assets/lantern-icon.svg", stage / "lantern.svg")
            if target.endswith("linux-gnu"):
                shutil.copy2(ROOT / "packaging/dev.megalithofficial.Lantern.desktop", stage / "dev.megalithofficial.Lantern.desktop")
                shutil.copy2(ROOT / "packaging/install.sh", stage / "install.sh")
                (stage / "install.sh").chmod(0o755)
        notes = [
            f"Lantern {current} ({target})",
            "Keep the binaries and any accompanying runtime libraries in the same directory.",
            "Run lantern --help for CLI commands and lantern setupmcp claudecode/codex for MCP setup.",
            "Java 21 or newer is required to decompile Minecraft; it is not bundled.",
        ]
        if desktop:
            if windows:
                shutil.copy2(ROOT / "scripts/setup-windows.ps1", stage / "setup-windows.ps1")
                notes += [
                    "In PowerShell, run . ./setup-windows.ps1 to install GTK and configure this session,",
                    "then run ./lantern-ui.exe. Repeat the dot-source command in each new session.",
                    "The Microsoft Visual C++ x64 Redistributable is also required.",
                ]
            elif target.endswith("apple-darwin"):
                notes += [
                    "Install the desktop runtime with: brew install gtk4 gtksourceview5",
                    "Run ./lantern-ui from Terminal. This build is for Apple Silicon Macs.",
                ]
            else:
                notes += [
                    "Run ./lantern-ui to launch, or ./install.sh to install for the current user.",
                    "This desktop build uses Arch Linux and needs compatible glibc/C++ libraries.",
                ]
            notes += [
                "GTK >= 4.22, GtkSourceView 5 and Pango >= 1.56 must be installed separately.",
                "No Minecraft files, models, or API keys are bundled.",
            ]
        (stage / "RELEASE.txt").write_text("\n".join(notes) + "\n", encoding="utf-8")
        if windows:
            archive = output / f"{name}.zip"
            with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
                for file in sorted(stage.rglob("*")):
                    if file.is_file():
                        bundle.write(file, file.relative_to(stage.parent).as_posix())
        else:
            archive = output / f"{name}.tar.gz"
            with tarfile.open(archive, "w:gz") as bundle:
                bundle.add(stage, arcname=name)
    with archive.open("rb") as file:
        digest = hashlib.file_digest(file, "sha256").hexdigest()
    archive.with_name(archive.name + ".sha256").write_text(f"{digest}  {archive.name}\n", encoding="utf-8")
    return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verify-version", action="store_true")
    parser.add_argument("--target", choices=TARGETS)
    parser.add_argument("--desktop", action="store_true")
    args = parser.parse_args()
    verify_version(version(), os.environ.get("GITHUB_REF_TYPE"), os.environ.get("GITHUB_REF_NAME"))
    if args.verify_version:
        print(f"Workspace version: {version()}")
    elif args.target:
        print(package(args.target, args.desktop))
    else:
        parser.error("--target or --verify-version is required")


if __name__ == "__main__":
    main()
