// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "ImageVault",
    platforms: [.macOS(.v13)],
    targets: [
        // Swift cannot catch Objective-C exceptions; this tiny ObjC shim can.
        .target(name: "ObjCExceptionCatcher", path: "Sources/ObjCExceptionCatcher"),
        .executableTarget(name: "ImageVault", dependencies: ["ObjCExceptionCatcher"], path: "Sources/ImageVault"),
    ]
)
