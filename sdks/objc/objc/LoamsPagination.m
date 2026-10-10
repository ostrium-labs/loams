#import "include/LoamsPagination.h"

@implementation LoamsPagination

+ (NSArray *)iterateWithFetcher:(NSDictionary * _Nullable (^)(NSString *pageToken))fetcher {
    NSMutableArray *items = [NSMutableArray array];
    NSString *pageToken = @"";

    while (YES) {
        NSDictionary *page = fetcher(pageToken);
        if (!page) break;

        NSArray *pageItems = page[@"items"];
        if ([pageItems isKindOfClass:[NSArray class]]) {
            [items addObjectsFromArray:pageItems];
        }

        NSString *next = page[@"nextPageToken"];
        if (!next || next.length == 0 || [next isEqualToString:pageToken]) {
            break;
        }
        pageToken = next;
    }

    return [items copy];
}

@end
