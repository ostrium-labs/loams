#import <Foundation/Foundation.h>

@interface LoamsFrame : NSObject
@property (nonatomic, assign, readonly) uint8_t flags;
@property (nonatomic, copy, readonly, nonnull) NSData *payload;

- (nonnull instancetype)initWithFlags:(uint8_t)flags payload:(nonnull NSData *)payload;
- (BOOL)isMessage;
- (BOOL)isTrailer;
@end

@interface LoamsEnvelopes : NSObject
+ (nonnull NSData *)packPayload:(nonnull NSData *)payload flags:(uint8_t)flags;
+ (nonnull NSArray<LoamsFrame *> *)splitData:(nonnull NSData *)data;
@end
