"""Build a self-contained Swift XCFramework from the shared Rust core.

The default builds the host macOS architecture for local development. Release
packages must select every required device/simulator target explicitly; no
unbuilt Apple platform is represented as supported by the binary artifact.
"""
import argparse
import os
import platform
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CORE = ROOT / "sdks/native"
PACKAGE = ROOT / "sdks/swift"
TARGETS = {
    "aarch64-apple-darwin": "macos",
    "x86_64-apple-darwin": "macos",
    "aarch64-apple-ios": "ios",
    "aarch64-apple-ios-sim": "ios-simulator",
    "x86_64-apple-ios": "ios-simulator",
    "aarch64-apple-tvos": "tvos",
    "aarch64-apple-tvos-sim": "tvos-simulator",
    "x86_64-apple-tvos": "tvos-simulator",
    "aarch64-apple-watchos": "watchos",
    "arm64_32-apple-watchos": "watchos",
    "armv7k-apple-watchos": "watchos",
    "aarch64-apple-watchos-sim": "watchos-simulator",
    "x86_64-apple-watchos-sim": "watchos-simulator",
}


def run(command, **kwargs):
    subprocess.run([str(arg) for arg in command], check=True, **kwargs)


def test_package():
    """Compile and run a consumer with only the packaged directory available."""
    with tempfile.TemporaryDirectory(prefix="parsequerade-swift-consumer-") as temporary:
        stage = Path(temporary)
        shutil.copytree(PACKAGE, stage / "SDK", ignore=shutil.ignore_patterns(".build", ".swiftpm"))
        consumer = stage / "Consumer"
        (consumer / "Sources/Consumer").mkdir(parents=True)
        (consumer / "Package.swift").write_text('''// swift-tools-version: 5.9
import PackageDescription
let package = Package(name: "Consumer", platforms: [.macOS(.v12)],
    dependencies: [.package(name: "Parsequerade", path: "../SDK")],
    targets: [.executableTarget(name: "Consumer", dependencies: [.product(name: "Parsequerade", package: "Parsequerade")])])
''')
        (consumer / "Sources/Consumer/main.swift").write_text('''import Foundation
import CryptoKit
import Parsequerade

let client = try Client(issuerID: "issuer", keyID: "key", privateKey: P256.Signing.PrivateKey().pemRepresentation) { request in
    let data = Data(#"{"data":[{"type":"apps","id":"app1","attributes":{"name":"Packaged"}}],"links":{"next":null}}"#.utf8)
    return (data, HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: "HTTP/1.1", headerFields: ["Content-Type":"application/json"])!)
}
let page = try await client.apps()
precondition(page.items.first?.name == "Packaged")
let checkpoint = ListingImageUploadCheckpoint()
_ = try JSONDecoder().decode(ListingImageUploadCheckpoint.self, from: JSONEncoder().encode(checkpoint))
await client.close()
print("Isolated Swift package consumer passed")
''')
        run(["swift", "run", "--package-path", consumer, "Consumer"], cwd=stage)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--targets", help="Comma-separated Rust Apple targets; defaults to the host macOS architecture.")
    parser.add_argument("--host", action="store_true", help="Build just the current macOS host architecture (the default).")
    parser.add_argument("--debug", action="store_true", help="Use a debug Rust library for faster local iteration.")
    parser.add_argument("--skip-build", action="store_true", help="Package previously built libraries.")
    parser.add_argument("--toolchain", help="Explicit Rust toolchain, for example nightly for tier-3 targets.")
    parser.add_argument("--build-std", action="store_true", help="Use nightly -Zbuild-std=std,panic_abort (requires rust-src).")
    parser.add_argument("--test-package", action="store_true", help="Also build and run a consumer of an isolated package copy.")
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("Swift Apple XCFramework packaging requires macOS and Xcode.")
    if args.host and args.targets:
        parser.error("Choose --host or --targets.")
    host = "aarch64" if platform.machine() == "arm64" else "x86_64"
    targets = args.targets.split(",") if args.targets else [host + "-apple-darwin"]
    if len(set(targets)) != len(targets) or any(target not in TARGETS for target in targets):
        parser.error("Unsupported or duplicate target. Available mappings: " + ", ".join(TARGETS))
    installed_command = ["rustup", "target", "list", "--installed"]
    if args.toolchain:
        installed_command.extend(["--toolchain", args.toolchain])
    installed = subprocess.check_output(installed_command, text=True).split()
    missing = [target for target in targets if target not in installed]
    if missing and not args.skip_build and not args.build_std:
        parser.error("Rust standard libraries are not installed for: " + ", ".join(missing) +
                     ". Install supported targets with rustup first. Tier-3 targets such as tvOS require an explicitly configured Rust build-std toolchain; this script never substitutes a macOS slice.")
    profile = "debug" if args.debug else "release"
    target_root = Path(os.environ.get("CARGO_TARGET_DIR", CORE / "target")).resolve()
    for target in targets:
        if not args.skip_build:
            command = ["cargo"]
            if args.toolchain:
                command.append("+" + args.toolchain)
            command.extend(["build", "--manifest-path", CORE / "Cargo.toml", "--locked", "--target", target])
            if args.build_std:
                command.append("-Zbuild-std=std,panic_abort")
            if not args.debug:
                command.append("--release")
            env = os.environ.copy()
            # ring builds C/assembly too; otherwise the current Xcode SDK can
            # accidentally give those objects the build host's newer minimum OS.
            env.update(MACOSX_DEPLOYMENT_TARGET="12.0", IPHONEOS_DEPLOYMENT_TARGET="15.0",
                       TVOS_DEPLOYMENT_TARGET="15.0", WATCHOS_DEPLOYMENT_TARGET="8.0")
            run(command, cwd=ROOT, env=env)
    groups = {}
    for target in targets:
        groups.setdefault(TARGETS[target], []).append(target)
    with tempfile.TemporaryDirectory(prefix="parsequerade-swift-") as temporary:
        stage = Path(temporary)
        headers = stage / "Headers"
        headers.mkdir()
        shutil.copy2(CORE / "include/parsequerade.h", headers / "parsequerade.h")
        (headers / "module.modulemap").write_text('module ParsequeradeCore {\n    header "parsequerade.h"\n    export *\n}\n')
        command = ["xcodebuild", "-create-xcframework"]
        for group, members in groups.items():
            libraries = [target_root / target / profile / "libparsequerade_native.a" for target in members]
            for library in libraries:
                if not library.is_file():
                    parser.error("Missing static library: " + str(library))
            destination = stage / group / "libparsequerade_native.a"
            destination.parent.mkdir()
            if len(libraries) == 1:
                shutil.copy2(libraries[0], destination)
            else:
                run(["xcrun", "lipo", "-create", *libraries, "-output", destination])
            command.extend(["-library", destination, "-headers", headers])
        artifact = stage / "ParsequeradeCore.xcframework"
        command.extend(["-output", artifact])
        run(command)
        destination = PACKAGE / artifact.name
        if destination.exists():
            shutil.rmtree(destination)
        shutil.copytree(artifact, destination)
    print("Swift XCFramework contains: " + ", ".join(targets))
    print("The Swift manifest retains macOS, iOS, tvOS and watchOS minimum versions; distribution must include the matching slices. A host-only artifact supports local macOS tests only.")
    if args.test_package:
        test_package()


if __name__ == "__main__":
    main()
