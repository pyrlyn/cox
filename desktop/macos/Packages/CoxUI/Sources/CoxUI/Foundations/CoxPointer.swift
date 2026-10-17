// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `.coxPointer(_:)` (DS§6.1, DS§8): the pointer over an element, decided by the core's shared
// rule (`cox_render::pointer`, A141) so every client answers the same. Separate so this is the
// only file that touches `PointerStyle` and `NSCursor` (the `no_pointer_style_outside_foundations`
// lint rule keeps it so), and so CoxUI, which links no Rust, still has one seam to the rule: the
// app injects it through `pointerRule`, and this file only maps the answer to the platform.

import AppKit
import SwiftUI

/// What is known about the element under the pointer; the core decides the shape from it.
public struct PointerInteraction: Equatable, Sendable {
  /// A click does something: a button, a link, a row that opens.
  public var clickable: Bool
  /// Its text can be selected and copied.
  public var selectable: Bool
  /// Already chosen or running: the selected row, the active segment, a button in flight.
  public var pressed: Bool
  /// Disabled or locked.
  public var blocked: Bool

  public init(
    clickable: Bool = false, selectable: Bool = false, pressed: Bool = false,
    blocked: Bool = false
  ) {
    (self.clickable, self.selectable, self.pressed, self.blocked) =
      (clickable, selectable, pressed, blocked)
  }

  /// A control a click acts on, unless `isEnabled` is off or it `isChosen` already.
  public static func control(isEnabled: Bool = true, isChosen: Bool = false) -> Self {
    Self(clickable: true, pressed: isChosen, blocked: !isEnabled)
  }

  /// Text that can be selected and copied.
  public static let selectableText = Self(selectable: true)
}

/// Which way a drag handle moves.
public enum PointerEdge: Equatable, Sendable {
  /// A horizontal divider: the drag moves along the vertical axis.
  case vertical
  /// A vertical divider: the drag moves along the horizontal axis.
  case horizontal
}

/// The core's answer: one case per `Pointer` of `cox_render::pointer`.
public enum PointerShape: Equatable, Sendable {
  case `default`, action, text, notAllowed
  /// A drag handle; the core never answers it, the handle's view names it (`coxPointer(handle:)`).
  case resize(PointerEdge)
}

extension PointerShape {
  /// The platform style, `nil` for the one `PointerStyle` has no value for.
  var style: PointerStyle? {
    switch self {
    case .default: .default
    case .action: .link
    case .text: .horizontalText
    case .notAllowed: nil
    case .resize(.vertical): .rowResize
    case .resize(.horizontal): .columnResize
    }
  }

  /// `PointerStyle` has no not-allowed pointer; `NSCursor.operationNotAllowed` draws it.
  var cursor: NSCursor? {
    self == .notAllowed ? .operationNotAllowed : nil
  }
}

/// The core's `pointer(Interaction) -> Pointer`, handed to CoxUI by the app. CoxUI keeps no rule
/// of its own, so a preview or snapshot with none injected shows the arrow everywhere.
public struct PointerRule: Sendable {
  let shape: @Sendable (PointerInteraction) -> PointerShape

  public init(_ shape: @escaping @Sendable (PointerInteraction) -> PointerShape) {
    self.shape = shape
  }

  public static let arrow = PointerRule { _ in .default }
}

extension EnvironmentValues {
  /// The shared pointer rule; the app sets it at every scene root.
  @Entry public var pointerRule = PointerRule.arrow
}

extension ControlState {
  /// A catalogue control's facts: clickable, blocked while disabled, pressed when `isChosen`.
  func interaction(isChosen: Bool = false) -> PointerInteraction {
    .control(isEnabled: self != .disabled, isChosen: isChosen)
  }
}

extension View {
  /// The pointer the shared rule gives an element with these facts.
  public func coxPointer(_ interaction: PointerInteraction) -> some View {
    modifier(RuledPointer(interaction: interaction))
  }

  /// A control's pointer from its own `isEnabled`, for one that keeps a system style (a `Menu`,
  /// whose layout a custom `ButtonStyle` would change); `isChosen` as in `coxPointer(_:)`.
  public func coxControlPointer(isChosen: Bool = false) -> some View {
    modifier(ControlPointer(isChosen: isChosen))
  }

  /// The pointer over a drag handle, which is not an `Interaction`: named directly.
  public func coxPointer(handle edge: PointerEdge) -> some View {
    modifier(PointerFace(shape: .resize(edge)))
  }
}

private struct ControlPointer: ViewModifier {
  let isChosen: Bool
  @Environment(\.isEnabled) private var isEnabled

  func body(content: Content) -> some View {
    content.coxPointer(.control(isEnabled: isEnabled, isChosen: isChosen))
  }
}

private struct RuledPointer: ViewModifier {
  let interaction: PointerInteraction
  @Environment(\.pointerRule) private var rule

  func body(content: Content) -> some View {
    content.modifier(PointerFace(shape: rule.shape(interaction)))
  }
}

private struct PointerFace: ViewModifier {
  let shape: PointerShape

  func body(content: Content) -> some View {
    content
      .pointerStyle(shape.style)
      .overlay { if shape.cursor != nil { NotAllowedCursor() } }
  }
}

/// A transparent layer that sets the not-allowed cursor over its frame. A tracking area with
/// `cursorUpdate` sits beside the cursor rect because SwiftUI's own pointer styles arrive the
/// same way; the layer never takes a click.
private struct NotAllowedCursor: NSViewRepresentable {
  func makeNSView(context: Context) -> NSView { CursorView() }
  func updateNSView(_ view: NSView, context: Context) {}

  private final class CursorView: NSView {
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    override func updateTrackingAreas() {
      super.updateTrackingAreas()
      trackingAreas.forEach(removeTrackingArea)
      addTrackingArea(
        NSTrackingArea(
          rect: .zero, options: [.cursorUpdate, .activeInKeyWindow, .inVisibleRect],
          owner: self, userInfo: nil))
    }

    override func cursorUpdate(with event: NSEvent) { NSCursor.operationNotAllowed.set() }

    override func resetCursorRects() {
      addCursorRect(bounds, cursor: .operationNotAllowed)
    }
  }
}
