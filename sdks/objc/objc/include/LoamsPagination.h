#import <Foundation/Foundation.h>

@interface LoamsPagination : NSObject
+ (nonnull NSArray *)iterateWithFetcher:(nonnull NSDictionary * _Nullable (^)(NSString * _Nonnull pageToken))fetcher;
@end
