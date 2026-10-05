// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// The app's remote workspaces (DT§4.4, T52.21): each host File › Connect to Host… or
// `desktop.remote_hosts` named, whether its connection holds, and its sessions as a sidebar
// group of their own. Beside `AppStore`, not in it: the registry holds open sessions whatever
// core they came from, this store holds the connections a remote one opens through. The saved
// list is user config; a project config cannot set it (the project-config guard).

import CoxClient
import Foundation
import Observation

@Observable
@MainActor
public final class RemoteHosts {
  /// One host as the sidebar shows it.
  public struct Host: Identifiable, Equatable, Sendable {
    /// The `ssh` host alias.
    public let id: String
    public var isConnected: Bool
    /// Every session of every project the host listed, newest project first.
    public var sessions: [(project: Project, entry: SessionEntry)] = []
    /// Why the last connect or read failed; the next success clears it.
    public var failure: String?

    public static func == (lhs: Host, rhs: Host) -> Bool {
      lhs.id == rhs.id && lhs.isConnected == rhs.isConnected && lhs.failure == rhs.failure
        && lhs.sessions.map(\.entry) == rhs.sessions.map(\.entry)
    }
  }

  public private(set) var hosts: [Host] = []
  /// Set while a connect runs; the sheet waits on it.
  public private(set) var isConnecting = false
  @ObservationIgnored private var workspaces: [String: any RemoteWorkspace] = [:]
  @ObservationIgnored private let connector: (any RemoteConnector)?
  @ObservationIgnored private let settings: SettingsStore?
  @ObservationIgnored private var isWatching = false

  /// A fixture launch has no connector: the File menu item stays disabled.
  public init(connector: (any RemoteConnector)?, settings: SettingsStore?) {
    (self.connector, self.settings) = (connector, settings)
  }

  public var canConnect: Bool { connector != nil }

  /// The projects a host lists, and the sessions it lists of each: the local sidebar's limits.
  static let projectLimit: UInt32 = 20
  static let sessionLimit: UInt32 = 20

  /// `desktop.remote_hosts` as the settings view read it.
  public var saved: [String] {
    settings?.view.flatMap { SectionRows($0.settings, "desktop").decode("remote_hosts") } ?? []
  }

  /// Connects the saved hosts, then re-reads every host each `every` until cancelled: one loop
  /// per launch, however many windows ask.
  public func watch(every: Duration = .seconds(5)) async {
    guard !isWatching else { return }
    isWatching = true
    defer { isWatching = false }
    await connectSaved()
    while !Task.isCancelled {
      try? await Task.sleep(for: every)
      guard !Task.isCancelled else { return }
      await refresh()
    }
  }

  /// Connects every saved host not listed yet; one that fails shows disconnected.
  public func connectSaved() async {
    if settings?.view == nil { await settings?.load() }
    for host in saved where !hosts.contains(where: { $0.id == host }) {
      _ = await connect(host, save: false)
    }
  }

  /// Connects to `host` and lists its sessions; a new alias joins `desktop.remote_hosts` in the
  /// user config. Returns why it failed, for the sheet.
  public func connect(_ host: String, save: Bool = true) async -> String? {
    let host = host.trimmingCharacters(in: .whitespacesAndNewlines)
    guard let connector, !host.isEmpty else { return nil }
    isConnecting = true
    defer { isConnecting = false }
    do {
      let workspace = try await connector.connect(host: host)
      workspaces[host] = workspace
      await read(host, workspace)
      if save, !saved.contains(host) {
        await settings?.set("desktop.remote_hosts", to: .list(saved + [host]))
      }
      return nil
    } catch {
      let failure = String(describing: error)
      update(host) { ($0.isConnected, $0.failure) = (false, failure) }
      return failure
    }
  }

  /// The group's Reconnect: the same workspace dials again, or a new one when none connected.
  public func reconnect(_ host: String) async {
    guard let workspace = workspaces[host] else {
      _ = await connect(host, save: false)
      return
    }
    do {
      try await workspace.reconnect()
      await read(host, workspace)
    } catch {
      let failure = String(describing: error)
      update(host) { ($0.isConnected, $0.failure) = (false, failure) }
    }
  }

  /// Reads every connected host's list again; a dropped one turns disconnected.
  public func refresh() async {
    for (host, workspace) in workspaces {
      if workspace.isConnected {
        await read(host, workspace)
      } else {
        update(host) { $0.isConnected = false }
      }
    }
  }

  /// The host a listed session lives on and its cwd there; `nil` for a local session.
  public func workspace(for session: String) -> (workspace: any RemoteWorkspace, cwd: String)? {
    for host in hosts {
      guard let entry = host.sessions.first(where: { $0.entry.id == session })?.entry,
        let workspace = workspaces[host.id]
      else { continue }
      return (workspace, entry.cwd)
    }
    return nil
  }

  /// One sidebar group per host, after the local projects.
  public var sections: [SidebarSection] {
    hosts.map { host in
      SidebarSection(
        id: "host:\(host.id)", title: host.id, kind: .host(isConnected: host.isConnected),
        rows: host.sessions.map { listed in
          SidebarRow(
            id: listed.entry.id, session: listed.entry.id, status: .idle, title: listed.entry.name,
            subtitle: listed.project.name,
            cost: listed.entry.costUsd > 0 ? usd(listed.entry.costUsd) : nil,
            isReadOnly: !host.isConnected)
        })
    }
  }

  /// The alias a section id names, for the group's Reconnect.
  public static func host(section: String) -> String? {
    section.hasPrefix("host:") ? String(section.dropFirst("host:".count)) : nil
  }

  private func read(_ host: String, _ workspace: any RemoteWorkspace) async {
    do {
      var sessions: [(project: Project, entry: SessionEntry)] = []
      for project in try await workspace.projects(limit: Self.projectLimit) {
        for entry in try await workspace.sessions(project: project.root, limit: Self.sessionLimit) {
          sessions.append((project, entry))
        }
      }
      update(host) { ($0.isConnected, $0.sessions, $0.failure) = (true, sessions, nil) }
    } catch {
      let failure = String(describing: error)
      update(host) { ($0.isConnected, $0.failure) = (workspace.isConnected, failure) }
    }
  }

  private func update(_ host: String, _ change: (inout Host) -> Void) {
    if let index = hosts.firstIndex(where: { $0.id == host }) {
      change(&hosts[index])
    } else {
      var new = Host(id: host, isConnected: false)
      change(&new)
      hosts.append(new)
    }
  }
}
