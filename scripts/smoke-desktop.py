"""Launch the first-run desktop UI and verify that it renders a window, without user data."""

import argparse
import os
from pathlib import Path
import struct
import subprocess
import tempfile


def smoke(binary):
    binary = Path(binary).resolve()
    subprocess.run([binary, "--version"], check=True, timeout=30)
    with tempfile.TemporaryDirectory(prefix="lantern-smoke-") as temporary:
        root = Path(temporary)
        snapshot = root / "onboarding.png"
        env = {key: value for key, value in os.environ.items() if not key.startswith("LANTERN_")}
        env.update({
            "LANTERN_DB": str(root / "index.db"),
            "LANTERN_SNAPSHOT": str(snapshot),
            "LANTERN_WAIT": "3000",
            "LANTERN_SIZE": "1200x800",
            "GSK_RENDERER": "cairo",
            "XDG_CACHE_HOME": str(root / "cache"),
        })
        subprocess.run([binary], env=env, check=True, timeout=45)
        png = snapshot.read_bytes()
        if png[:8] != b"\x89PNG\r\n\x1a\n":
            raise RuntimeError("The desktop did not produce a PNG snapshot")
        width, height = struct.unpack(">II", png[16:24])
        if width < 800 or height < 500:
            raise RuntimeError(f"Unexpected desktop snapshot size: {width}x{height}")
        print(f"Desktop rendered successfully ({width}x{height})")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    smoke(parser.parse_args().binary)
