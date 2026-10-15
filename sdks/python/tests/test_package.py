"""A wheel consumer must work outside the checkout with bundled contracts and types."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import zipfile


class PackageTests(unittest.TestCase):
    def test_installed_wheel_is_self_contained(self):
        sdk = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory(prefix="parsequerade-wheel-") as directory:
            root = Path(directory)
            subprocess.run([sys.executable, "-m", "pip", "wheel", "--no-deps", str(sdk), "--wheel-dir", directory],
                           cwd=root, check=True, capture_output=True, text=True)
            wheel = next(root.glob("*.whl"))
            with zipfile.ZipFile(wheel) as archive:
                paths = set(archive.namelist())
                self.assertIn("parsequerade/contracts.json", paths)
                self.assertIn("parsequerade/py.typed", paths)
                self.assertTrue(any(path.endswith("/licenses/LICENSE") for path in paths))
                self.assertFalse(any("worker/" in path for path in paths))
            target = root / "installed"
            subprocess.run([sys.executable, "-m", "pip", "install", "--no-deps", "--target", str(target), str(wheel)],
                           cwd=root, check=True, capture_output=True, text=True)
            consumer = root / "consumer.py"
            consumer.write_text('''
import sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import parsequerade
from parsequerade import ApiError, Client, Response
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import ec
assert Path(parsequerade.__file__).is_relative_to(Path(sys.argv[1]))
key = ec.generate_private_key(ec.SECP256R1())
pem = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()).decode()
calls = []
def send(method, url, headers, body, timeout):
    calls.append(url)
    assert url.startswith("https://api.appstoreconnect.apple.com/v1/apps")
    assert headers["Authorization"].startswith("Bearer ")
    return Response(200, {"content-type": "application/json"}, b'{"data":[{"type":"apps","id":"123","attributes":{"name":"Installed app"}}],"links":{}}')
client = Client("issuer", "key", pem, transport=send)
assert not calls
assert client.apps()["items"][0]["name"] == "Installed app"
try:
    client.apps({"limit": 0})
except ApiError as error:
    assert error.code == "invalid_query"
else:
    raise AssertionError("Expected local validation")
assert len(calls) == 1
''')
            subprocess.run([sys.executable, "-I", str(consumer), str(target)], cwd=root, check=True, capture_output=True, text=True)


if __name__ == "__main__":
    unittest.main()
