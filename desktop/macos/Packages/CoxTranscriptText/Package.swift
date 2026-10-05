// swift-tools-version: 6.2
// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// CoxTranscriptText (DT§4.6, DT§5.2): the transcript as one TextKit 2
// `NSTextView` with every timeline block a tracked text range, the selection
// engine spike T37.37 chose (research.md §9.5.13). Depends on `CoxClient`'s
// value types only, not on CoxUI: CoxUI's transcript organism hosts this view
// and hands it a style built from its tokens (T37.23).
import PackageDescription

/// desktop/macos/.swiftlint.yml, through Packages/CoxTranscriptText/.swiftlint.yml.
let swiftLint = Target.PluginUsage.plugin(
  name: "SwiftLintBuildToolPlugin", package: "SwiftLintPlugins")

let package = Package(
  name: "CoxTranscriptText",
  platforms: [.macOS(.v26)],
  products: [
    .library(name: "CoxTranscriptText", targets: ["CoxTranscriptText"])
  ],
  dependencies: [
    .package(path: "../CoxModel"),
    .package(url: "https://github.com/SimplyDanny/SwiftLintPlugins", exact: "0.65.1"),
  ],
  targets: [
    .target(
      name: "CoxTranscriptText",
      dependencies: [.product(name: "CoxClient", package: "CoxModel")],
      plugins: [swiftLint]
    ),
    .testTarget(
      name: "CoxTranscriptTextTests",
      dependencies: [
        "CoxTranscriptText",
        .product(name: "CoxClient", package: "CoxModel"),
      ],
      plugins: [swiftLint]
    ),
  ]
)
