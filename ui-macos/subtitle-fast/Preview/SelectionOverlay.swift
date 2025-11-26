import SwiftUI

enum SelectionHandle: CaseIterable {
    case topLeft
    case topRight
    case bottomLeft
    case bottomRight
    case move

    static var cornerCases: [SelectionHandle] {
        [.topLeft, .topRight, .bottomLeft, .bottomRight]
    }

    func position(in rect: CGRect) -> CGPoint {
        switch self {
        case .topLeft:
            return CGPoint(x: rect.minX, y: rect.minY)
        case .topRight:
            return CGPoint(x: rect.maxX, y: rect.minY)
        case .bottomLeft:
            return CGPoint(x: rect.minX, y: rect.maxY)
        case .bottomRight:
            return CGPoint(x: rect.maxX, y: rect.maxY)
        case .move:
            return CGPoint(x: rect.midX, y: rect.midY)
        }
    }
}

struct SelectionOverlay: View {
    let rect: CGRect
    let color: Color
    let isHighlightActive: Bool
    let onDrag: (SelectionHandle, DragGesture.Value) -> Void
    let onDragEnd: () -> Void

    var body: some View {
        ZStack(alignment: .topLeading) {
            Path(rect)
                .stroke(color, style: StrokeStyle(lineWidth: 2, dash: [6, 4]))
            
            Path(rect)
                .fill(color.opacity(isHighlightActive ? 0.08 : 0.1))

            Rectangle()
                .fill(Color.clear)
                .frame(width: rect.width, height: rect.height)
                .position(x: rect.midX, y: rect.midY)
                .contentShape(Rectangle())
                .highPriorityGesture(dragGesture(for: .move))

            ForEach(SelectionHandle.cornerCases, id: \.self) { handle in
                handleView(for: handle)
                    .position(handle.position(in: rect))
                    .highPriorityGesture(dragGesture(for: handle))
            }
        }
    }

    private func dragGesture(for handle: SelectionHandle) -> some Gesture {
        DragGesture()
            .onChanged { value in
                onDrag(handle, value)
            }
            .onEnded { _ in
                onDragEnd()
            }
    }

    private func handleView(for handle: SelectionHandle) -> some View {
        let size: CGFloat = 12
        return RoundedRectangle(cornerRadius: 3, style: .continuous)
            .fill(.thinMaterial)
            .frame(width: size, height: size)
            .overlay(
                RoundedRectangle(cornerRadius: 3, style: .continuous)
                    .stroke(color, lineWidth: 1.5)
            )
    }
}
