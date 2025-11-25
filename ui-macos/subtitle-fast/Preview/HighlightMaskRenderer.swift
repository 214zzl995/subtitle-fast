import AppKit

enum HighlightMaskRenderer {
    static func makeMask(
        frame: CGImage,
        selection: CGRect,
        threshold: Double,
        tolerance: Double,
        tint: NSColor
    ) -> CGImage? {
        let frameSize = CGSize(width: CGFloat(frame.width), height: CGFloat(frame.height))
        let bounded = selection
            .integral
            .intersection(CGRect(origin: .zero, size: frameSize))
        guard bounded.width >= 1, bounded.height >= 1 else { return nil }

        guard let cropped = frame.cropping(to: bounded) else { return nil }
        let width = cropped.width
        let height = cropped.height
        let bytesPerPixel = 4
        let bytesPerRow = bytesPerPixel * width
        let bufferSize = bytesPerRow * height

        guard let data = malloc(bufferSize) else { return nil }
        defer { free(data) }

        guard let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) else { return nil }
        guard let context = CGContext(
            data: data,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: bytesPerRow,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
        ) else {
            return nil
        }

        context.draw(cropped, in: CGRect(x: 0, y: 0, width: width, height: height))

        guard let sRGBTint = tint.usingColorSpace(.sRGB) else { return nil }
        let alpha: Double = 0.8
        let alphaByte = UInt8(clamping: Int(alpha * 255))
        let tintR = UInt8(clamping: Int(sRGBTint.redComponent * 255 * alpha))
        let tintG = UInt8(clamping: Int(sRGBTint.greenComponent * 255 * alpha))
        let tintB = UInt8(clamping: Int(sRGBTint.blueComponent * 255 * alpha))

        let minThreshold = max(0, threshold - tolerance)
        let maxThreshold = min(255, threshold + tolerance)
        let buffer = data.bindMemory(to: UInt8.self, capacity: bufferSize)

        for y in 0..<height {
            let row = buffer.advanced(by: y * bytesPerRow)
            for x in 0..<width {
                let idx = x * bytesPerPixel
                let r = Double(row[idx])
                let g = Double(row[idx + 1])
                let b = Double(row[idx + 2])
                let luma = 0.299 * r + 0.587 * g + 0.114 * b
                if luma >= minThreshold && luma <= maxThreshold {
                    row[idx] = tintR
                    row[idx + 1] = tintG
                    row[idx + 2] = tintB
                    row[idx + 3] = alphaByte
                } else {
                    row[idx] = 0
                    row[idx + 1] = 0
                    row[idx + 2] = 0
                    row[idx + 3] = 0
                }
            }
        }

        return context.makeImage()
    }
}
