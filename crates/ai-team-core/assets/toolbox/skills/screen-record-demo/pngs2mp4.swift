import AVFoundation
import AppKit
import Foundation

// Encodes a numbered PNG sequence into an H.264 mp4 via AVAssetWriter.
// usage: pngs2mp4 <frameDir> <out.mp4> <fps>

let args = CommandLine.arguments
guard args.count == 4, let fps = Int32(args[3]) else {
    FileHandle.standardError.write("usage: pngs2mp4 <frameDir> <out.mp4> <fps>\n".data(using: .utf8)!)
    exit(2)
}

let frameDir = URL(fileURLWithPath: args[1])
let outURL = URL(fileURLWithPath: args[2])

let files = try FileManager.default
    .contentsOfDirectory(at: frameDir, includingPropertiesForKeys: nil)
    .filter { $0.pathExtension.lowercased() == "png" }
    .sorted { $0.lastPathComponent < $1.lastPathComponent }

guard let first = files.first, let firstImage = NSImage(contentsOf: first) else {
    FileHandle.standardError.write("no frames found\n".data(using: .utf8)!)
    exit(1)
}

var rect = CGRect(origin: .zero, size: firstImage.size)
guard let firstCG = firstImage.cgImage(forProposedRect: &rect, context: nil, hints: nil) else {
    FileHandle.standardError.write("cannot decode first frame\n".data(using: .utf8)!)
    exit(1)
}

let width = firstCG.width
let height = firstCG.height

try? FileManager.default.removeItem(at: outURL)

let writer = try AVAssetWriter(outputURL: outURL, fileType: .mp4)

let settings: [String: Any] = [
    AVVideoCodecKey: AVVideoCodecType.h264,
    AVVideoWidthKey: width,
    AVVideoHeightKey: height,
    AVVideoCompressionPropertiesKey: [
        AVVideoAverageBitRateKey: 6_000_000,
        AVVideoProfileLevelKey: AVVideoProfileLevelH264HighAutoLevel,
        AVVideoMaxKeyFrameIntervalKey: fps * 2,
    ],
]

let input = AVAssetWriterInput(mediaType: .video, outputSettings: settings)
input.expectsMediaDataInRealTime = false

let adaptor = AVAssetWriterInputPixelBufferAdaptor(
    assetWriterInput: input,
    sourcePixelBufferAttributes: [
        kCVPixelBufferPixelFormatTypeKey as String: Int(kCVPixelFormatType_32ARGB),
        kCVPixelBufferWidthKey as String: width,
        kCVPixelBufferHeightKey as String: height,
    ]
)

writer.add(input)
writer.startWriting()
writer.startSession(atSourceTime: .zero)

func pixelBuffer(from image: CGImage) -> CVPixelBuffer? {
    var buffer: CVPixelBuffer?
    let attrs: [String: Any] = [kCVPixelBufferCGImageCompatibilityKey as String: true]

    guard CVPixelBufferCreate(kCFAllocatorDefault, width, height, kCVPixelFormatType_32ARGB, attrs as CFDictionary, &buffer) == kCVReturnSuccess,
          let pb = buffer else { return nil }

    CVPixelBufferLockBaseAddress(pb, [])
    defer { CVPixelBufferUnlockBaseAddress(pb, []) }

    guard let ctx = CGContext(
        data: CVPixelBufferGetBaseAddress(pb),
        width: width,
        height: height,
        bitsPerComponent: 8,
        bytesPerRow: CVPixelBufferGetBytesPerRow(pb),
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.noneSkipFirst.rawValue
    ) else { return nil }

    ctx.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))

    return pb
}

let queue = DispatchQueue(label: "encode")
let done = DispatchSemaphore(value: 0)
var index = 0
var failed: String?

input.requestMediaDataWhenReady(on: queue) {
    while input.isReadyForMoreMediaData {
        if index >= files.count {
            input.markAsFinished()
            done.signal()
            return
        }

        let url = files[index]
        var r = CGRect(origin: .zero, size: firstImage.size)

        guard let img = NSImage(contentsOf: url)?.cgImage(forProposedRect: &r, context: nil, hints: nil),
              let pb = pixelBuffer(from: img) else {
            failed = "frame decode failed: \(url.lastPathComponent)"
            input.markAsFinished()
            done.signal()
            return
        }

        adaptor.append(pb, withPresentationTime: CMTime(value: CMTimeValue(index), timescale: fps))
        index += 1
    }
}

done.wait()
writer.finishWriting { }

while writer.status == .writing {
    usleep(50_000)
}

if let failed {
    FileHandle.standardError.write("\(failed)\n".data(using: .utf8)!)
    exit(1)
}

if writer.status != .completed {
    FileHandle.standardError.write("writer failed: \(writer.error?.localizedDescription ?? "unknown")\n".data(using: .utf8)!)
    exit(1)
}

print("wrote \(index) frames at \(width)x\(height) -> \(outURL.path)")
