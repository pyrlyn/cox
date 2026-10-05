// swift-tools-version: 6.2
// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// CoxCore (DT§4.6): the Rust core behind the `CoreClient` protocol.
// `CoxFFI` is the XCFramework `just desktop-xcframework` builds;
// `CoxFFIBindings` is UniFFI's generated Swift over it, a symlink into the
// same gitignored build output, compiled in Swift 5 mode because its
// `Sendable` coverage is partial (research.md 9.3.4) — `CoxCore` wraps it, so
// that stays inside this package. Needs the XCFramework built first.
import PackageDescription

/// desktop/macos/.swiftlint.yml, through Packages/CoxCore/.swiftlint.yml;
/// not on the generated `CoxFFIBindings`.
let swiftLint = Target.PluginUsage.plugin(
  name: "SwiftLintBuildToolPlugin", package: "SwiftLintPlugins")

let package = Package(
  name: "CoxCore",
  platforms: [.macOS(.v26)],
  products: [
    .library(name: "CoxCore", targets: ["CoxCore"])
  ],
  dependencies: [
    .package(path: "../CoxModel"),
    .package(url: "https://github.com/SimplyDanny/SwiftLintPlugins", exact: "0.65.1"),
  ],
  targets: [
    .binaryTarget(name: "CoxFFI", path: "../../build/CoxFFI.xcframework"),
    .target(
      name: "CoxFFIBindings",
      dependencies: ["CoxFFI"],
      swiftSettings: [.swiftLanguageMode(.v5)],
      // The static library's own system links: reqwest's proxy lookup
      // needs SystemConfiguration; Security comes through Foundation.
      linkerSettings: [.linkedFramework("SystemConfiguration")]
    ),
    .target(
      name: "CoxCore",
      dependencies: [
        "CoxFFIBindings",
        .product(name: "CoxClient", package: "CoxModel"),
      ],
      plugins: [swiftLint]
    ),
    .testTarget(
      name: "CoxCoreTests",
      dependencies: [
        "CoxCore", "CoxFFIBindings",
        .product(name: "CoxClient", package: "CoxModel"),
      ],
      plugins: [swiftLint]
    ),
  ]
)
