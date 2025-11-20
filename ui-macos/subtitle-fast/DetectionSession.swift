import AppKit
import AVFoundation
import Combine
import Foundation
import SwiftUI
import UniformTypeIdentifiers

struct SubtitleRow: Identifiable {
    let id = UUID()
    let timecode: TimeInterval
    let text: String
    let confidence: Double?
}

struct ProgressMetrics {
    var fps: Double?
    var det: Double?
    var seg: Double?
    var pf: Double?
    var ocr: Double?
    var wr: Double?
    var ocrEmpty: Int = 0
    var cues: Int = 0
}

enum AppLanguage: String, CaseIterable, Identifiable {
    case english
    case chinese

    var id: String { rawValue }

    var locale: Locale {
        switch self {
        case .english:
            return Locale(identifier: "en")
        case .chinese:
            return Locale(identifier: "zh-Hans")
        }
    }

    var label: String {
        switch self {
        case .english:
            return "English"
        case .chinese:
            return "中文"
        }
    }

    static func systemDefault() -> AppLanguage {
        let code = Locale.current.language.languageCode?.identifier ?? ""
        if code.starts(with: "zh") {
            return .chinese
        }
        return .english
    }
}

@MainActor
final class DetectionSession: ObservableObject {
    @Published var selectedFile: URL?
    @Published var duration: TimeInterval = 0
    @Published var currentTime: TimeInterval = 0
    @Published var videoSize: CGSize?
    @Published var selection: CGRect?
    @Published var subtitleColor: Color = .white
    @Published var threshold: Double = 230
    @Published var tolerance: Double = 12
    @Published var isHighlightActive: Bool = false
    @Published var isDetecting: Bool = false
    @Published var isPlaying: Bool = false
    @Published var progress: Double = 0
    @Published var metrics = ProgressMetrics()
    @Published var subtitles: [SubtitleRow] = []
    @Published var errorMessage: String?
    @Published var locale: Locale = AppLanguage.systemDefault().locale
    @Published var isSelectingRegion: Bool = false
    @Published var isSelectingColor: Bool = false

    var player: AVPlayer?
    private var timeObserver: Any?

    var fileDisplayName: String {
        selectedFile?.lastPathComponent ?? NSLocalizedString("ui.no_file", comment: "")
    }

    func pickFile() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.allowedContentTypes = [
            .mpeg4Movie,
            .quickTimeMovie,
            .movie
        ]
        panel.prompt = NSLocalizedString("ui.open_file", comment: "")

        guard panel.runModal() == .OK, let url = panel.url else { return }

        selectedFile = url
        let asset = AVURLAsset(url: url)
        duration = 0
        currentTime = 0
        videoSize = nil

        Task { @MainActor in
            do {
                let dur = try await asset.load(.duration)
                let seconds = CMTimeGetSeconds(dur)
                duration = seconds.isFinite ? seconds : 0
                
                if let track = try await asset.loadTracks(withMediaType: .video).first {
                    let size = try await track.load(.naturalSize)
                    let transform = try await track.load(.preferredTransform)
                    // Handle rotation (e.g. portrait video)
                    let transformedSize = size.applying(transform)
                    videoSize = CGSize(width: abs(transformedSize.width), height: abs(transformedSize.height))
                }
            } catch {
                duration = 0
                videoSize = nil
            }
        }

        if let oldObserver = timeObserver {
            player?.removeTimeObserver(oldObserver)
            timeObserver = nil
        }

        let item = AVPlayerItem(asset: asset)
        let player = AVPlayer(playerItem: item)
        self.player = player
        
        timeObserver = player.addPeriodicTimeObserver(forInterval: CMTime(seconds: 0.5, preferredTimescale: 600), queue: .main) { [weak self] time in
            guard let self = self else { return }
            // Only update if not dragging slider (we can add a flag for that later if needed, 
            // but for now simple update is better than nothing)
            self.currentTime = time.seconds
        }

        // Reset state for new selection
        selection = nil
        subtitles = []
        progress = 0
        metrics = ProgressMetrics()
        errorMessage = nil
    }

    func resetSelection() {
        selection = nil
    }

    func updateSelection(normalized: CGRect?) {
        selection = normalized
    }

    func seek(to value: Double) {
        currentTime = min(max(0, value), duration)
        let target = CMTime(seconds: currentTime, preferredTimescale: 600)
        player?.seek(to: target)
    }

    func togglePlayPause() {
        guard let player = player else { return }
        if player.timeControlStatus == .playing {
            player.pause()
            isPlaying = false
        } else {
            player.play()
            isPlaying = true
        }
    }

    func toggleHighlight() {
        isHighlightActive.toggle()
    }

    func startDetectionDemo() {
        guard selectedFile != nil else {
            errorMessage = NSLocalizedString("ui.error_no_file", comment: "")
            return
        }
        guard selection != nil else {
            errorMessage = NSLocalizedString("ui.error_no_selection", comment: "")
            return
        }

        errorMessage = nil
        isDetecting = true
        progress = 0
        subtitles = []

        Task {
            let steps = 6
            for step in 1...steps {
                try await Task.sleep(nanoseconds: 500_000_000) // 0.5s
                progress = Double(step) / Double(steps)
                metrics.fps = 24 + Double(step)
                metrics.det = 10 + Double(step)
                metrics.seg = 5 + Double(step)
                metrics.pf = 3 + Double(step)
                metrics.ocr = 12 + Double(step)
                metrics.wr = 2 + Double(step)
                metrics.ocrEmpty = step / 2
                metrics.cues = subtitles.count

                subtitles.append(
                    SubtitleRow(
                        timecode: Double(step) * 1.3,
                        text: String(format: "%@ %d", NSLocalizedString("ui.sample_subtitle", comment: ""), step),
                        confidence: Double.random(in: 0.8...0.99)
                    )
                )
            }
            isDetecting = false
        }
    }

    func focusLanguage(_ language: AppLanguage) {
        locale = language.locale
    }
}

extension CGRect {
    func normalized(in size: CGSize) -> CGRect {
        guard size.width > 0, size.height > 0 else { return .zero }
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
