// Publishes the guest's live display on a private pipe.
// Renders display pixels into compact JPEG frames.
import AppKit
// Checks video buffers before converting each frame.
import CoreMedia
// Streams the guest display through Apple capture.
import ScreenCaptureKit

// Converts ScreenCaptureKit frames to the worker's length-delimited stream.
final class DesktopOutput: NSObject, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    private let context = CIContext()
    private let output = FileHandle.standardOutput
    // Frames arrive on one serial queue, so a plain flag marks an encode in progress.
    private var encoding = false

    /// Encodes one JPEG, stepping down in quality when a busy screen exceeds the frame limit.
    private func jpeg(_ bitmap: CGImage) -> Data? {
        let image = NSBitmapImageRep(cgImage: bitmap)
        for quality in [0.7, 0.45] {
            if let data = image.representation(using: .jpeg,
                properties: [.compressionFactor: quality]), (4...768 * 1024).contains(data.count) {
                return data
            }
        }
        return nil
    }

    /// Writes complete display updates to the authenticated guest worker.
    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer,
                of type: SCStreamOutputType) {
        guard type == .screen, sampleBuffer.isValid,
              let pixel = sampleBuffer.imageBuffer else { return }
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer,
                                                                        createIfNecessary: false)
            as? [[SCStreamFrameInfo: Any]],
           let status = attachments.first?[.status] as? Int,
           status != SCFrameStatus.complete.rawValue { return }
        // A frame that arrives during an encode is dropped; the next one is newer anyway.
        guard !encoding else { return }
        encoding = true
        defer { encoding = false }
        let image = CIImage(cvPixelBuffer: pixel)
        guard let bitmap = context.createCGImage(image, from: image.extent),
              let jpeg = jpeg(bitmap) else { return }
        var length = UInt32(jpeg.count).bigEndian
        let prefix = withUnsafeBytes(of: &length) { Data($0) }
        do {
            try output.write(contentsOf: prefix)
            try output.write(contentsOf: jpeg)
        } catch {
            exit(0)
        }
    }

    /// Ends the helper if macOS stops the display stream.
    func stream(_ stream: SCStream, didStopWithError error: any Error) {
        exit(1)
    }
}

// Starts one background display capture at up to thirty frames per second.
@main
struct DesktopStream {
    /// Captures the logged-in guest display until the owning worker closes the pipe.
    static func main() async {
        do {
            if CommandLine.arguments.count == 2 && CommandLine.arguments[1] == "--authorize" {
                exit(CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess() ? 0 : 1)
            }
            guard CommandLine.arguments.count == 1,
                  CGPreflightScreenCaptureAccess() else { exit(1) }
            let content = try await SCShareableContent.excludingDesktopWindows(
                false, onScreenWindowsOnly: false)
            guard let display = content.displays.first else { exit(1) }
            let filter = SCContentFilter(display: display, excludingWindows: [])
            let config = SCStreamConfiguration()
            let scale = min(1.0, 1920.0 / Double(max(display.width, display.height)))
            config.width = max(1, Int(Double(display.width) * scale))
            config.height = max(1, Int(Double(display.height) * scale))
            config.minimumFrameInterval = CMTime(value: 1, timescale: 30)
            config.queueDepth = 3
            config.capturesAudio = false
            config.showsCursor = true
            let output = DesktopOutput()
            let stream = SCStream(filter: filter, configuration: config, delegate: output)
            try stream.addStreamOutput(output, type: .screen,
                                       sampleHandlerQueue: DispatchQueue(label: "dev.miniclaw.desktop"))
            try await stream.startCapture()
            while true { try await Task.sleep(for: .seconds(60)) }
        } catch {
            exit(1)
        }
    }
}
