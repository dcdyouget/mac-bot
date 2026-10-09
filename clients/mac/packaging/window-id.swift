import CoreGraphics
import Foundation

let arguments = Array(CommandLine.arguments.dropFirst())
if arguments.first == "--permission" {
    let allowed = CGPreflightScreenCaptureAccess()
    print(allowed ? "authorized" : "denied")
    exit(allowed ? 0 : 2)
}

let target = arguments.first ?? "Mac Bot"
let targetPID = arguments.dropFirst().first.flatMap { Int($0) }
let normalizedTarget = target.lowercased().replacingOccurrences(of: " ", with: "")
let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
guard let rawWindows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else {
    exit(1)
}

var mainWindow: (number: UInt32, area: Double)?
for window in rawWindows {
    guard let layer = window[kCGWindowLayer as String] as? Int, layer == 0,
          let owner = window[kCGWindowOwnerName as String] as? String,
          (owner.localizedCaseInsensitiveContains(target)
            || (normalizedTarget == "macbot" && ["macbot", "macbot-desktop"].contains(owner.lowercased()))),
          let windowNumber = window[kCGWindowNumber as String] as? UInt32,
          let bounds = window[kCGWindowBounds as String] as? [String: Any],
          let width = (bounds["Width"] as? NSNumber)?.doubleValue,
          let height = (bounds["Height"] as? NSNumber)?.doubleValue,
          width >= 200, height >= 120 else {
        continue
    }
    if let targetPID,
       (window[kCGWindowOwnerPID as String] as? NSNumber)?.intValue != targetPID {
        continue
    }
    let area = width * height
    if mainWindow == nil || area > mainWindow!.area {
        mainWindow = (windowNumber, area)
    }
}
if let mainWindow {
    print(mainWindow.number)
    exit(0)
}
exit(1)
