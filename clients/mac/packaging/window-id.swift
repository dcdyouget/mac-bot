import CoreGraphics
import Foundation

let target = CommandLine.arguments.dropFirst().first ?? "Mac Bot"
let normalizedTarget = target.lowercased().replacingOccurrences(of: " ", with: "")
let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
guard let rawWindows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else {
    exit(1)
}

for window in rawWindows {
    guard let layer = window[kCGWindowLayer as String] as? Int, layer == 0,
          let owner = window[kCGWindowOwnerName as String] as? String,
          (owner.localizedCaseInsensitiveContains(target)
            || (normalizedTarget == "macbot" && ["macbot", "macbot-desktop"].contains(owner.lowercased()))),
          let windowNumber = window[kCGWindowNumber as String] as? UInt32 else {
        continue
    }
    print(windowNumber)
    exit(0)
}
exit(1)
