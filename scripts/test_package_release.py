import hashlib
import importlib.util
import os
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile


spec = importlib.util.spec_from_file_location("package_release", Path(__file__).with_name("package-release.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class PackageReleaseTests(unittest.TestCase):
    def test_tag_must_match_workspace_version(self):
        release.verify_version("1.2.3", "tag", "v1.2.3")
        release.verify_version("1.2.3-rc.1", "tag", "v1.2.3-rc.1")
        release.verify_version("1.2.3", "branch", "master")
        with self.assertRaises(ValueError):
            release.verify_version("1.2.3", "tag", "v1.2.4")

    def test_archives_include_binaries_licenses_and_desktop_resources(self):
        for target, desktop in [(target, desktop) for target in release.TARGETS for desktop in (False, True)]:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                binaries = root / "bin"
                binaries.mkdir()
                suffix = ".exe" if "windows" in target else ""
                for name in ("lantern", "lantern-mcp", "lantern-ui"):
                    binary = binaries / (name + suffix)
                    binary.write_bytes(b"test binary")
                    binary.chmod(0o755)
                library_name = "onnxruntime.dll" if suffix else ("libonnxruntime.dylib" if "apple" in target else "libonnxruntime.so.1")
                (binaries / library_name).write_bytes(b"test runtime")
                archive = release.package(target, desktop, binaries, root / "out")
                checksum = archive.with_name(archive.name + ".sha256").read_text().split()[0]
                self.assertEqual(hashlib.sha256(archive.read_bytes()).hexdigest(), checksum)
                if suffix:
                    with zipfile.ZipFile(archive) as bundle:
                        paths = [Path(name) for name in bundle.namelist()]
                        self.assertIn("lantern.exe", [path.name for path in paths])
                else:
                    with tarfile.open(archive) as bundle:
                        paths = [Path(member.name) for member in bundle.getmembers()]
                        if desktop and "linux" in target and os.name != "nt":
                            installer = next(member for member in bundle.getmembers() if member.name.endswith("/install.sh"))
                            self.assertTrue(installer.mode & 0o111)
                        for member in bundle.getmembers():
                            if os.name != "nt" and member.name.endswith(("/lantern", "/lantern-mcp", "/lantern-ui")):
                                self.assertTrue(member.mode & 0o111)
                names = [path.name for path in paths]
                for name in ("LICENSE", "README.md", "RELEASE.txt", "lantern-mcp" + suffix, library_name):
                    self.assertIn(name, names)
                if desktop:
                    for name in ("lantern-ui" + suffix, "lantern.svg", "Lucide.txt", "OFL-IBMPlex.txt", "OFL-Lilex.txt"):
                        self.assertIn(name, names)
                    if "linux" in target:
                        self.assertIn("dev.megalithofficial.Lantern.desktop", names)
                        self.assertIn("install.sh", names)
                    else:
                        self.assertNotIn("dev.megalithofficial.Lantern.desktop", names)
                        self.assertNotIn("install.sh", names)
                    self.assertEqual("setup-windows.ps1" in names, bool(suffix))
                else:
                    self.assertNotIn("lantern-ui" + suffix, names)
                    self.assertNotIn("setup-windows.ps1", names)

    @unittest.skipIf(os.name == "nt", "creating symlinks requires Windows Developer Mode")
    def test_runtime_symlinks_are_packaged_as_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binaries = root / "bin"
            binaries.mkdir()
            for name in ("lantern", "lantern-mcp"):
                (binaries / name).write_bytes(b"binary")
            runtime = root / "cached-runtime.so"
            runtime.write_bytes(b"runtime contents")
            (binaries / "libonnxruntime.so").symlink_to(runtime)
            archive = release.package(release.TARGETS[0], bin_dir=binaries, output=root / "out")
            with tarfile.open(archive) as bundle:
                library = next(member for member in bundle.getmembers() if member.name.endswith("/libonnxruntime.so"))
                self.assertTrue(library.isfile())
                self.assertEqual(bundle.extractfile(library).read(), b"runtime contents")


if __name__ == "__main__":
    unittest.main()
