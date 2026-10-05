// swift-tools-version: 6.0
//
// The SwiftPM manifest for the Swift SDK (SDK2 Task 4, Loams issue #288).
//
// Platform floors are the plan's (SDK2 Task 4: "iOS 15+/macOS 12+/Linux"):
// `swift-tools-version: 6.0` is Swift 6, and `.macOS(.v12)` / `.iOS(.v15)` are
// the lowest versions whose Foundation has the async `URLSession` APIs the
// transport is written against. Linux is supported and needs no declaration
// here — SwiftPM treats a platform with no entry as "any".
//
// # There are no package dependencies, and that is a decision
//
// The language matrix row 5 says Swift is Connect over `connect-swift`, and that
// remains the intent. This manifest does not depend on it yet, for a reason
// recorded in `README.md` and `DEPENDENCIES.md`: **the Swift toolchain was not
// available on the machine that wrote this SDK**, so no single API of
// `connect-swift` could be checked against a compiler. Pinning a dependency
// whose entire surface is unverified would make `swift build` fail during
// resolution — before compiling a line of this SDK's own code — and would hide
// whether the code below is correct.
//
// So the transport is written directly against the Connect wire protocol over
// `URLSession`, and it sits behind `HTTPTransport`, which is the seam where
// `connect-swift` slots in. See `Sources/Loams/Transport.swift` for the exact
// boundary. Adding the dependency is then a contained change to one file, and it
// can be made by whoever has a toolchain.

import PackageDescription

let package = Package(
    name: "Loams",
    platforms: [
        .iOS(.v15),
        .macOS(.v12),
    ],
    products: [
        .library(name: "Loams", targets: ["Loams"]),
    ],
    dependencies: [
        // Intentionally empty. See the header comment.
    ],
    targets: [
        .target(
            name: "Loams",
            path: "Sources/Loams"
        ),
        .testTarget(
            name: "LoamsTests",
            dependencies: ["Loams"],
            path: "Tests/LoamsTests"
        ),
        .executableTarget(
            name: "Quickstart",
            dependencies: ["Loams"],
            path: "Examples/Quickstart"
        ),
    ],
    swiftLanguageVersions: [.v6]
)