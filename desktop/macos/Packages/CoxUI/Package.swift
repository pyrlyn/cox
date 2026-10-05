// swift-tools-version: 6.2
// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// CoxUI: the macOS app's views and design system (DS§5). Foundations style views from the
// generated tokens only; the package depends on no other cox package (T37.19).

import PackageDescription

let package = Package(
  name: "CoxUI",
  platforms: [.macOS(.v26)],
  products: [
    .library(name: "CoxUI", targets: ["CoxUI"])
  ],
  dependencies: [
    .package(url: "https://github.com/SimplyDanny/SwiftLintPlugins", exact: "0.65.1"),
    .package(url: "https://github.com/pointfreeco/swift-snapshot-testing", exact: "1.19.6"),
  ],
  targets: [
    .target(
      name: "CoxUI",
      resources: [.process("Tokens/Colors.xcassets")],
      plugins: [.plugin(name: "SwiftLintBuildToolPlugin", package: "SwiftLintPlugins")]
    ),
    .testTarget(
      name: "CoxUITests",
      dependencies: [
        "CoxUI",
        .product(name: "SnapshotTesting", package: "swift-snapshot-testing"),
      ],
      exclude: ["__Snapshots__"],
      plugins: [.plugin(name: "SwiftLintBuildToolPlugin", package: "SwiftLintPlugins")]
    ),
  ]
)
