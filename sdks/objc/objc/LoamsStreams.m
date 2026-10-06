#import "include/LoamsStreams.h"
#import "include/LoamsErrors.h"

@implementation LoamsStreamHandle {
    NSArray<LoamsFrame *> *_frames;
    NSMutableArray<NSString *> *_kinds;
    BOOL _closed;
}

- (instancetype)initWithFrames:(NSArray<LoamsFrame *> *)frames cursor:(NSString *)cursor {
    self = [super init];
    if (self) {
        _frames = [frames copy];
        _lastCursor = [cursor copy];
        _kinds = [NSMutableArray array];
        _closed = NO;
    }
    return self;
}

- (NSArray<NSString *> *)frameKinds {
    return [_kinds copy];
}

- (NSArray *)getMessagesWithError:(NSError **)error {
    NSMutableArray *result = [NSMutableArray array];

    for (LoamsFrame *frame in _frames) {
        if (_closed) break;

        if ([frame isTrailer]) {
            [_kinds addObject:@"trailer"];
            id json = [NSJSONSerialization JSONObjectWithData:frame.payload options:0 error:nil];
            if ([json isKindOfClass:[NSDictionary class]] && json[@"error"]) {
                if (error) {
                    *error = [LoamsErrorParser parseWithStatus:400 body:frame.payload];
                }
                return @[];
            }
            continue;
        }

        [_kinds addObject:@"message"];
        id json = [NSJSONSerialization JSONObjectWithData:frame.payload options:0 error:nil];
        if ([json isKindOfClass:[NSDictionary class]]) {
            NSDictionary *dict = (NSDictionary *)json;
            if (dict[@"cursor"]) {
                _lastCursor = [dict[@"cursor"] description];
            }
            if (dict[@"heartbeat"]) {
                [_kinds addObject:@"heartbeat"];
                continue; // filter out heartbeat
            }
            [result addObject:dict];
        } else {
            [result addObject:frame.payload];
        }
    }

    return [result copy];
}

- (void)close {
    _closed = YES;
}

@end
