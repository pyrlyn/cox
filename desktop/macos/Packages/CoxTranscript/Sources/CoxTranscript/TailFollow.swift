// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

// `TailFollow` (T37.23.7, DT§5.2): the transcript stays at its end while a reply streams, as long
// as the reader left it there. Separate from `TranscriptView` because it is scroll state the
// coordinator keeps, not what the transcript draws; the text view it moves stays unaware of it.

import AppKit
import CoxTranscriptText

/// Keeps a transcript at its end while the reader is there: a patch batch that lands while they
/// are at the bottom scrolls its new end into view; one that lands after they scrolled up leaves
/// the view where it is, until they scroll back to the bottom. Whether to follow is decided when
/// the view scrolls, not when a batch lands, so text that grows below a following view between
/// batches (a card taking its height a turn after it lands) does not end the follow. A new text
/// size lays the text out again over several passes, so after one the view moves to each new end
/// until the next batch.
@MainActor
final class TailFollow: NSObject {
  private weak var text: TranscriptTextView?
  /// `nil` until the view first scrolls; a batch then asks where the view is.
  private var following: Bool?
  /// Set while a batch lands and the view follows it, so the scroll that causes is not read as
  /// the reader's.
  private var moving = false
  /// Set by a restyle the view followed, until the next batch (`resized`).
  private var pinned = false

  init(_ text: TranscriptTextView) {
    self.text = text
    super.init()
    guard let clip = text.enclosingScrollView?.contentView else { return }
    clip.postsBoundsChangedNotifications = true
    // A selector observer goes with its object, so nothing has to remove it.
    NotificationCenter.default.addObserver(
      self, selector: #selector(scrolled), name: NSView.boundsDidChangeNotification, object: clip)
    text.postsFrameChangedNotifications = true
    NotificationCenter.default.addObserver(
      self, selector: #selector(resized), name: NSView.frameDidChangeNotification, object: text)
  }

  /// Runs `edit`, then shows the text's end if the reader was following it; `restyling`, keeps
  /// showing it while the restyled text is laid out.
  func around(restyling: Bool = false, _ edit: () -> Void) {
    let follow = following ?? atBottom
    pinned = restyling && follow
    moving = true
    defer { moving = false }
    edit()
    guard follow, let text else { return }
    following = true
    // The scroll stops at the last line's glyphs and the frame grows by the space its line
    // height leaves under it (A93) only when the viewport is laid out, so lay it out now and
    // take the clip view to the text's bottom.
    text.scrollToEndOfDocument(nil)
    text.textLayoutManager?.textViewportLayoutController.layoutViewport()
    showBottom()
  }

  /// Restyled text laid out again below a following view keeps the end in view (T37.23.6). Only
  /// after a restyle: a batch scrolls itself, and a frame change is too common to scroll on each.
  @objc private func resized() {
    guard pinned, following == true, !moving else { return }
    moving = true
    defer { moving = false }
    // The clip view, not `scrollToEndOfDocument`: while the frame changes, TextKit has not
    // laid out the new last line that scroll would reveal, and the view stays put.
    showBottom()
  }

  /// Moves the clip view to the text's bottom edge.
  private func showBottom() {
    guard let scroll = text?.enclosingScrollView, let document = scroll.documentView else {
      return
    }
    let clip = scroll.contentView
    clip.scroll(to: NSPoint(x: 0, y: max(0, document.frame.maxY - clip.bounds.height)))
    scroll.reflectScrolledClipView(clip)
  }

  @objc private func scrolled() {
    guard !moving else { return }
    following = atBottom
  }

  /// Whether the view shows the end of its text, within a rounding slack: AppKit rounds a scroll
  /// to the backing's pixels.
  private var atBottom: Bool {
    guard let scroll = text?.enclosingScrollView, let document = scroll.documentView else {
      return false
    }
    return scroll.documentVisibleRect.maxY >= document.bounds.maxY - 1
  }
}
