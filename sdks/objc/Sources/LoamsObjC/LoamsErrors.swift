import Foundation

public let LoamsErrorDomain = "dev.loams.error"
public let LoamsErrorReasonKey = "LoamsErrorReasonKey"
public let LoamsErrorCodeKey = "LoamsErrorCodeKey"
public let LoamsErrorHTTPStatusKey = "LoamsErrorHTTPStatusKey"

#if canImport(ObjectiveC)
@objcMembers
open class LoamsError: NSError, @unchecked Sendable {
    public let codeString: String
    public let reason: String?
    public let unknownReason: String?
    public let httpStatus: Int
    public let rawDetails: [Any]

    public init(message: String,
                codeString: String,
                reason: String?,
                unknownReason: String?,
                httpStatus: Int,
                rawDetails: [Any] = []) {
        self.codeString = codeString
        self.reason = reason
        self.unknownReason = unknownReason
        self.httpStatus = httpStatus
        self.rawDetails = rawDetails

        var info: [String: Any] = [
            NSLocalizedDescriptionKey: message,
            LoamsErrorCodeKey: codeString,
            LoamsErrorHTTPStatusKey: httpStatus
        ]
        if let r = reason {
            info[LoamsErrorReasonKey] = r
        }

        super.init(domain: LoamsErrorDomain, code: httpStatus, userInfo: info)
    }

    public required init?(coder: NSCoder) {
        self.codeString = "unknown"
        self.reason = nil
        self.unknownReason = nil
        self.httpStatus = 0
        self.rawDetails = []
        super.init(coder: coder)
    }
}

@objcMembers
public class LoamsErrorParser: NSObject {
    public static func decodeBase64Safe(_ b64: String) -> Data {
        return decodeBase64SafeInternal(b64)
    }

    public static func decodeErrorInfo(_ data: Data) -> String? {
        return decodeErrorInfoInternal(data)
    }

    public static func extractFromStatusDetailsBin(_ bin: Data) -> String? {
        return extractFromStatusDetailsBinInternal(bin)
    }

    public static func parse(withStatus status: Int, body: Data) -> LoamsError {
        return parseInternal(withStatus: status, body: body)
    }
}
#else
open class LoamsError: NSError, @unchecked Sendable {
    public let codeString: String
    public let reason: String?
    public let unknownReason: String?
    public let httpStatus: Int
    public let rawDetails: [Any]

    public init(message: String,
                codeString: String,
                reason: String?,
                unknownReason: String?,
                httpStatus: Int,
                rawDetails: [Any] = []) {
        self.codeString = codeString
        self.reason = reason
        self.unknownReason = unknownReason
        self.httpStatus = httpStatus
        self.rawDetails = rawDetails

        var info: [String: Any] = [
            NSLocalizedDescriptionKey: message,
            LoamsErrorCodeKey: codeString,
            LoamsErrorHTTPStatusKey: httpStatus
        ]
        if let r = reason {
            info[LoamsErrorReasonKey] = r
        }

        super.init(domain: LoamsErrorDomain, code: httpStatus, userInfo: info)
    }

    public required init?(coder: NSCoder) {
        self.codeString = "unknown"
        self.reason = nil
        self.unknownReason = nil
        self.httpStatus = 0
        self.rawDetails = []
        super.init(coder: coder)
    }
}

public class LoamsErrorParser: NSObject {
    public static func decodeBase64Safe(_ b64: String) -> Data {
        return decodeBase64SafeInternal(b64)
    }

    public static func decodeErrorInfo(_ data: Data) -> String? {
        return decodeErrorInfoInternal(data)
    }

    public static func extractFromStatusDetailsBin(_ bin: Data) -> String? {
        return extractFromStatusDetailsBinInternal(bin)
    }

    public static func parse(withStatus status: Int, body: Data) -> LoamsError {
        return parseInternal(withStatus: status, body: body)
    }
}
#endif

private func decodeBase64SafeInternal(_ b64: String) -> Data {
    var clean = b64.trimmingCharacters(in: .whitespacesAndNewlines)
    clean = clean.replacingOccurrences(of: "-", with: "+")
    clean = clean.replacingOccurrences(of: "_", with: "/")
    let pad = clean.count % 4
    if pad > 0 {
        clean += String(repeating: "=", count: 4 - pad)
    }
    return Data(base64Encoded: clean, options: .ignoreUnknownCharacters) ?? Data()
}

private func decodeErrorInfoInternal(_ data: Data) -> String? {
    let bytes = [UInt8](data)
    let len = bytes.count
    var pos = 0

    while pos < len {
        var tag: UInt64 = 0
        var shift = 0
        while pos < len {
            let b = bytes[pos]
            pos += 1
            tag |= UInt64(b & 0x7F) << shift
            if (b & 0x80) == 0 { break }
            shift += 7
        }

        let field = tag >> 3
        let wireType = tag & 7

        if wireType == 2 {
            var payloadLen: UInt64 = 0
            shift = 0
            while pos < len {
                let b = bytes[pos]
                pos += 1
                payloadLen |= UInt64(b & 0x7F) << shift
                if (b & 0x80) == 0 { break }
                shift += 7
            }
            if pos + Int(payloadLen) > len { break }
            if field == 1 {
                let sub = data.subdata(in: pos..<(pos + Int(payloadLen)))
                return String(data: sub, encoding: .utf8)
            }
            pos += Int(payloadLen)
        } else if wireType == 0 {
            while pos < len && (bytes[pos] & 0x80) != 0 {
                pos += 1
            }
            if pos < len { pos += 1 }
        } else if wireType == 1 {
            pos += 8
        } else if wireType == 5 {
            pos += 4
        } else {
            break
        }
    }
    return nil
}

private func extractFromStatusDetailsBinInternal(_ bin: Data) -> String? {
    let bytes = [UInt8](bin)
    let len = bytes.count
    var pos = 0

    while pos < len {
        var tag: UInt64 = 0
        var shift = 0
        while pos < len {
            let b = bytes[pos]
            pos += 1
            tag |= UInt64(b & 0x7F) << shift
            if (b & 0x80) == 0 { break }
            shift += 7
        }

        let field = tag >> 3
        let wireType = tag & 7

        if wireType == 2 {
            var payloadLen: UInt64 = 0
            shift = 0
            while pos < len {
                let b = bytes[pos]
                pos += 1
                payloadLen |= UInt64(b & 0x7F) << shift
                if (b & 0x80) == 0 { break }
                shift += 7
            }
            if pos + Int(payloadLen) > len { break }

            if field == 3 { // details: repeated Any
                let anyData = bin.subdata(in: pos..<(pos + Int(payloadLen)))
                let anyBytes = [UInt8](anyData)
                let anyLen = anyBytes.count
                var anyPos = 0

                while anyPos < anyLen {
                    var aTag: UInt64 = 0
                    shift = 0
                    while anyPos < anyLen {
                        let ab = anyBytes[anyPos]
                        anyPos += 1
                        aTag |= UInt64(ab & 0x7F) << shift
                        if (ab & 0x80) == 0 { break }
                        shift += 7
                    }
                    let aField = aTag >> 3
                    let aWire = aTag & 7

                    if aWire == 2 {
                        var aPayloadLen: UInt64 = 0
                        shift = 0
                        while anyPos < anyLen {
                            let ab = anyBytes[anyPos]
                            anyPos += 1
                            aPayloadLen |= UInt64(ab & 0x7F) << shift
                            if (ab & 0x80) == 0 { break }
                            shift += 7
                        }
                        if anyPos + Int(aPayloadLen) > anyLen { break }
                        if aField == 2 {
                            let errInfoData = anyData.subdata(in: anyPos..<(anyPos + Int(aPayloadLen)))
                            if let reason = decodeErrorInfoInternal(errInfoData) {
                                return reason
                            }
                        }
                        anyPos += Int(aPayloadLen)
                    } else if aWire == 0 {
                        while anyPos < anyLen && (anyBytes[anyPos] & 0x80) != 0 {
                            anyPos += 1
                        }
                        if anyPos < anyLen { anyPos += 1 }
                    } else {
                        break
                    }
                }
            }
            pos += Int(payloadLen)
        } else if wireType == 0 {
            while pos < len && (bytes[pos] & 0x80) != 0 {
                pos += 1
            }
            if pos < len { pos += 1 }
        } else {
            break
        }
    }
    return nil
}

private func parseInternal(withStatus status: Int, body bodyData: Data) -> LoamsError {
    var code = "unknown"
    var message = "HTTP \(status)"
    var reason: String? = nil
    let unknownReason: String? = nil
    var rawDetails: [Any] = []

    var activeData = bodyData
    if activeData.count >= 5 {
        let frames = LoamsEnvelopes.splitData(activeData)
        for f in frames {
            if f.isTrailer {
                activeData = f.payload
                break
            }
        }
    }

    if let str = String(data: activeData, encoding: .utf8) {
        if str.contains("grpc-status-details-bin:") {
            if let regex = try? NSRegularExpression(pattern: "grpc-status-details-bin:\\s*([^\\r\\n]+)") {
                let range = NSRange(str.startIndex..<str.endIndex, in: str)
                if let match = regex.firstMatch(in: str, options: [], range: range) {
                    if let b64Range = Range(match.range(at: 1), in: str) {
                        let b64 = String(str[b64Range]).trimmingCharacters(in: .whitespaces)
                        let bin = decodeBase64SafeInternal(b64)
                        if let extracted = extractFromStatusDetailsBinInternal(bin) {
                            reason = extracted
                        }
                    }
                }
            }
        }
    }

    if let json = try? JSONSerialization.jsonObject(with: activeData, options: []) as? [String: Any] {
        var dict = json
        if let err = dict["error"] as? [String: Any] {
            dict = err
        }
        if let c = dict["code"] {
            code = "\(c)"
        }
        if let m = dict["message"] {
            message = "\(m)"
        }
        if let details = dict["details"] as? [Any] {
            rawDetails = details
            for detail in details {
                if let d = detail as? [String: Any] {
                    let type = "\(d["@type"] ?? d["type"] ?? "")"
                    if type.hasSuffix("ErrorInfo") {
                        if let r = d["reason"] {
                            reason = "\(r)"
                        }
                    }
                    if let val = d["value"] {
                        let bin = decodeBase64SafeInternal("\(val)")
                        if let r = decodeErrorInfoInternal(bin) {
                            reason = r
                        }
                    }
                    if let dbg = d["debug"] {
                        let bin = decodeBase64SafeInternal("\(dbg)")
                        if let r = decodeErrorInfoInternal(bin) {
                            reason = r
                        }
                    }
                } else if let strDetail = detail as? String {
                    let bin = decodeBase64SafeInternal(strDetail)
                    if let r = decodeErrorInfoInternal(bin) {
                        reason = r
                    }
                }
            }
        }
    }

    if code == "unauthenticated" || status == 401 {
        if reason == nil {
            reason = "unauthenticated"
        }
    }

    return LoamsError(message: message,
                      codeString: code,
                      reason: reason,
                      unknownReason: unknownReason,
                      httpStatus: status,
                      rawDetails: rawDetails)
}
