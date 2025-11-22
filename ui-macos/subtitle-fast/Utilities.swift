import Foundation
import CoreGraphics

func formattedTime(_ time: TimeInterval) -> String {
    guard time.isFinite else { return "--:--" }
    let totalSeconds = Int(time.rounded())
    let seconds = totalSeconds % 60
    let minutes = (totalSeconds / 60) % 60
    let hours = totalSeconds / 3600
    if hours > 0 {
        return String(format: "%02d:%02d:%02d", hours, minutes, seconds)
    }
    return String(format: "%02d:%02d", minutes, seconds)
}

func formattedTimestamp(_ time: TimeInterval) -> String {
    guard time.isFinite else { return "--:--" }
    let totalMillis = Int((time * 1000).rounded())
    let millis = totalMillis % 1000
    let seconds = (totalMillis / 1000) % 60
    let minutes = (totalMillis / 60_000) % 60
    let hours = totalMillis / 3_600_000
    if hours > 0 {
        return String(format: "%02d:%02d:%02d.%03d", hours, minutes, seconds, millis)
    }
    return String(format: "%02d:%02d.%03d", minutes, seconds, millis)
}

func formattedRange(start: TimeInterval, end: TimeInterval) -> String {
    "\(formattedTimestamp(start)) – \(formattedTimestamp(end))"
}

extension CGRect {
    func normalized(in size: CGSize) -> CGRect {
        guard size.width > 0 && size.height > 0 else { return .zero }
        return CGRect(
            x: origin.x / size.width,
            y: origin.y / size.height,
            width: width / size.width,
            height: height / size.height
        )
    }

    func denormalized(in size: CGSize) -> CGRect {
        CGRect(
            x: origin.x * size.width,
            y: origin.y * size.height,
            width: width * size.width,
            height: height * size.height
        )
    }
}

extension CGPoint {
    func offsetBy(dx: CGFloat, dy: CGFloat) -> CGPoint {
        CGPoint(x: x + dx, y: y + dy)
    }
}
