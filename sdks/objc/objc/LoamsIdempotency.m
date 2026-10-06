#import "include/LoamsIdempotency.h"

@implementation LoamsIdempotency

+ (NSString *)mintKey {
    uint64_t timestamp = (uint64_t)([[NSDate date] timeIntervalSince1970] * 1000.0);
    NSString *timeHex = [NSString stringWithFormat:@"%012llx", (unsigned long long)timestamp];

    NSString *part1 = [timeHex substringWithRange:NSMakeRange(0, 8)];
    NSString *part2 = [timeHex substringWithRange:NSMakeRange(8, 4)];

    uint32_t r1 = arc4random_uniform(0x1000);
    NSString *part3 = [NSString stringWithFormat:@"7%03x", r1];

    uint32_t r2 = arc4random_uniform(0x4000) | 0x8000;
    NSString *part4 = [NSString stringWithFormat:@"%04x", r2];

    uint32_t r3 = arc4random_uniform(0x1000000);
    uint32_t r4 = arc4random_uniform(0x1000000);
    NSString *part5 = [NSString stringWithFormat:@"%06x%06x", r3, r4];

    return [NSString stringWithFormat:@"%@-%@-%@-%@-%@", part1, part2, part3, part4, part5];
}

@end
