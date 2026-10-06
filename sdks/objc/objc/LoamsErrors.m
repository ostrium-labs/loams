#import "include/LoamsErrors.h"
#import "include/LoamsEnvelopes.h"

NSString * const LoamsErrorDomain = @"dev.loams.error";
NSString * const LoamsErrorReasonKey = @"LoamsErrorReasonKey";
NSString * const LoamsErrorCodeKey = @"LoamsErrorCodeKey";
NSString * const LoamsErrorHTTPStatusKey = @"LoamsErrorHTTPStatusKey";

@implementation LoamsError

- (instancetype)initWithMessage:(NSString *)message
                     codeString:(NSString *)codeString
                         reason:(NSString *)reason
                  unknownReason:(NSString *)unknownReason
                     httpStatus:(NSInteger)httpStatus
                     rawDetails:(NSArray *)rawDetails {
    NSMutableDictionary *userInfo = [NSMutableDictionary dictionary];
    userInfo[NSLocalizedDescriptionKey] = message;
    userInfo[LoamsErrorCodeKey] = codeString;
    if (reason) userInfo[LoamsErrorReasonKey] = reason;
    userInfo[LoamsErrorHTTPStatusKey] = @(httpStatus);

    self = [super initWithDomain:LoamsErrorDomain code:httpStatus userInfo:userInfo];
    if (self) {
        _codeString = [codeString copy];
        _reason = [reason copy];
        _unknownReason = [unknownReason copy];
        _httpStatus = httpStatus;
        _rawDetails = [rawDetails copy];
    }
    return self;
}

@end

@implementation LoamsErrorParser

+ (NSData *)decodeBase64Safe:(NSString *)b64 {
    NSString *clean = [[b64 stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]]
                       stringByReplacingOccurrencesOfString:@"-" withString:@"+"];
    clean = [clean stringByReplacingOccurrencesOfString:@"_" withString:@"/"];
    NSUInteger pad = clean.length % 4;
    if (pad > 0) {
        clean = [clean stringByPaddingToLength:clean.length + (4 - pad) withString:@"=" startingAtIndex:0];
    }
    return [[NSData alloc] initWithBase64EncodedString:clean options:NSDataBase64DecodingIgnoreUnknownCharacters] ?: [NSData data];
}

+ (NSString *)decodeErrorInfo:(NSData *)data {
    // Protobuf wire parser: find field 1 (string)
    const uint8_t *bytes = (const uint8_t *)data.bytes;
    NSUInteger len = data.length;
    NSUInteger pos = 0;

    while (pos < len) {
        // read varint tag
        uint64_t tag = 0;
        int shift = 0;
        while (pos < len) {
            uint8_t b = bytes[pos++];
            tag |= (uint64_t)(b & 0x7F) << shift;
            if ((b & 0x80) == 0) break;
            shift += 7;
        }

        uint64_t field = tag >> 3;
        uint32_t wireType = tag & 7;

        if (wireType == 2) {
            // length-delimited
            uint64_t payloadLen = 0;
            shift = 0;
            while (pos < len) {
                uint8_t b = bytes[pos++];
                payloadLen |= (uint64_t)(b & 0x7F) << shift;
                if ((b & 0x80) == 0) break;
                shift += 7;
            }
            if (pos + payloadLen > len) break;
            if (field == 1) {
                NSData *sub = [data subdataWithRange:NSMakeRange(pos, (NSUInteger)payloadLen)];
                return [[NSString alloc] initWithData:sub encoding:NSUTF8StringEncoding];
            }
            pos += payloadLen;
        } else if (wireType == 0) {
            // varint
            while (pos < len && (bytes[pos++] & 0x80) != 0);
        } else if (wireType == 1) {
            pos += 8;
        } else if (wireType == 5) {
            pos += 4;
        } else {
            break;
        }
    }
    return nil;
}

+ (NSString *)extractFromStatusDetailsBin:(NSData *)bin {
    const uint8_t *bytes = (const uint8_t *)bin.bytes;
    NSUInteger len = bin.length;
    NSUInteger pos = 0;

    while (pos < len) {
        uint64_t tag = 0;
        int shift = 0;
        while (pos < len) {
            uint8_t b = bytes[pos++];
            tag |= (uint64_t)(b & 0x7F) << shift;
            if ((b & 0x80) == 0) break;
            shift += 7;
        }

        uint64_t field = tag >> 3;
        uint32_t wireType = tag & 7;

        if (wireType == 2) {
            uint64_t payloadLen = 0;
            shift = 0;
            while (pos < len) {
                uint8_t b = bytes[pos++];
                payloadLen |= (uint64_t)(b & 0x7F) << shift;
                if ((b & 0x80) == 0) break;
                shift += 7;
            }
            if (pos + payloadLen > len) break;
            if (field == 3) { // details: repeated Any
                NSData *anyData = [bin subdataWithRange:NSMakeRange(pos, (NSUInteger)payloadLen)];
                // in Any, field 2 is value
                const uint8_t *anyBytes = (const uint8_t *)anyData.bytes;
                NSUInteger anyLen = anyData.length;
                NSUInteger anyPos = 0;
                while (anyPos < anyLen) {
                    uint64_t aTag = 0;
                    shift = 0;
                    while (anyPos < anyLen) {
                        uint8_t ab = anyBytes[anyPos++];
                        aTag |= (uint64_t)(ab & 0x7F) << shift;
                        if ((ab & 0x80) == 0) break;
                        shift += 7;
                    }
                    uint64_t aField = aTag >> 3;
                    uint32_t aWire = aTag & 7;
                    if (aWire == 2) {
                        uint64_t aPayloadLen = 0;
                        shift = 0;
                        while (anyPos < anyLen) {
                            uint8_t ab = anyBytes[anyPos++];
                            aPayloadLen |= (uint64_t)(ab & 0x7F) << shift;
                            if ((ab & 0x80) == 0) break;
                            shift += 7;
                        }
                        if (anyPos + aPayloadLen > anyLen) break;
                        if (aField == 2) {
                            NSData *errInfoData = [anyData subdataWithRange:NSMakeRange(anyPos, (NSUInteger)aPayloadLen)];
                            NSString *reason = [self decodeErrorInfo:errInfoData];
                            if (reason) return reason;
                        }
                        anyPos += aPayloadLen;
                    } else if (aWire == 0) {
                        while (anyPos < anyLen && (anyBytes[anyPos++] & 0x80) != 0);
                    } else {
                        break;
                    }
                }
            }
            pos += payloadLen;
        } else if (wireType == 0) {
            while (pos < len && (bytes[pos++] & 0x80) != 0);
        } else {
            break;
        }
    }
    return nil;
}

+ (LoamsError *)parseWithStatus:(NSInteger)status body:(NSData *)bodyData {
    NSString *code = @"unknown";
    NSString *message = [NSString stringWithFormat:@"HTTP %ld", (long)status];
    NSString *reason = nil;
    NSString *unknownReason = nil;
    NSArray *rawDetails = @[];

    NSData *activeData = bodyData;
    if (activeData.length >= 5) {
        NSArray<LoamsFrame *> *frames = [LoamsEnvelopes splitData:activeData];
        for (LoamsFrame *f in frames) {
            if ([f isTrailer]) {
                activeData = f.payload;
                break;
            }
        }
    }

    NSString *str = [[NSString alloc] initWithData:activeData encoding:NSUTF8StringEncoding];
    if (str && [str containsString:@"grpc-status-details-bin:"]) {
        NSRange r = [str rangeOfString:@"grpc-status-details-bin:\\s*([^\\r\\n]+)" options:NSRegularExpressionSearch];
        if (r.location != NSNotFound) {
            NSString *matched = [str substringWithRange:r];
            NSArray *parts = [matched componentsSeparatedByString:@":"];
            if (parts.count >= 2) {
                NSString *b64 = [parts[1] stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceCharacterSet]];
                NSData *bin = [self decodeBase64Safe:b64];
                NSString *extracted = [self extractFromStatusDetailsBin:bin];
                if (extracted) reason = extracted;
            }
        }
    }

    id json = [NSJSONSerialization JSONObjectWithData:activeData options:0 error:nil];
    if ([json isKindOfClass:[NSDictionary class]]) {
        NSDictionary *dict = (NSDictionary *)json;
        if ([dict[@"error"] isKindOfClass:[NSDictionary class]]) {
            dict = dict[@"error"];
        }
        if (dict[@"code"]) code = [dict[@"code"] description];
        if (dict[@"message"]) message = [dict[@"message"] description];
        if ([dict[@"details"] isKindOfClass:[NSArray class]]) {
            rawDetails = dict[@"details"];
            for (id detail in rawDetails) {
                if ([detail isKindOfClass:[NSDictionary class]]) {
                    NSDictionary *d = (NSDictionary *)detail;
                    NSString *type = [d[@"@type"] ?: d[@"type"] description];
                    if ([type hasSuffix:@"ErrorInfo"]) {
                        if (d[@"reason"]) {
                            reason = [d[@"reason"] description];
                        }
                    }
                    if (d[@"value"]) {
                        NSData *bin = [self decodeBase64Safe:[d[@"value"] description]];
                        NSString *r = [self decodeErrorInfo:bin];
                        if (r) reason = r;
                    }
                    if (d[@"debug"]) {
                        NSData *bin = [self decodeBase64Safe:[d[@"debug"] description]];
                        NSString *r = [self decodeErrorInfo:bin];
                        if (r) reason = r;
                    }
                } else if ([detail isKindOfClass:[NSString class]]) {
                    NSData *bin = [self decodeBase64Safe:(NSString *)detail];
                    NSString *r = [self decodeErrorInfo:bin];
                    if (r) reason = r;
                }
            }
        }
    }

    if ([code isEqualToString:@"unauthenticated"] || status == 401) {
        if (!reason) reason = @"unauthenticated";
    }

    return [[LoamsError alloc] initWithMessage:message
                                    codeString:code
                                        reason:reason
                                 unknownReason:unknownReason
                                    httpStatus:status
                                    rawDetails:rawDetails];
}

@end
