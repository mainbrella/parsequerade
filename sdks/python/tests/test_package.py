"""Wheel and source consumers must run independently of the checkout."""
from email.parser import Parser
import importlib.machinery
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
import venv
import zipfile

from test_client import PEM


class PackageTests(unittest.TestCase):
    def run_command(self, arguments, *, cwd):
        result = subprocess.run(arguments, cwd=cwd, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def assert_no_runtime_dependencies(self, metadata):
        requirements = Parser().parsestr(metadata).get_all("Requires-Dist", [])
        self.assertTrue(all("extra ==" in value for value in requirements), requirements)

    def test_wheel_and_source_distribution_are_self_contained(self):
        sdk = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory(prefix="parsequerade-package-") as directory:
            root = Path(directory)
            self.run_command([sys.executable, "-m", "pip", "wheel", "--no-deps", str(sdk),
                              "--wheel-dir", directory], cwd=root)
            wheel = next(root.glob("*.whl"))
            with zipfile.ZipFile(wheel) as archive:
                paths = set(archive.namelist())
                self.assertIn("parsequerade/contracts.json", paths)
                self.assertIn("parsequerade/py.typed", paths)
                self.assertTrue(any(path.startswith("parsequerade/_native") and
                                    any(path.endswith(suffix) for suffix in importlib.machinery.EXTENSION_SUFFIXES)
                                    for path in paths))
                self.assertTrue(any(path.endswith("/licenses/LICENSE") for path in paths))
                self.assertFalse(any("worker/" in path for path in paths))
                metadata = next(path for path in paths if path.endswith(".dist-info/METADATA"))
                self.assert_no_runtime_dependencies(archive.read(metadata).decode())
            self.run_command([sys.executable, "-m", "maturin", "sdist", "--manifest-path",
                              str(sdk / "native/Cargo.toml"), "--out", directory], cwd=sdk)
            sdist = next(root.glob("*.tar.gz"))
            with tarfile.open(sdist) as archive:
                paths = archive.getnames()
                self.assertTrue(any(path.endswith("/native/Cargo.toml") for path in paths))
                self.assertTrue(any(path.endswith("/rust/src/upload.rs") for path in paths), paths)
                self.assertTrue(any(path.endswith("/src/parsequerade/_client.py") for path in paths))
            consumer = root / "consumer.py"
            consumer.write_text('''
from importlib.util import find_spec
from pathlib import Path
import sys
import parsequerade
from parsequerade import ApiError, Client, Response
import parsequerade._native as native
assert Path(parsequerade.__file__).is_relative_to(Path(sys.prefix))
assert Path(native.__file__).is_relative_to(Path(sys.prefix))
assert native.__file__.endswith((".so", ".pyd"))
assert find_spec("cryptography") is None
pem = PEM_LITERAL
calls = []
def send(method, url, headers, body, timeout):
    calls.append(url)
    assert url.startswith("https://api.appstoreconnect.apple.com/v1/apps")
    assert any(k.lower() == "authorization" and v.startswith("Bearer ") for k, v in headers.items())
    return Response(200, {"content-type": "application/json"}, b'{"data":[{"type":"apps","id":"123","attributes":{"name":"Installed app"}}],"links":{}}')
client = Client("issuer", "key", pem, transport=send)
assert not calls
assert client.apps()["items"][0]["name"] == "Installed app"
for method in ("screenshot_sets", "screenshots", "preview_sets", "previews", "placements", "library_image",
               "builds", "build", "version_build", "build_prerelease_version", "update_metadata",
               "create_version", "attach_build", "listing_image_reference_data", "upload_listing_image"):
    assert callable(getattr(client, method))
try:
    client.apps({"limit": 0})
except ApiError as error:
    assert error.code == "invalid_query"
else:
    raise AssertionError("Expected local validation")
assert len(calls) == 1
'''.replace("PEM_LITERAL", repr(PEM)))
            for name, artifact in (("wheel", wheel), ("sdist", sdist)):
                with self.subTest(artifact=name):
                    environment = root / name
                    venv.EnvBuilder(with_pip=True).create(environment)
                    python = environment / ("Scripts/python.exe" if sys.platform == "win32" else "bin/python")
                    self.run_command([str(python), "-m", "pip", "install", "--no-deps", str(artifact)], cwd=root)
                    self.run_command([str(python), "-I", str(consumer)], cwd=root)


if __name__ == "__main__":
    unittest.main()
