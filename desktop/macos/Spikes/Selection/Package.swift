// swift-tools-version: 6.2
// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// Spike T37.37 (A67): which engine selects transcript text across blocks.
// Throwaway code, kept so the numbers in research.md §9.5 can be reproduced:
//
//   swift test --no-parallel --package-path desktop/macos/Spikes/Selection 2>&1 | grep MEASURE
//
// `--no-parallel` matters: both suites run on the main actor, and interleaved they
// time each other.
//
// It lives outside Packages/ on purpose: CI's `swift test` loop globs
// desktop/macos/Packages/*/Package.swift and the lint jobs scan Packages/ and
// LintFixtures/ only, and .swiftlint.yml excludes Spikes/. Literals are allowed
// here; nothing in the app may import this package.
import PackageDescription

let package = Package(
  name: "SelectionSpike",
  platforms: [.macOS(.v26)],
  dependencies: [
    // research.md §9.5.10: MIT, 0.5.0 (2026-06-15).
    .package(url: "https://github.com/gonzalezreal/textual", exact: "0.5.0")
  ],
  targets: [
    .target(
      name: "SelectionSpike",
      dependencies: [.product(name: "Textual", package: "textual")]
    ),
    .testTarget(
      name: "SelectionSpikeTests",
      dependencies: ["SelectionSpike", .product(name: "Textual", package: "textual")]
    ),
  ]
)
