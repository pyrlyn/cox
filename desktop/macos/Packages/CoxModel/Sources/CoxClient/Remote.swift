// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A remote host's workspace (DT§4.4, T52.21): cox on another machine, reached over the person's
// own `ssh` as `cox app-server --stdio`, listed in the sidebar as its own group and opened like
// a local session. The protocol CoxModel depends on instead of cox-ffi's `RemoteHandle`; the
// live one is in CoxCore. Separate from `Workspace.swift` because every read crosses the network
// and is async, and the host can drop.

import Foundation

/// One connected host: its projects and sessions, and `open` for one of them. Keys never cross
/// the connection: the host reads its own.
public protocol RemoteWorkspace: CoreClient {
  /// The `ssh` host alias it was connected with.
  var host: String { get }
  /// False once the connection dropped, until `reconnect` succeeds.
  var isConnected: Bool { get }
  func reconnect() async throws
  func projects(limit: UInt32) async throws -> [Project]
  func sessions(project: String, limit: UInt32) async throws -> [SessionEntry]
}

/// Connects to a host by its `ssh` alias; the core refuses an alias that could pass for an
/// `ssh` option before anything runs.
public protocol RemoteConnector: Sendable {
  func connect(host: String) async throws -> any RemoteWorkspace
}

/// What a remote session cannot answer from here yet: a call the wire has no message for.
public struct RemoteUnsupported: Error, CustomStringConvertible, Equatable {
  public let what: String

  public init(_ what: String) { self.what = what }

  public var description: String { "\(what) is not available for a remote session" }
}
