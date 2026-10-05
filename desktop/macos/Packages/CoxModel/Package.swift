// swift-tools-version: 6.2
// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// CoxModel (DT§4.6): the app's state without the Rust core. `CoxClient` is
// the contract — the timeline value types, the `CoreClient` protocol and the
// fixture client — and `CoxModel` the `@Observable` stores over it. The
// protocol lives here, not in CoxCore, because a package that declares the
// `CoxFFI` binary target fails to load until the XCFramework is built, and
// these tests and every SwiftUI preview must run without Rust (DT§8).
import PackageDescription

/// desktop/macos/.swiftlint.yml, through Packages/CoxModel/.swiftlint.yml.
let swiftLint = Target.PluginUsage.plugin(
  name: "SwiftLintBuildToolPlugin", package: "SwiftLintPlugins")

let package = Package(
  name: "CoxModel",
  platforms: [.macOS(.v26)],
  products: [
    .library(name: "CoxClient", targets: ["CoxClient"]),
    .library(name: "CoxModel", targets: ["CoxModel"]),
  ],
  dependencies: [
    .package(url: "https://github.com/apple/swift-collections", from: "1.7.1"),
    .package(url: "https://github.com/SimplyDanny/SwiftLintPlugins", exact: "0.65.1"),
  ],
  targets: [
    .target(name: "CoxClient", plugins: [swiftLint]),
    .target(
      name: "CoxModel",
      dependencies: [
        "CoxClient",
        .product(name: "OrderedCollections", package: "swift-collections"),
      ],
      plugins: [swiftLint]
    ),
    .testTarget(name: "CoxModelTests", dependencies: ["CoxModel"], plugins: [swiftLint]),
  ]
)
