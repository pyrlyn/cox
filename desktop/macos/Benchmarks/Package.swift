// swift-tools-version: 6.2
// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The app-launch half of the DT§1 budget suite (T37.33): `cox-bench` launches the built
// Cox.app on a 2 000-block fixture and times the first window, then reads the idle app's
// memory. `just desktop-bench` runs it after the in-process transcript budgets
// (CoxTranscript's TranscriptBenchmarkTests); research.md §9.5.14 keeps the numbers.
//
// It lives outside Packages/ on purpose, as Spikes/ does: CI's `swift test` loop globs
// desktop/macos/Packages/*/Package.swift, and a launch needs a built app and a screen, which a
// pull request's job has neither of. Nothing in the app imports this package.
import PackageDescription

let package = Package(
  name: "CoxBenchmarks",
  platforms: [.macOS(.v26)],
  targets: [
    .target(name: "LaunchBench"),
    .executableTarget(name: "cox-bench", dependencies: ["LaunchBench"]),
    .testTarget(name: "LaunchBenchTests", dependencies: ["LaunchBench"]),
  ]
)
