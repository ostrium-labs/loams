// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "LoamsObjC",
    platforms: [
        .iOS(.v15),
        .macOS(.v12),
    ],
    products: [
        .library(name: "LoamsObjC", targets: ["LoamsObjC"]),
    ],
    targets: [
        .target(
            name: "LoamsObjC",
            path: "Sources/LoamsObjC"
        ),
        .testTarget(
            name: "LoamsObjCTests",
            dependencies: ["LoamsObjC"],
            path: "Tests/LoamsObjCTests"
        ),
    ]
)
