import SwiftUI
import Combine
import AVFoundation
import AppKit

struct SubtitleItem: Identifiable {
    let id = UUID()
    let index: Int
    let timecode: TimeInterval
    let endTime: TimeInterval
    let text: String
    let confidence: Double?
}

struct DetectionMetrics {
    var fps: Double
    var det: Double
    var ocr: Double
    var cues: Int
    var ocrEmpty: Int

    static let empty = DetectionMetrics(fps: 0, det: 0, ocr: 0, cues: 0, ocrEmpty: 0)
}

@MainActor
final class DetectionSession: ObservableObject {
    // MARK: Selection Properties
    @Published var selection: CGRect?
    @Published var threshold: Double = 230
    @Published var tolerance: Double = 20
    @Published var isHighlightActive = false
    @Published var isSelectingRegion = false

    // MARK: Playback
    @Published var selectedFile: URL?
    @Published var player: AVPlayer?
    @Published var videoSize: CGSize?
    @Published var isPlaying = false
    @Published var currentTime: TimeInterval = 0
    @Published var duration: TimeInterval = 0

    // MARK: Detection
    @Published var isDetecting = false
    @Published var progress: Double = 0
    @Published var errorMessage: String?
    @Published var metrics = DetectionMetrics.empty
    @Published var subtitles: [SubtitleItem] = []

    private var detectionHandle: UInt64?
    private var timeObserver: Any?
    private var outputURL: URL?
    private let samplesPerSecond: UInt32 = 7
    private let ffi = SubtitleFastFFI.shared

    init() {
        ffi.registerCallbacks(
            onProgress: { [weak self] update in
                DispatchQueue.main.async {
                    self?.handleProgress(update)
                }
            },
            onError: { [weak self] message in
                DispatchQueue.main.async {
                    self?.errorMessage = message
                    self?.isDetecting = false
                    self?.detectionHandle = nil
                }
            }
        )
    }

    // MARK: - File + Playback

    func pickFile() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.allowedFileTypes = ["mp4", "mkv", "mov", "avi"]
        panel.begin { [weak self] response in
            guard response == .OK, let url = panel.url else { return }
            self?.loadVideo(from: url)
        }
    }

    private func loadVideo(from url: URL) {
        selectedFile = url
        player = AVPlayer(url: url)
        setupTimeObserver()
        updateVideoMetadata(for: url)
        isPlaying = false
        currentTime = 0
        duration = 0
        progress = 0
        errorMessage = nil
        subtitles = []
        outputURL = nil
    }

    private func setupTimeObserver() {
        if let observer = timeObserver, let player {
            player.removeTimeObserver(observer)
        }
        guard let player else { return }
        let interval = CMTime(seconds: 0.2, preferredTimescale: 600)
        timeObserver = player.addPeriodicTimeObserver(forInterval: interval, queue: .main) { [weak self] time in
            guard let self else { return }
            Task { @MainActor in
                self.currentTime = time.seconds
                self.isPlaying = self.player?.timeControlStatus == .playing
                if self.duration == 0, let item = self.player?.currentItem {
                    self.duration = item.asset.duration.seconds
                }
            }
        }
    }

    private func updateVideoMetadata(for url: URL) {
        let asset = AVAsset(url: url)
        if let track = asset.tracks(withMediaType: .video).first {
            let transformed = track.naturalSize.applying(track.preferredTransform)
            videoSize = CGSize(width: abs(transformed.width), height: abs(transformed.height))
        } else {
            videoSize = nil
        }
        let length = asset.duration.seconds
        if length.isFinite {
            duration = length
        }
    }

    func togglePlayPause() {
        guard let player else { return }
        if player.timeControlStatus == .playing {
            player.pause()
            isPlaying = false
        } else {
            player.play()
            isPlaying = true
        }
    }

    func seek(to time: TimeInterval) {
        guard let player else { return }
        let cmTime = CMTime(seconds: time, preferredTimescale: 600)
        player.seek(to: cmTime, toleranceBefore: .zero, toleranceAfter: .zero)
        currentTime = time
    }

    // MARK: - Selection

    func resetSelection() {
        selection = nil
    }

    func startRegionSelection() {
        isSelectingRegion.toggle()
    }

    func finishRegionSelection() {
        isSelectingRegion = false
    }

    func updateSelection(normalized rect: CGRect) {
        selection = rect
    }

    // MARK: - Detection

    func startDetection() {
        guard !isDetecting else { return }
        guard let input = selectedFile else {
            errorMessage = NSLocalizedString("ui.error_no_file", comment: "no file")
            return
        }
        guard selection != nil else {
            errorMessage = NSLocalizedString("ui.error_no_selection", comment: "no selection")
            return
        }

        errorMessage = nil
        subtitles = []
        metrics = .empty
        progress = 0

        let output = makeOutputURL(for: input)
        outputURL = output
        try? FileManager.default.removeItem(at: output)

        let target = UInt8(clamping: Int(threshold))
        let delta = UInt8(clamping: Int(tolerance))

        let result = ffi.startRun(
            input: input,
            output: output,
            decoderBackend: nil,
            samplesPerSecond: samplesPerSecond,
            detectorTarget: target,
            detectorDelta: delta
        )

        switch result {
        case .success(let handle):
            detectionHandle = handle
            isDetecting = true
            player?.pause()
            isPlaying = false
        case .failure(let error):
            errorMessage = error.localizedDescription
            detectionHandle = nil
        }
    }

    func cancelDetection() {
        guard let handle = detectionHandle else { return }
        _ = ffi.cancel(handle: handle)
        detectionHandle = nil
        isDetecting = false
    }

    private func handleProgress(_ update: GuiProgressUpdate) {
        metrics = DetectionMetrics(
            fps: update.fps,
            det: update.det_ms,
            ocr: update.ocr_ms,
            cues: Int(update.cues),
            ocrEmpty: Int(update.ocr_empty)
        )
        progress = max(0, min(1, update.progress))

        if update.completed {
            isDetecting = false
            detectionHandle = nil
            loadSubtitlesFromOutput()
        }
    }

    private func loadSubtitlesFromOutput() {
        guard let outputURL, FileManager.default.fileExists(atPath: outputURL.path) else {
            return
        }
        guard let contents = try? String(contentsOf: outputURL, encoding: .utf8) else {
            return
        }
        subtitles = SubtitleParser.parse(srt: contents)
    }

    func exportSubtitles() {
        guard let outputURL, FileManager.default.fileExists(atPath: outputURL.path) else {
            return
        }
        let panel = NSSavePanel()
        panel.allowedFileTypes = ["srt"]
        panel.canCreateDirectories = true
        panel.nameFieldStringValue = outputURL.lastPathComponent
        panel.begin { response in
            guard response == .OK, let destination = panel.url else { return }
            do {
                if FileManager.default.fileExists(atPath: destination.path) {
                    try FileManager.default.removeItem(at: destination)
                }
                try FileManager.default.copyItem(at: outputURL, to: destination)
            } catch {
                self.errorMessage = error.localizedDescription
            }
        }
    }

    private func makeOutputURL(for input: URL) -> URL {
        let name = input.deletingPathExtension().lastPathComponent
        return FileManager.default.temporaryDirectory.appendingPathComponent("\(name)-subtitle-fast.srt")
    }

    func load(from url: URL) {
        loadVideo(from: url)
    }
}

enum SubtitleParser {
    static func parse(srt: String) -> [SubtitleItem] {
        let normalized = srt.replacingOccurrences(of: "\r\n", with: "\n")
        let blocks = normalized.components(separatedBy: "\n\n")
        var items: [SubtitleItem] = []

        for (blockIndex, block) in blocks.enumerated() {
            let lines = block.split(whereSeparator: \.isNewline)
            guard !lines.isEmpty else { continue }

            let (timeLine, textStart) = timeLineAndStartIndex(lines: lines)
            guard let timeLine else { continue }

            let parts = timeLine.split(separator: " ")
            guard parts.count >= 3 else { continue }

            guard
                let start = clockTime(String(parts[0])),
                let end = clockTime(String(parts[2]))
            else { continue }

            let text = lines[textStart...].joined(separator: "\n")
            let number = Int(lines.first ?? "") ?? (blockIndex + 1)

            items.append(
                SubtitleItem(
                    index: number,
                    timecode: start,
                    endTime: end,
                    text: text.trimmingCharacters(in: .whitespacesAndNewlines),
                    confidence: nil
                )
            )
        }

        return items
    }

    private static func timeLineAndStartIndex(lines: [Substring]) -> (Substring?, Int) {
        if lines.count >= 2, lines[1].contains("-->") {
            return (lines[1], 2)
        }
        if lines.first?.contains("-->") == true {
            return (lines.first, 1)
        }
        return (nil, 0)
    }

    private static func clockTime(_ raw: String) -> TimeInterval? {
        let parts = raw.split(separator: ":")
        guard parts.count == 3 else { return nil }
        let secParts = parts[2].split(separator: ",")
        guard secParts.count == 2 else { return nil }
        guard
            let hours = Int(parts[0]),
            let minutes = Int(parts[1]),
            let seconds = Int(secParts[0]),
            let millis = Int(secParts[1])
        else { return nil }

        let base = hours * 3600 + minutes * 60 + seconds
        return Double(base) + Double(millis) / 1000.0
    }
}
