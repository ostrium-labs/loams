// swift-tools-version: 6.0
// LoamsCore: pure logic shared in spirit with Android's :core (design §37 §7.1). Foundation and
// CryptoKit only on Apple platforms; on Linux, swift-crypto stands in for CryptoKit so the
// golden-fixture tests run in Linux CI too (AP3 Ruling 6).
import PackageDescription

let package = Package(
    name: "LoamsCore",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [.library(name: "LoamsCore", targets: ["LoamsCore"])],
    dependencies: [
        .package(url: "https://github.com/apple/swift-crypto.git", "3.0.0"..<"6.0.0"),
    ],
    targets: [
        .target(
            name: "LoamsCore",
            dependencies: [.product(name: "Crypto", package: "swift-crypto", condition: .when(platforms: [.linux]))]
        ),
        .testTarget(name: "LoamsCoreTests", dependencies: ["LoamsCore"]),
    ]
)
