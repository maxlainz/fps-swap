import AVFoundation
import Foundation
let url = URL(fileURLWithPath: CommandLine.arguments[1])
let asset = AVURLAsset(url: url)
let sem = DispatchSemaphore(value: 0)
Task {
  do {
    let dur = try await asset.load(.duration)
    print("asset.duration=\(dur.seconds)")
    for t in try await asset.load(.tracks) {
      let (nfr, mfd, nts, tr, fds) = try await t.load(.nominalFrameRate, .minFrameDuration, .naturalTimeScale, .timeRange, .formatDescriptions)
      var extra = ""
      for fd in fds {
        if let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(fd)?.pointee { extra += " asbd.rate=\(asbd.mSampleRate)" }
        extra += " sub=\(String(describing: CMFormatDescriptionGetMediaSubType(fd)))"
      }
      print("track \(t.trackID) \(t.mediaType.rawValue) nominalFrameRate=\(nfr) minFrameDuration=\(mfd.value)/\(mfd.timescale) naturalTimeScale=\(nts) range=\(tr.start.seconds)+\(tr.duration.seconds)\(extra)")
      // decode the whole track
      let reader = try AVAssetReader(asset: asset)
      var settings: [String: Any]? = nil
      if t.mediaType == .audio { settings = [AVFormatIDKey: kAudioFormatLinearPCM, AVLinearPCMBitDepthKey: 16, AVLinearPCMIsFloatKey: false, AVLinearPCMIsBigEndianKey: false, AVLinearPCMIsNonInterleaved: false] }
      if t.mediaType == .video { settings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA] }
      if t.mediaType != .audio && t.mediaType != .video { continue }
      let out = AVAssetReaderTrackOutput(track: t, outputSettings: settings)
      reader.add(out); reader.startReading()
      var n = 0, frames = 0; var first = CMTime.invalid; var lastEnd = CMTime.zero; var rate = 0.0
      while let sb = out.copyNextSampleBuffer() {
        n += 1; frames += CMSampleBufferGetNumSamples(sb)
        let pts = CMSampleBufferGetPresentationTimeStamp(sb); if !first.isValid { first = pts }
        lastEnd = CMTimeAdd(pts, CMSampleBufferGetDuration(sb))
        if let fd = CMSampleBufferGetFormatDescription(sb), let a = CMAudioFormatDescriptionGetStreamBasicDescription(fd)?.pointee { rate = a.mSampleRate }
      }
      print("  decoded status=\(reader.status.rawValue) err=\(String(describing: reader.error?.localizedDescription)) buffers=\(n) samples=\(frames) firstPTS=\(first.seconds) end=\(lastEnd.seconds) outRate=\(rate)" + (rate > 0 ? " samples/outRate=\(Double(frames)/rate)" : ""))
    }
  } catch { print("ERROR \(error)") }
  sem.signal()
}
sem.wait()
