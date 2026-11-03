// swift-tools-version:5.9
// libopus packaged for the iPhone app (Swift Package Manager).
// The desktop app compiles the same files from desktop/src-tauri/build.rs.
import PackageDescription

let package = Package(
    name: "COpus",
    products: [
        .library(name: "COpus", targets: ["COpus", "COpusShim"]),
    ],
    targets: [
        .target(
            name: "COpus",
            path: "libopus",
            exclude: ["COPYING", "AUTHORS", "VERSION"],
            sources: ["celt", "silk", "src"],
            publicHeadersPath: "include",
            cSettings: [
                .headerSearchPath("."),
                .headerSearchPath("celt"),
                .headerSearchPath("silk"),
                .headerSearchPath("silk/float"),
                .define("OPUS_BUILD"),
                .define("HAVE_LRINTF"),
                .define("HAVE_LRINT"),
                .define("VAR_ARRAYS"),
            ]
        ),
        // opus_encoder_ctl() is variadic, which Swift can't call directly.
        .target(
            name: "COpusShim",
            dependencies: ["COpus"],
            path: "shim",
            publicHeadersPath: "include"
        ),
    ]
)
