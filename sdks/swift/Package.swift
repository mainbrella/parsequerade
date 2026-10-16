// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "Parsequerade",
    platforms: [.macOS(.v12), .iOS(.v15), .tvOS(.v15), .watchOS(.v8)],
    products: [.library(name: "Parsequerade", targets: ["Parsequerade"])],
    targets: [
        .binaryTarget(name: "ParsequeradeCore", path: "ParsequeradeCore.xcframework"),
        .target(name: "Parsequerade", dependencies: ["ParsequeradeCore"],
                linkerSettings: [.linkedLibrary("iconv"), .linkedFramework("Security")]),
        .executableTarget(name: "list-apps", dependencies: ["Parsequerade"], path: "Examples"),
        .testTarget(name: "ParsequeradeTests", dependencies: ["Parsequerade"], resources: [.process("Fixtures")]),
    ]
)
