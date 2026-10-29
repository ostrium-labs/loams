// A notes app on Loams from Objective-C: a namespace, a collection with a
// schema, documents and a hybrid search with a filter.
//
//   LOAMS_ENDPOINT=http://127.0.0.1:8080 LOAMS_NAMESPACE=app ./notes
//
// Built against the `LoamsObjC` module (CocoaPods `Loams`); every Swift
// `throws` call is an Objective-C method with a trailing `error:`.

@import Foundation;
@import LoamsObjC;

int main(void) {
  @autoreleasepool {
    LoamsClientConfiguration *config = [LoamsClientConfiguration fromEnvironment];
    if (config == nil) {
      NSLog(@"LOAMS_ENDPOINT is not set");
      return 1;
    }
    config.sessionConsistency = YES;  // reads see this client's writes
    LoamsClient *client = [[LoamsClient alloc] initWithConfiguration:config];
    NSError *error = nil;

    [client.namespaces create:config.namespace options:nil error:&error];  // already_exists on a rerun
    error = nil;
    LoamsSchema *schema = [[[[[[[LoamsSchema alloc] init] keyword:@"title"] text:@"body"] keyword:@"tags"] i64:@"n"]
        vector:@"embedding" dim:4 distance:@"cosine"];
    [client.collections create:@"notes" schema:schema options:nil error:&error];
    error = nil;

    LoamsCollectionHandle *notes = [client collection:@"notes" namespace:nil];
    NSArray<LoamsDocument *> *docs = @[
      [[LoamsDocument alloc] initWithId:@1
                                 source:@{@"title" : @"Refunds", @"body" : @"refund policy for orders", @"tags" : @[ @"policy" ], @"n" : @1}
                                vectors:@{@"embedding" : @[ @1, @0, @0, @0 ]}],
      [[LoamsDocument alloc] initWithId:@2
                                 source:@{@"title" : @"Shipping", @"body" : @"shipping times", @"tags" : @[ @"ops" ], @"n" : @2}
                                vectors:@{@"embedding" : @[ @0, @1, @0, @0 ]}],
    ];
    if (![notes upsert:docs options:nil error:&error]) {
      NSLog(@"upsert failed: %@", error);
      return 1;
    }

    // k is explicit so the example also runs on a 0.1.0 engine, which has no default k.
    LoamsSearch *search = [[[[[LoamsSearch on:@"notes"] text:@"refund" fields:@[ @"body" ] k:@10]
        vector:@"embedding" values:@[ @1, @0, @0, @0 ] k:@10 exact:nil nprobes:nil refineFactor:nil ef:nil
              oversampling:nil distance:nil filter:nil]
        fuse:[LoamsFusion rrf:@60]] filter:[LoamsFilter eq:@"tags" value:@"policy"]];
    NSDictionary *page = [notes search:[search limit:5] options:nil error:&error];
    for (NSDictionary *hit in page[@"hits"]) {
      NSLog(@"%@: %@", hit[@"pk"], hit[@"source"][@"title"]);
    }

    NSDictionary *nearest = [client.vector search:@"notes" vector:@[@1, @0, @0, @0]
        field:@"embedding" limit:3 filter:nil options:nil error:&error];
    if (error != nil) {
      NSLog(@"vector search failed: %@", error);
      return 1;
    }
    NSLog(@"vector hits: %lu", (unsigned long)[nearest[@"hits"] count]);

    NSDictionary *sql = [client.sql query:@"SELECT title FROM notes WHERE n >= $1 ORDER BY n"
        params:@[@2] maxRows:nil consistency:nil options:nil error:&error];
    if (error != nil) {
      NSLog(@"SQL failed: %@", error);
      return 1;
    }
    NSLog(@"SQL rows: %@", sql[@"rows"]);

    [client.streams create:@"notes_events" partitions:1 options:nil error:&error];
    NSDictionary *produced = [client.streams produce:@"notes_events"
        records:@[@{@"kind" : @"created", @"note" : @1}]
        partition:0 producerId:@"objc-notes" producerSeq:@1 options:nil error:&error];
    if (error != nil) {
      NSLog(@"stream produce failed: %@", error);
      return 1;
    }
    uint64_t start = [produced[@"baseOffset"] respondsToSelector:@selector(unsignedLongLongValue)]
        ? [produced[@"baseOffset"] unsignedLongLongValue] : 0;
    LoamsSubscription *events = [client.streams subscribe:@"notes_events" partition:0
        fromOffset:start maxRecords:@1 maxWaitMs:@100 options:nil];
    NSDictionary *event = [events next];
    NSData *eventBytes = event[@"value"];
    if (eventBytes != nil) {
      NSLog(@"stream event at offset %@: %@", event[@"offset"],
            [[NSString alloc] initWithData:eventBytes encoding:NSUTF8StringEncoding]);
    }
  }
  return 0;
}
