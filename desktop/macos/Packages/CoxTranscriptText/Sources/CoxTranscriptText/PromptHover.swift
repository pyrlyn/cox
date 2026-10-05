// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// A hovered prompt's actions (T37.23.9, DT§5.2): while the pointer is over a prompt, the view
// `TranscriptCards.promptActions` makes for it floats over its bubble's top trailing corner, and
// `promptGutter`'s over its turn number (T37.48, Figma frame 14), which opens `promptMenu` in a
// popover. Its own file because it is the one place the transcript follows the pointer; the
// actions and what they do are the caller's.

import AppKit
import CoxClient
import SwiftUI

extension TranscriptTextView {
  /// Marks the tracking area that follows the pointer for prompts, apart from AppKit's own.
  private static let promptTracking = "cox.transcript.prompts"

  func trackPrompts() {
    let options: NSTrackingArea.Options = [
      .mouseMoved, .mouseEnteredAndExited, .activeInActiveApp, .inVisibleRect,
    ]
    addTrackingArea(
      NSTrackingArea(
        rect: .zero, options: options, owner: self, userInfo: [Self.promptTracking: true]))
  }

  override public func mouseMoved(with event: NSEvent) {
    super.mouseMoved(with: event)
    hover(at: convert(event.locationInWindow, from: nil))
  }

  override public func mouseExited(with event: NSEvent) {
    super.mouseExited(with: event)
    if event.trackingArea?.userInfo?[Self.promptTracking] != nil { hover(at: nil) }
  }

  /// The prompt whose actions show, if any.
  public var hoveredPrompt: BlockID? { promptHover?.id }

  /// Shows the actions of the prompt under `point`, in this view's coordinates; `nil`, or a
  /// point off every prompt, hides them. A point on the actions keeps them.
  public func hover(at point: NSPoint?) {
    if let point, let shown = promptHover,
      shown.view.frame.contains(point) || shown.gutter?.frame.contains(point) == true
    {
      return
    }
    let id = point.flatMap(prompt(at:))
    guard id != promptHover?.id else { return }
    promptHover?.view.removeFromSuperview()
    promptHover?.gutter?.removeFromSuperview()
    promptHover = nil
    guard let id, let block = blocks[id], let make = cards.promptActions, let top = bubbleTop(id)
    else { return }
    let host = NSHostingView(rootView: make(block))
    let size = host.fittingSize
    // Inside the bubble's top trailing corner, in by its own padding: above or across its edge
    // the first prompt's strip would be cut off by the top of the view.
    let padding = style.bubble.padding
    host.frame = NSRect(
      origin: NSPoint(x: top.maxX - padding.width - size.width, y: top.minY + padding.height),
      size: size)
    // The gutter first: the actions stay the last subview, over it.
    let gutter = gutterHost(block)
    gutter.map { addSubview($0) }
    addSubview(host)
    promptHover = (id, host, gutter)
  }

  /// `promptGutter`'s view for `block`, over the number its fragment draws and on the view's
  /// background, so the drawn number does not show through.
  private func gutterHost(_ block: Block) -> NSView? {
    guard let make = cards.promptGutter, let box = gutterBox(block.id) else { return nil }
    let host = NSHostingView(rootView: make(block) { [weak self] in self?.openMenu(block.id) })
    let height = max(host.fittingSize.height, box.height)
    host.frame = NSRect(x: box.minX, y: box.midY - height / 2, width: box.width, height: height)
    host.wantsLayer = true
    effectiveAppearance.performAsCurrentDrawingAppearance {
      host.layer?.backgroundColor = backgroundColor.cgColor
    }
    return host
  }

  /// Shows `promptMenu` for prompt `id` in a popover under its turn number.
  public func openMenu(_ id: BlockID) {
    guard let block = blocks[id], let make = cards.promptMenu, let box = gutterBox(id) else {
      return
    }
    promptMenu?.close()
    let popover = NSPopover()
    popover.behavior = .transient
    popover.contentViewController = NSHostingController(
      rootView: make(block) { [weak self, weak popover] in
        popover?.close()
        if self?.promptMenu === popover { self?.promptMenu = nil }
      })
    promptMenu = popover
    popover.show(relativeTo: box, of: self, preferredEdge: .maxY)
  }

  /// Where prompt `id`'s fragment draws its turn number, in this view's coordinates.
  func gutterBox(_ id: BlockID) -> CGRect? {
    guard let (fragment, decor, edge) = bubbleFragment(id),
      let box = decor.gutterArea(fragment, edge)
    else { return nil }
    let frame = fragment.layoutFragmentFrame
    return box.offsetBy(
      dx: frame.minX + textContainerOrigin.x, dy: frame.minY + textContainerOrigin.y)
  }

  /// The prompt block laid out at `point`, the space after it included.
  private func prompt(at point: NSPoint) -> BlockID? {
    guard let manager = textLayoutManager, let content = manager.textContentManager else {
      return nil
    }
    let origin = textContainerOrigin
    // Left of the column too: the turn number in the gutter belongs to its prompt.
    let inContainer = CGPoint(x: max(point.x - origin.x, 0), y: point.y - origin.y)
    guard let fragment = manager.textLayoutFragment(for: inContainer) else { return nil }
    let offset = content.offset(
      from: content.documentRange.location, to: fragment.rangeInElement.location)
    guard let id = blockID(at: offset), case .user = blocks[id]?.kind else { return nil }
    return id
  }

  /// The top slice of prompt `id`'s bubble in this view's coordinates, as its fragment draws it.
  private func bubbleTop(_ id: BlockID) -> CGRect? {
    guard let (fragment, decor, edge) = bubbleFragment(id), let area = decor.area(fragment, edge)
    else { return nil }
    let frame = fragment.layoutFragmentFrame
    return area.offsetBy(
      dx: frame.minX + textContainerOrigin.x, dy: frame.minY + textContainerOrigin.y)
  }

  /// Prompt `id`'s first paragraph and its bubble.
  private func bubbleFragment(_ id: BlockID) -> (DecorFragment, Decor, Decor.Edge)? {
    guard let range = range(of: id), range.length > 0,
      let location = textRange(range)?.location,
      let fragment = textLayoutManager?.textLayoutFragment(for: location) as? DecorFragment,
      let (decor, edge) = fragment.decoration, decor.kind == .bubble
    else { return nil }
    return (fragment, decor, edge)
  }
}
