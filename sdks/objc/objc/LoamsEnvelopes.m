#import "include/LoamsEnvelopes.h"

@implementation LoamsFrame

- (instancetype)initWithFlags:(uint8_t)flags payload:(NSData *)payload {
    self = [super init];
    if (self) {
        _flags = flags;
        _payload = [payload copy];
    }
    return self;
}

- (BOOL)isMessage {
    return _flags == 0;
}

- (BOOL)isTrailer {
    return (_flags & 0x02) != 0 || (_flags & 0x80) != 0;
}

@end

@implementation LoamsEnvelopes

+ (NSData *)packPayload:(NSData *)payload flags:(uint8_t)flags {
    NSMutableData *data = [NSMutableData dataWithCapacity:5 + payload.length];
    [data appendBytes:&flags length:1];
    uint32_t len = CFSwapInt32HostToBig((uint32_t)payload.length);
    [data appendBytes:&len length:4];
    [data appendData:payload];
    return [data copy];
}

+ (NSArray<LoamsFrame *> *)splitData:(NSData *)data {
    NSMutableArray<LoamsFrame *> *frames = [NSMutableArray array];
    const uint8_t *bytes = (const uint8_t *)data.bytes;
    NSUInteger totalLen = data.length;
    NSUInteger pos = 0;

    while (pos < totalLen) {
        if (pos + 5 > totalLen) break;
        uint8_t flags = bytes[pos];
        uint32_t rawLen = *(const uint32_t *)(bytes + pos + 1);
        uint32_t payloadLen = CFSwapInt32BigToHost(rawLen);
        pos += 5;

        if (pos + payloadLen > totalLen) break;
        NSData *payload = [data subdataWithRange:NSMakeRange(pos, payloadLen)];
        pos += payloadLen;

        [frames addObject:[[LoamsFrame alloc] initWithFlags:flags payload:payload]];
    }

    return [frames copy];
}

@end
