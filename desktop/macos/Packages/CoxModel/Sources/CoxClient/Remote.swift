// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A remote host's workspace (DT§4.4, T52.21): cox on another machine, reached over the person's
// own `ssh` (or in a container or VM, T68.5) as `cox app-server --stdio`, listed in the sidebar as
// its own group and opened like a local session. The protocol CoxModel depends on instead of cox-ffi's `RemoteHandle`; the
// live one is in CoxCore. Separate from `Workspace.swift` because every read crosses the network
// and is async, and the host can drop.

import Foundation

/// One connected host: its projects and sessions, and `open` for one of them. Keys never cross
/// the connection: the host reads its own.
public protocol RemoteWorkspace: CoreClient {
  /// The `ssh` host alias, or the `kind:name` of any other target, it was connected with.
  var host: String { get }
  /// What the target answered to `hello` (T68.5), shown before its first session opens.
  var info: HostInfo { get }
  /// False once the connection dropped, until `reconnect` succeeds.
  var isConnected: Bool { get }
  func reconnect() async throws
  func projects(limit: UInt32) async throws -> [Project]
  func sessions(project: String, limit: UInt32) async throws -> [SessionEntry]
}

/// `cox_app::wire::HostInfo`: the machine a target runs on.
public struct HostInfo: Equatable, Sendable {
  public let wire: UInt32
  public let cox: String
  /// `os`: `macos`, `linux` or `windows`.
  public let platform: String
  public let arch: String
  /// The backend that confines shell commands there; `nil` when it has none (D7).
  public let sandbox: String?

  public init(wire: UInt32, cox: String, platform: String, arch: String, sandbox: String?) {
    (self.wire, self.cox, self.platform, self.arch, self.sandbox) =
      (wire, cox, platform, arch, sandbox)
  }
}

/// `cox_app::TargetChoice`: one target the picker lists.
public struct TargetChoice: Equatable, Sendable {
  public let name: String
  /// What `connect(target:)` takes: a `kind:name`.
  public let target: String
  /// Named in config, as opposed to found running.
  public let configured: Bool

  public init(name: String, target: String, configured: Bool) {
    (self.name, self.target, self.configured) = (name, target, configured)
  }
}

/// Connects to a target (`ssh:<host>`, `docker:<name>`, `lima:<name>`, `colima:<name>` or a
/// configured name); the core refuses a name that could pass for an option before anything runs.
public protocol RemoteConnector: Sendable {
  func connect(target: String) async throws -> any RemoteWorkspace
  /// The configured targets and the containers and VMs running now.
  func targets() async -> [TargetChoice]
}

/// What a remote session cannot answer from here yet: a call the wire has no message for.
public struct RemoteUnsupported: Error, CustomStringConvertible, Equatable {
  public let what: String

  public init(_ what: String) { self.what = what }

  public var description: String { "\(what) is not available for a remote session" }
}
