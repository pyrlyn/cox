import SwiftUI

struct PointerModifier: ViewModifier {
  func body(content: Content) -> some View {
    content.pointerStyle(.link)
  }
}
