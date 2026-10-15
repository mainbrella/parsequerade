// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "Parsequerade",
    platforms: [.macOS(.v12), .iOS(.v15), .tvOS(.v15), .watchOS(.v8)],
    products: [.library(name: "Parsequerade", targets: ["Parsequerade"])],
    targets: [
        .target(name: "Parsequerade", resources: [.process("Resources")]),
        .executableTarget(name: "list-apps", dependencies: ["Parsequerade"], path: "Examples"),
        .testTarget(name: "ParsequeradeTests", dependencies: ["Parsequerade"], resources: [.process("Fixtures")]),
    ]
)
