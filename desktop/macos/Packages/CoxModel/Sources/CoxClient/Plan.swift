// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// What the inspector's Plan tab lists (T37.29.2, DT§5.1), field for field as cox-ffi exports
// cox-protocol's `TodoItem`: the list the latest `todo` call left, as cox-app's timeline folded
// it. Separate from the timeline because it answers a call, not a patch.

/// One step of the plan.
public struct TodoItem: Equatable, Sendable {
  /// Where a step stands.
  public enum State: Equatable, Sendable {
    case pending, inProgress, done
  }

  /// Unique within the list.
  public var id: String
  public var text: String
  public var state: State

  public init(id: String, text: String, state: State) {
    (self.id, self.text, self.state) = (id, text, state)
  }
}
