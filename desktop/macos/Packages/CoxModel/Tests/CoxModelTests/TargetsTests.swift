// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The Connect to Target sheet's list (T68.5): grouped by kind, configured ones first, saved ssh
// hosts as `ssh:<alias>`, a typed name checked before it goes to the core, and `hello` as text.

import CoxClient
import Testing

@testable import CoxModel

@Suite struct TargetsTests {
  private func choice(_ name: String, _ target: String, _ configured: Bool) -> TargetChoice {
    TargetChoice(name: name, target: target, configured: configured)
  }

  @Test func targetsGroupByKindWithConfiguredFirst() {
    let options = TargetList.options(
      [
        choice("docker:db", "docker:db", false), choice("lima:dev", "lima:dev", false),
        choice("web", "docker:web", true), choice("lab", "colima:lab", true),
      ], hosts: [])
    #expect(options.map(\.target) == ["docker:web", "docker:db", "lima:dev", "colima:lab"])
    #expect(options.map(\.kind) == [.docker, .docker, .lima, .colima])
  }

  @Test func savedSshHostsBecomeSshTargetsWithoutDuplicates() {
    let options = TargetList.options(
      [choice("devbox", "ssh:devbox", true), choice("bare", "bare", true)],
      hosts: ["devbox", "gpu"])
    #expect(options.map(\.target) == ["ssh:devbox", "ssh:gpu"])
    #expect(options.allSatisfy { $0.isConfigured })
    #expect(options.map(\.name) == ["devbox", "gpu"])
  }

  @Test func aTypedNameBecomesAKindAndNameOrNothing() {
    #expect(TargetList.text(kind: .docker, name: "  web ") == "docker:web")
    #expect(TargetList.text(kind: .ssh, name: "dev box") == nil)
    #expect(TargetList.text(kind: .lima, name: "  ") == nil)
    #expect(TargetList.text(kind: .colima, name: "a\tb") == nil)
  }

  @Test func aHostIDRoundTripsToItsTarget() {
    #expect(TargetList.hostID("ssh:devbox") == "devbox")
    #expect(TargetList.hostID("docker:web") == "docker:web")
    #expect(TargetList.text(hostID: "devbox") == "ssh:devbox")
    #expect(TargetList.text(hostID: "docker:web") == "docker:web")
  }

  @Test func helloNamesTheMachineAndWhetherItHasASandbox() {
    let mac = HostHello(
      HostInfo(wire: 1, cox: "1.2.3", platform: "macos", arch: "aarch64", sandbox: "seatbelt"))
    #expect(mac == HostHello(summary: "macOS · arm64 · cox 1.2.3", sandbox: "seatbelt"))
    let bare = HostHello(
      HostInfo(wire: 1, cox: "1.2.3", platform: "plan9", arch: "mips", sandbox: nil))
    #expect(bare.summary == "plan9 · mips · cox 1.2.3")
    #expect(bare.sandbox == nil)
  }
}
