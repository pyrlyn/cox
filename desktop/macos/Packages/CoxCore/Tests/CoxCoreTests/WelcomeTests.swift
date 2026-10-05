// The welcome hero's facts through the real Rust core (T37.49's Check, Figma frame 22): this
// repository reads as the Rust workspace it is, with its instruction file loaded, and its
// suggestions include the diff review, since it is a git checkout.

import CoxClient
import Foundation
import Testing

@testable import CoxCore

@Test func thisRepositoryReadsAsARustWorkspaceWithItsInstructions() async throws {
  let (home, client) = try scratch()
  defer { try? FileManager.default.removeItem(at: home) }
  // Tests/CoxCoreTests/WelcomeTests.swift → the repository root, six folders up.
  var root = URL(fileURLWithPath: #filePath)
  for _ in 0..<7 { root.deleteLastPathComponent() }

  let facts = try await client.welcome(cwd: root.path())
  #expect(facts.summary.hasPrefix("Rust workspace · "), "\(facts.summary)")
  #expect(facts.summary.contains("AGENTS.md"), "\(facts.summary)")
  #expect(facts.suggestions.map(\.title).contains("Review my uncommitted diff"))
}
