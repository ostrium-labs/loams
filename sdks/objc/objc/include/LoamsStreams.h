#import <Foundation/Foundation.h>
#import "LoamsEnvelopes.h"

@interface LoamsStreamHandle : NSObject
@property (nonatomic, copy, readonly, nonnull) NSArray<NSString *> *frameKinds;
@property (nonatomic, copy, nullable) NSString *lastCursor;

- (nonnull instancetype)initWithFrames:(nonnull NSArray<LoamsFrame *> *)frames cursor:(nullable NSString *)cursor;
- (nonnull NSArray *)getMessagesWithError:(NSError * _Nullable * _Nullable)error;
- (void)close;
@end
