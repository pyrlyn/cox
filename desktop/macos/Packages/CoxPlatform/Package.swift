// swift-tools-version: 6.2
// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// CoxPlatform (DT§4.6): what the app asks macOS for — today the provider
// keys in the Keychain over the Security framework, no wrapper package
// (research.md 9.5.8) — and notifications with Allow, Deny and Answer
// actions (T37.27); the terminal pane over SwiftTerm (T51.5); the agent's browser page
// over WebKit's `WebPage` (T51.9, no dependency); later Sparkle and the OAuth handoff.
// Depends on CoxClient only, for the `SecretStore` seam, so it builds and
// tests without the Rust XCFramework.
import PackageDescription

/// desktop/macos/.swiftlint.yml, through Packages/CoxPlatform/.swiftlint.yml.
let swiftLint = Target.PluginUsage.plugin(
  name: "SwiftLintBuildToolPlugin", package: "SwiftLintPlugins")

let package = Package(
  name: "CoxPlatform",
  platforms: [.macOS(.v26)],
  products: [
    .library(name: "CoxPlatform", targets: ["CoxPlatform"])
  ],
  dependencies: [
    .package(path: "../CoxModel"),
    // T51.5: the terminal pane's emulator and renderer (research.md §9.5.2, MIT).
    .package(url: "https://github.com/migueldeicaza/SwiftTerm", from: "1.20.0"),
    .package(url: "https://github.com/SimplyDanny/SwiftLintPlugins", exact: "0.65.1"),
  ],
  targets: [
    .target(
      name: "CoxPlatform",
      dependencies: [
        .product(name: "CoxClient", package: "CoxModel"),
        .product(name: "SwiftTerm", package: "SwiftTerm"),
      ],
      linkerSettings: [.linkedFramework("Security")],
      plugins: [swiftLint]
    ),
    .testTarget(
      name: "CoxPlatformTests",
      dependencies: [
        "CoxPlatform", .product(name: "CoxClient", package: "CoxModel"),
        .product(name: "SwiftTerm", package: "SwiftTerm"),
      ],
      plugins: [swiftLint]
    ),
  ]
)
