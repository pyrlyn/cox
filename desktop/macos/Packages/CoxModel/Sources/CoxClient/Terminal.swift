// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A terminal pane's shell as the views see it (T51.5, DT§3.2): cox-ffi's `TerminalHandle`
// reduced to bytes in and bytes out, so CoxPlatform's SwiftTerm view and its tests never load
// the XCFramework. The shell itself runs in Rust (`cox_app::terminal`): Swift never spawns a
// process, and what the shell prints never reaches the session's timeline. The fixture
// terminal stands in for it in previews and tests (DT§8).

import Synchronization

/// One running terminal of a session, implemented over `TerminalHandle` by CoxCore.
public protocol TerminalClient: AnyObject, Sendable {
  /// Keys and pastes, as the terminal view encodes them.
  func write(_ bytes: [UInt8]) throws
  /// The pane's size in cells; the shell gets SIGWINCH.
  func resize(cols: UInt16, rows: UInt16) throws
  /// What the shell prints, batch by batch, until it exits. One consumer at a time.
  var outputs: AsyncStream<[UInt8]> { get }
  /// The shell's exit code once it exited.
  func exitStatus() -> UInt32?
  /// A job other than the shell holds the foreground; closing would kill it, so ask first.
  func isBusy() -> Bool
  /// Hangs the shell up and kills its process groups.
  func close()
}

/// A shell that prints what a test yields and keeps what it was sent: no process behind it.
public final class FixtureTerminal: TerminalClient {
  public let outputs: AsyncStream<[UInt8]>
  private let shell: AsyncStream<[UInt8]>.Continuation
  private let state = Mutex(State())

  private struct State {
    var written: [UInt8] = []
    var isBusy = false
    var isClosed = false
  }

  public init(isBusy: Bool = false) {
    (outputs, shell) = AsyncStream.makeStream(of: [UInt8].self)
    state.withLock { $0.isBusy = isBusy }
  }

  /// Everything written so far.
  public var written: [UInt8] { state.withLock { $0.written } }
  public var isClosed: Bool { state.withLock { $0.isClosed } }

  /// Prints `bytes`, as the shell would.
  public func emit(_ bytes: [UInt8]) { shell.yield(bytes) }

  /// Starts or ends a foreground job.
  public func setBusy(_ isBusy: Bool) { state.withLock { $0.isBusy = isBusy } }

  public func write(_ bytes: [UInt8]) throws { state.withLock { $0.written += bytes } }
  public func resize(cols: UInt16, rows: UInt16) throws {}
  public func exitStatus() -> UInt32? { state.withLock { $0.isClosed ? 0 : nil } }
  public func isBusy() -> Bool { state.withLock { !$0.isClosed && $0.isBusy } }

  public func close() {
    state.withLock { $0.isClosed = true }
    shell.finish()
  }
}
