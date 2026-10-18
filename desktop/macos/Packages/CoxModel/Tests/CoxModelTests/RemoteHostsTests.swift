// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A remote host in the sidebar (T52.21): a connected host lists its sessions as one group of its
// own and routes their opening to itself; a host that cannot connect still shows, disconnected,
// with its rows read-only.

import CoxClient
import Foundation
import Testing

@testable import CoxModel

private struct Unreachable: Error {}

private struct FakeRemote: RemoteWorkspace {
  let host: String
  let isConnected = true
  let info = HostInfo(wire: 1, cox: "0.9.1", platform: "linux", arch: "x86_64", sandbox: "bwrap")

  func reconnect() async throws {}

  func projects(limit: UInt32) async throws -> [Project] {
    [Project(root: "/srv/api", name: "api")]
  }

  func sessions(project: String, limit: UInt32) async throws -> [SessionEntry] {
    [SessionEntry(id: "far", title: "Fix the deploy", cwd: "/srv/api", costUsd: 0.5)]
  }

  func open(_ request: OpenSession) async throws -> any SessionClient { throw Unreachable() }
}

private struct FakeConnector: RemoteConnector {
  func connect(target: String) async throws -> any RemoteWorkspace {
    guard ["ssh:devbox", "docker:web"].contains(target) else { throw Unreachable() }
    return FakeRemote(host: TargetList.hostID(target))
  }

  func targets() async -> [TargetChoice] {
    [TargetChoice(name: "web", target: "docker:web", configured: false)]
  }
}

@MainActor
@Suite struct RemoteHostsTests {
  @Test func aConnectedHostIsOneGroupThatOpensItsOwnSessions() async {
    let remotes = RemoteHosts(connector: FakeConnector(), settings: nil)
    #expect(await remotes.connect("ssh:devbox") == nil)
    let sections = remotes.sections
    #expect(sections.map(\.id) == ["host:devbox"])
    #expect(sections[0].kind == .host(isConnected: true))
    #expect(sections[0].rows.map(\.title) == ["Fix the deploy"])
    #expect(sections[0].rows.map(\.subtitle) == ["api"])
    #expect(sections[0].rows.allSatisfy { !$0.isReadOnly })
    #expect(remotes.workspace(for: "far")?.cwd == "/srv/api")
    #expect(remotes.workspace(for: "local") == nil)
    #expect(RemoteHosts.host(section: sections[0].id) == "devbox")
  }

  @Test func aHostThatCannotConnectShowsDisconnected() async {
    let remotes = RemoteHosts(connector: FakeConnector(), settings: nil)
    #expect(await remotes.connect("ssh:gone") != nil)
    #expect(remotes.sections.map(\.kind) == [.host(isConnected: false)])
    #expect(remotes.hosts.first?.failure != nil)
  }

  @Test func aTargetOfAnotherKindKeepsItsPrefixAndShowsItsHello() async {
    let remotes = RemoteHosts(connector: FakeConnector(), settings: nil)
    #expect(await remotes.connect("docker:web") == nil)
    #expect(remotes.sections.map(\.id) == ["host:docker:web"])
    #expect(RemoteHosts.host(section: "host:docker:web") == "docker:web")
    #expect(remotes.hello(for: "docker:web")?.sandbox == "bwrap")
    #expect(await remotes.targets().map(\.target) == ["docker:web"])
  }

  @Test func anSshTargetIsAGroupNamedByItsBareAlias() async {
    let remotes = RemoteHosts(connector: FakeConnector(), settings: nil)
    #expect(await remotes.connect("ssh:devbox", save: false) == nil)
    #expect(remotes.hosts.map(\.id) == ["devbox"])
    #expect(remotes.hello(for: "ssh:devbox")?.summary == "Linux · x86_64 · cox 0.9.1")
  }
}
