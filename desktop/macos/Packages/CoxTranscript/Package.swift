// swift-tools-version: 6.2
// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// CoxTranscript (DT§4.6, DT§5.2): the transcript organism, where the three
// view-layer packages meet — `CoxTranscriptText`'s TextKit 2 view, CoxUI's
// cards and tokens, and the session's timeline from CoxModel. Its own package
// so CoxUI keeps importing no other cox package and CoxTranscriptText keeps
// knowing nothing of CoxUI; only this layer maps a timeline block to a card.
import PackageDescription

/// desktop/macos/.swiftlint.yml, through Packages/CoxTranscript/.swiftlint.yml.
let swiftLint = Target.PluginUsage.plugin(
  name: "SwiftLintBuildToolPlugin", package: "SwiftLintPlugins")

let package = Package(
  name: "CoxTranscript",
  platforms: [.macOS(.v26)],
  products: [
    .library(name: "CoxTranscript", targets: ["CoxTranscript"])
  ],
  dependencies: [
    .package(path: "../CoxModel"),
    .package(path: "../CoxTranscriptText"),
    .package(path: "../CoxUI"),
    .package(url: "https://github.com/SimplyDanny/SwiftLintPlugins", exact: "0.65.1"),
    .package(url: "https://github.com/pointfreeco/swift-snapshot-testing", exact: "1.19.6"),
  ],
  targets: [
    .target(
      name: "CoxTranscript",
      dependencies: [
        .product(name: "CoxClient", package: "CoxModel"),
        .product(name: "CoxModel", package: "CoxModel"),
        "CoxTranscriptText",
        "CoxUI",
      ],
      plugins: [swiftLint]
    ),
    .testTarget(
      name: "CoxTranscriptTests",
      dependencies: [
        "CoxTranscript",
        .product(name: "CoxClient", package: "CoxModel"),
        .product(name: "CoxModel", package: "CoxModel"),
        "CoxTranscriptText",
        "CoxUI",
        .product(name: "SnapshotTesting", package: "swift-snapshot-testing"),
      ],
      exclude: ["__Snapshots__"],
      plugins: [swiftLint]
    ),
  ]
)
