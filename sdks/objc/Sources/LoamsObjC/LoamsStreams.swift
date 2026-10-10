import Foundation

#if canImport(ObjectiveC)
@objcMembers
public class LoamsStreamHandle: NSObject {
    private let frames: [LoamsFrame]
    public private(set) var lastCursor: String?
    public private(set) var frameKinds: [String] = []
    private var isClosed: Bool = false

    public init(frames: [LoamsFrame], cursor: String? = nil) {
        self.frames = frames
        self.lastCursor = cursor
        super.init()
    }

    public func getMessages() throws -> [[String: Any]] {
        var result: [[String: Any]] = []

        for frame in frames {
            if isClosed { break }

            if frame.isTrailer {
                frameKinds.append("trailer")
                if let json = try? JSONSerialization.jsonObject(with: frame.payload, options: []) as? [String: Any],
                   json["error"] != nil {
                    throw LoamsErrorParser.parse(withStatus: 400, body: frame.payload)
                }
                continue
            }

            frameKinds.append("message")
            if let json = try? JSONSerialization.jsonObject(with: frame.payload, options: []) as? [String: Any] {
                if let c = json["cursor"] {
                    lastCursor = "\(c)"
                }
                if let hb = json["heartbeat"] as? Bool, hb {
                    frameKinds.append("heartbeat")
                    continue // filter heartbeat frame
                }
                result.append(json)
            }
        }

        return result
    }

    public func close() {
        isClosed = true
    }
}
#else
public class LoamsStreamHandle: NSObject {
    private let frames: [LoamsFrame]
    public private(set) var lastCursor: String?
    public private(set) var frameKinds: [String] = []
    private var isClosed: Bool = false

    public init(frames: [LoamsFrame], cursor: String? = nil) {
        self.frames = frames
        self.lastCursor = cursor
        super.init()
    }

    public func getMessages() throws -> [[String: Any]] {
        var result: [[String: Any]] = []

        for frame in frames {
            if isClosed { break }

            if frame.isTrailer {
                frameKinds.append("trailer")
                if let json = try? JSONSerialization.jsonObject(with: frame.payload, options: []) as? [String: Any],
                   json["error"] != nil {
                    throw LoamsErrorParser.parse(withStatus: 400, body: frame.payload)
                }
                continue
            }

            frameKinds.append("message")
            if let json = try? JSONSerialization.jsonObject(with: frame.payload, options: []) as? [String: Any] {
                if let c = json["cursor"] {
                    lastCursor = "\(c)"
                }
                if let hb = json["heartbeat"] as? Bool, hb {
                    frameKinds.append("heartbeat")
                    continue // filter heartbeat frame
                }
                result.append(json)
            }
        }

        return result
    }

    public func close() {
        isClosed = true
    }
}
#endif
