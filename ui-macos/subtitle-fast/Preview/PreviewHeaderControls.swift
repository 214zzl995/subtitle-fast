import SwiftUI
import AppKit

private let highlightPalette: [NSColor] = [
    .controlAccentColor,
    NSColor.systemRed,
    NSColor.systemOrange,
    NSColor.systemYellow,
    NSColor.systemGreen,
    NSColor.systemTeal,
    NSColor.systemBlue,
    NSColor.systemPurple
]

struct HeaderIconButton: View {
    let systemName: String
    var active: Bool = false
    var disabled: Bool = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: systemName)
                .font(.system(size: 13, weight: .semibold))
                .frame(width: 28, height: 28)
                .foregroundStyle(active ? Color.accentColor : Color.primary)
                .overlay(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .stroke(active ? Color.accentColor.opacity(0.7) : Color.primary.opacity(0.2), lineWidth: 1.2)
                )
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.35 : 1)
    }
}

struct ColorMenuButton: View {
    let current: NSColor
    let disabled: Bool
    let onSelect: (NSColor) -> Void
    @State private var isOpen = false

    var body: some View {
        let resolvedCurrent = normalizedColor(current)
        Button {
            guard !disabled else { return }
            isOpen = false
            DispatchQueue.main.async {
                isOpen = true
            }
        } label: {
            HStack(spacing: 6) {
                Circle()
                    .fill(swiftUIColor(from: resolvedCurrent))
                    .frame(width: 18, height: 18)
                    .overlay(
                        Circle()
                            .stroke(Color.primary.opacity(0.18), lineWidth: 1)
                    )
                Image(systemName: "chevron.down")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(Color.primary.opacity(0.7))
            }
            .padding(.horizontal, 8)
            .frame(height: 28)
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .stroke(Color.primary.opacity(0.12), lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.35 : 1)
        .onChange(of: disabled) { value in
            if value {
                isOpen = false
            }
        }
        .popover(isPresented: $isOpen, arrowEdge: .bottom) {
            VStack(spacing: 0) {
                ForEach(Array(highlightPalette.enumerated()), id: \.offset) { index, swatch in
                    let resolvedSwatch = normalizedColor(swatch)
                    Button {
                        onSelect(resolvedSwatch)
                        isOpen = false
                    } label: {
                        HStack(spacing: 8) {
                            Circle()
                                .fill(swiftUIColor(from: resolvedSwatch))
                                .frame(width: 16, height: 16)
                                .overlay(Circle().stroke(Color.primary.opacity(0.2), lineWidth: 1))
                            Text(colorName(for: swatch))
                            Spacer()
                            if isSameColor(resolvedCurrent, resolvedSwatch) {
                                Image(systemName: "checkmark")
                                    .font(.system(size: 10, weight: .semibold))
                            }
                        }
                        .padding(.vertical, 6)
                        .padding(.horizontal, 10)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    if index != highlightPalette.indices.last {
                        Divider()
                    }
                }
            }
            .padding(6)
            .frame(minWidth: 180)
        }
    }

    private func normalizedColor(_ color: NSColor) -> NSColor {
        if let sRGB = color.usingColorSpace(.sRGB) { return sRGB }
        if let device = color.usingColorSpace(.deviceRGB) { return device }
        if let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) {
            let cgColor = color.cgColor
            if let converted = cgColor.converted(to: colorSpace, intent: .defaultIntent, options: nil),
               let nsColor = NSColor(cgColor: converted) {
                return nsColor
            }
        }
        return color
    }

    private func swiftUIColor(from color: NSColor) -> Color {
        let normalized = normalizedColor(color)
        return Color(
            .sRGB,
            red: normalized.redComponent,
            green: normalized.greenComponent,
            blue: normalized.blueComponent,
            opacity: normalized.alphaComponent
        )
    }

    private func isSameColor(_ lhs: NSColor, _ rhs: NSColor) -> Bool {
        let epsilon: CGFloat = 0.001
        return abs(lhs.redComponent - rhs.redComponent) < epsilon
            && abs(lhs.greenComponent - rhs.greenComponent) < epsilon
            && abs(lhs.blueComponent - rhs.blueComponent) < epsilon
            && abs(lhs.alphaComponent - rhs.alphaComponent) < epsilon
    }

    private func colorName(for swatch: NSColor) -> String {
        switch swatch {
        case NSColor.systemRed: return "Red"
        case NSColor.systemOrange: return "Orange"
        case NSColor.systemYellow: return "Yellow"
        case NSColor.systemGreen: return "Green"
        case NSColor.systemTeal: return "Teal"
        case NSColor.systemBlue: return "Blue"
        case NSColor.systemPurple: return "Purple"
        default: return "Accent"
        }
    }
}

struct PreviewModeToggle: View {
    @Binding var selection: PreviewMode

    var body: some View {
        HStack(spacing: 1) {
            ForEach(PreviewMode.allCases) { mode in
                let isSelected = selection == mode
                Button {
                    selection = mode
                } label: {
                    Text(label(for: mode))
                        .font(.caption.weight(.semibold))
                        .frame(width: 74, height: 24)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(isSelected ? Color.primary : Color.secondary)
                .background(
                    RoundedRectangle(cornerRadius: 7, style: .continuous)
                        .fill(isSelected ? Color.accentColor.opacity(0.18) : Color.clear)
                )
            }
        }
        .frame(height: 28)
        .padding(.horizontal, 4)
        .background(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .fill(Color.primary.opacity(0.05))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .stroke(Color.primary.opacity(0.12), lineWidth: 1)
        )
    }

    private func label(for mode: PreviewMode) -> LocalizedStringKey {
        switch mode {
        case .color:
            return "ui.preview_mode_color"
        case .luma:
            return "ui.preview_mode_luma"
        }
    }
}
