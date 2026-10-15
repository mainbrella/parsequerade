"""Generate or check all SDK artifacts from the reviewed facade contracts."""

import argparse

from generate_rust_sdk import outputs as rust_outputs
from generate_typescript_sdk import outputs as typescript_outputs
from sdk_contracts import ROOT


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    for path, content in (rust_outputs() | typescript_outputs()).items():
        if args.check:
            if not path.exists() or path.read_text() != content:
                raise SystemExit(f"{path.relative_to(ROOT)} is stale; run npm run generate:sdks")
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)


if __name__ == "__main__":
    main()
