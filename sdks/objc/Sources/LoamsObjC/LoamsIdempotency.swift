import Foundation

#if canImport(ObjectiveC)
@objcMembers
public class LoamsIdempotency: NSObject {
    public static func mintKey() -> String {
        return generateUUIDv7()
    }
}
#else
public class LoamsIdempotency: NSObject {
    public static func mintKey() -> String {
        return generateUUIDv7()
    }
}
#endif

private func generateUUIDv7() -> String {
    let now = Date().timeIntervalSince1970
    let timestamp = UInt64(now * 1000.0)
    let timeHex = String(format: "%012llx", timestamp)

    let part1 = String(timeHex.prefix(8))
    let part2 = String(timeHex.dropFirst(8).prefix(4))

    let r1 = UInt32.random(in: 0..<0x1000)
    let part3 = String(format: "7%03x", r1)

    let r2 = (UInt32.random(in: 0..<0x4000) | 0x8000)
    let part4 = String(format: "%04x", r2)

    let r3 = UInt32.random(in: 0..<0x1000000)
    let r4 = UInt32.random(in: 0..<0x1000000)
    let part5 = String(format: "%06x%06x", r3, r4)

    return "\(part1)-\(part2)-\(part3)-\(part4)-\(part5)"
}
