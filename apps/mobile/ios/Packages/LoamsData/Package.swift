// swift-tools-version: 6.0
// LoamsData: Connect clients, trust, Secure Enclave keys, the keychain and the backends the
// screens use (AP3 Tasks 3-5). Apple platforms only.
import PackageDescription

let package = Package(
    name: "LoamsData",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [.library(name: "LoamsData", targets: ["LoamsData"])],
    dependencies: [
        .package(path: "../LoamsCore"),
        .package(path: "../LoamsProto"),
        .package(url: "https://github.com/connectrpc/connect-swift.git", exact: "1.2.3"),
        .package(url: "https://github.com/apple/swift-protobuf.git", from: "1.38.1"),
    ],
    targets: [
        .target(
            name: "LoamsData",
            dependencies: [
                "LoamsCore",
                "LoamsProto",
                .product(name: "Connect", package: "connect-swift"),
                .product(name: "SwiftProtobuf", package: "swift-protobuf"),
            ],
            // TODO(AP3 Task 0): Swift 6 mode. Security, LocalAuthentication and URLSession types
            // are not Sendable-annotated; LoamsCore already builds in Swift 6 mode.
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(name: "LoamsDataTests", dependencies: ["LoamsData"]),
    ]
)
