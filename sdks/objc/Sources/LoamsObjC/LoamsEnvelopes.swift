import Foundation

public typealias LoamsEnvelopeFrame = LoamsFrame

#if canImport(ObjectiveC)
@objcMembers
public class LoamsFrame: NSObject {
    public let flags: UInt8
    public let payload: Data
    public var data: Data { return payload }

    public init(flags: UInt8, payload: Data) {
        self.flags = flags
        self.payload = payload
        super.init()
    }

    public var isMessage: Bool {
        return flags == 0
    }

    public var isTrailer: Bool {
        return (flags & 0x02) != 0 || (flags & 0x80) != 0
    }
}

@objcMembers
public class LoamsEnvelopes: NSObject {
    public static func packPayload(_ payload: Data, flags: UInt8 = 0) -> Data {
        var result = Data(capacity: 5 + payload.count)
        result.append(flags)
        let len = UInt32(payload.count)
        result.append(UInt8((len >> 24) & 0xFF))
        result.append(UInt8((len >> 16) & 0xFF))
        result.append(UInt8((len >> 8) & 0xFF))
        result.append(UInt8(len & 0xFF))
        result.append(payload)
        return result
    }

    public static func splitData(_ data: Data) -> [LoamsFrame] {
        var frames: [LoamsFrame] = []
        var pos = 0
        let total = data.count

        while pos + 5 <= total {
            let flags = data[pos]
            let b0 = UInt32(data[pos + 1])
            let b1 = UInt32(data[pos + 2])
            let b2 = UInt32(data[pos + 3])
            let b3 = UInt32(data[pos + 4])
            let payloadLength = Int((b0 << 24) | (b1 << 16) | (b2 << 8) | b3)
            pos += 5

            if pos + payloadLength > total {
                break
            }

            let payload = data.subdata(in: pos..<(pos + payloadLength))
            pos += payloadLength

            frames.append(LoamsFrame(flags: flags, payload: payload))
        }

        return frames
    }
}
#else
public class LoamsFrame: NSObject {
    public let flags: UInt8
    public let payload: Data
    public var data: Data { return payload }

    public init(flags: UInt8, payload: Data) {
        self.flags = flags
        self.payload = payload
        super.init()
    }

    public var isMessage: Bool {
        return flags == 0
    }

    public var isTrailer: Bool {
        return (flags & 0x02) != 0 || (flags & 0x80) != 0
    }
}

public class LoamsEnvelopes: NSObject {
    public static func packPayload(_ payload: Data, flags: UInt8 = 0) -> Data {
        var result = Data(capacity: 5 + payload.count)
        result.append(flags)
        let len = UInt32(payload.count)
        result.append(UInt8((len >> 24) & 0xFF))
        result.append(UInt8((len >> 16) & 0xFF))
        result.append(UInt8((len >> 8) & 0xFF))
        result.append(UInt8(len & 0xFF))
        result.append(payload)
        return result
    }

    public static func splitData(_ data: Data) -> [LoamsFrame] {
        var frames: [LoamsFrame] = []
        var pos = 0
        let total = data.count

        while pos + 5 <= total {
            let flags = data[pos]
            let b0 = UInt32(data[pos + 1])
            let b1 = UInt32(data[pos + 2])
            let b2 = UInt32(data[pos + 3])
            let b3 = UInt32(data[pos + 4])
            let payloadLength = Int((b0 << 24) | (b1 << 16) | (b2 << 8) | b3)
            pos += 5

            if pos + payloadLength > total {
                break
            }

            let payload = data.subdata(in: pos..<(pos + payloadLength))
            pos += payloadLength

            frames.append(LoamsFrame(flags: flags, payload: payload))
        }

        return frames
    }
}
#endif
