#import "include/LoamsClient.h"
#import "include/LoamsIdempotency.h"
#import "include/LoamsEnvelopes.h"

@implementation LoamsTransport

- (instancetype)initWithEndpoint:(NSString *)endpoint tokenSource:(id<LoamsTokenSource>)tokenSource maxRetries:(NSInteger)maxRetries {
    self = [super init];
    if (self) {
        _endpoint = [endpoint copy];
        _tokenSource = tokenSource;
        _maxRetries = maxRetries;
    }
    return self;
}

- (NSData *)executePath:(NSString *)path headers:(NSDictionary<NSString *, NSString *> *)headers body:(NSData *)body statusCode:(NSInteger *)statusCode error:(NSError **)error {
    NSString *cleanEndpoint = [_endpoint hasSuffix:@"/"] ? [_endpoint substringToIndex:_endpoint.length - 1] : _endpoint;
    NSString *cleanPath = [path hasPrefix:@"/"] ? [path substringFromIndex:1] : path;
    NSURL *url = [NSURL URLWithString:[NSString stringWithFormat:@"%@/%@", cleanEndpoint, cleanPath]];

    NSMutableURLRequest *req = [NSMutableURLRequest requestWithURL:url];
    [req setHTTPMethod:@"POST"];
    [req setHTTPBody:body];

    for (NSString *k in headers) {
        [req setValue:headers[k] forHTTPHeaderField:k];
    }

    if (_tokenSource) {
        NSString *token = [_tokenSource token];
        if (token.length > 0) {
            [req setValue:[NSString stringWithFormat:@"Bearer %@", token] forHTTPHeaderField:@"Authorization"];
        }
    }

    __block NSData *resultData = nil;
    __block NSInteger code = 0;
    __block NSError *reqError = nil;

    dispatch_semaphore_t sem = dispatch_semaphore_create(0);
    NSURLSessionDataTask *task = [[NSURLSession sharedSession] dataTaskWithRequest:req completionHandler:^(NSData * _Nullable data, NSURLResponse * _Nullable response, NSError * _Nullable err) {
        if ([response isKindOfClass:[NSHTTPURLResponse class]]) {
            code = [(NSHTTPURLResponse *)response statusCode];
        }
        resultData = data;
        reqError = err;
        dispatch_semaphore_signal(sem);
    }];
    [task resume];
    dispatch_semaphore_wait(sem, DISPATCH_TIME_FOREVER);

    if (statusCode) *statusCode = code;
    if (reqError && error) *error = reqError;

    return resultData;
}

- (NSData *)callWithRetryPath:(NSString *)path headers:(NSDictionary<NSString *, NSString *> *)headers body:(NSData *)body isMutation:(BOOL)isMutation idempotencyKey:(NSString *)idempotencyKey error:(NSError **)error {
    NSInteger attempt = 0;
    BOOL refreshed = NO;

    while (YES) {
        attempt++;
        NSMutableDictionary *curHeaders = [headers mutableCopy];
        if (idempotencyKey) {
            curHeaders[@"idempotency-key"] = idempotencyKey;
        }

        NSInteger status = 0;
        NSError *execErr = nil;
        NSData *res = [self executePath:path headers:curHeaders body:body statusCode:&status error:&execErr];

        if (status >= 200 && status < 300 && res) {
            return res;
        }

        LoamsError *parsedErr = [LoamsErrorParser parseWithStatus:status body:res ?: [NSData data]];

        // R1: Token refresh
        if ([parsedErr.reason isEqualToString:@"token_expired"] && !refreshed && _tokenSource) {
            [_tokenSource refreshToken];
            refreshed = YES;
            continue;
        }

        // R2: Retriable status
        BOOL isRetriable = (status == 503 || [parsedErr.codeString isEqualToString:@"unavailable"]);
        if (isRetriable && (idempotencyKey || !isMutation) && attempt < _maxRetries) {
            usleep((useconds_t)(20000 * attempt));
            continue;
        }

        if (error) *error = parsedErr;
        return nil;
    }
}

@end

@implementation LoamsInstanceService {
    LoamsTransport *_transport;
}

- (instancetype)initWithTransport:(LoamsTransport *)transport {
    self = [super init];
    if (self) {
        _transport = transport;
    }
    return self;
}

- (NSDictionary *)getInstance:(NSDictionary *)request error:(NSError **)error {
    NSData *body = [NSJSONSerialization dataWithJSONObject:request options:0 error:nil];
    NSDictionary *headers = @{@"content-type": @"application/json", @"connect-protocol-version": @"1"};
    NSData *res = [_transport callWithRetryPath:@"/loams.instance.v1.InstanceService/GetInstance" headers:headers body:body isMutation:NO idempotencyKey:nil error:error];
    if (!res) return nil;
    return [NSJSONSerialization JSONObjectWithData:res options:0 error:nil];
}

- (NSDictionary *)whoAmIWithError:(NSError **)error {
    NSData *body = [NSData dataWithBytes:"{}" length:2];
    NSDictionary *headers = @{@"content-type": @"application/json", @"connect-protocol-version": @"1"};
    NSData *res = [_transport callWithRetryPath:@"/loams.instance.v1.InstanceService/WhoAmI" headers:headers body:body isMutation:NO idempotencyKey:nil error:error];
    if (!res) return nil;
    return [NSJSONSerialization JSONObjectWithData:res options:0 error:nil];
}

@end

@implementation LoamsLiveService {
    LoamsTransport *_transport;
}

- (instancetype)initWithTransport:(LoamsTransport *)transport {
    self = [super init];
    if (self) {
        _transport = transport;
    }
    return self;
}

- (NSDictionary *)query:(NSDictionary *)request error:(NSError **)error {
    NSData *body = [NSJSONSerialization dataWithJSONObject:request options:0 error:nil];
    NSDictionary *headers = @{@"content-type": @"application/json", @"connect-protocol-version": @"1"};
    NSData *res = [_transport callWithRetryPath:@"/loams.live.v1.LiveService/Query" headers:headers body:body isMutation:NO idempotencyKey:nil error:error];
    if (!res) return nil;
    return [NSJSONSerialization JSONObjectWithData:res options:0 error:nil];
}

- (LoamsStreamHandle *)watchWithError:(NSError **)error {
    NSDictionary *headers = @{@"content-type": @"application/connect+json", @"connect-protocol-version": @"1"};
    NSData *body = [LoamsEnvelopes packPayload:[NSData dataWithBytes:"{}" length:2] flags:0];
    NSInteger status = 0;
    NSData *res = [_transport executePath:@"/loams.live.v1.LiveService/Watch" headers:headers body:body statusCode:&status error:error];
    if (status != 200) {
        if (error) *error = [LoamsErrorParser parseWithStatus:status body:res ?: [NSData data]];
        return nil;
    }
    NSArray<LoamsFrame *> *frames = [LoamsEnvelopes splitData:res ?: [NSData data]];
    return [[LoamsStreamHandle alloc] initWithFrames:frames cursor:nil];
}

@end

@implementation LoamsApprovalService {
    LoamsTransport *_transport;
}

- (instancetype)initWithTransport:(LoamsTransport *)transport {
    self = [super init];
    if (self) {
        _transport = transport;
    }
    return self;
}

- (NSDictionary *)decideApproval:(NSDictionary *)request idempotencyKey:(NSString *)idempotencyKey error:(NSError **)error {
    NSString *key = idempotencyKey ?: request[@"idempotencyKey"] ?: request[@"idempotency_key"] ?: [LoamsIdempotency mintKey];
    NSMutableDictionary *payload = [request mutableCopy];
    payload[@"idempotencyKey"] = key;

    NSData *body = [NSJSONSerialization dataWithJSONObject:payload options:0 error:nil];
    NSDictionary *headers = @{@"content-type": @"application/json", @"connect-protocol-version": @"1"};
    NSData *res = [_transport callWithRetryPath:@"/loams.approvals.v1.ApprovalService/DecideApproval" headers:headers body:body isMutation:YES idempotencyKey:key error:error];
    if (!res) return nil;
    return [NSJSONSerialization JSONObjectWithData:res options:0 error:nil];
}

- (LoamsStreamHandle *)watchApprovalsWithCursor:(NSString *)cursor error:(NSError **)error {
    NSDictionary *headers = @{@"content-type": @"application/connect+json", @"connect-protocol-version": @"1"};
    NSDictionary *reqObj = cursor ? @{@"cursor": cursor} : @{};
    NSData *jsonBytes = [NSJSONSerialization dataWithJSONObject:reqObj options:0 error:nil];
    NSData *body = [LoamsEnvelopes packPayload:jsonBytes flags:0];

    NSInteger status = 0;
    NSData *res = [_transport executePath:@"/loams.approvals.v1.ApprovalService/WatchApprovals" headers:headers body:body statusCode:&status error:error];
    if (status != 200) {
        if (error) *error = [LoamsErrorParser parseWithStatus:status body:res ?: [NSData data]];
        return nil;
    }
    NSArray<LoamsFrame *> *frames = [LoamsEnvelopes splitData:res ?: [NSData data]];
    return [[LoamsStreamHandle alloc] initWithFrames:frames cursor:cursor];
}

- (NSDictionary *)listApprovals:(NSDictionary *)request error:(NSError **)error {
    NSData *body = [NSJSONSerialization dataWithJSONObject:request options:0 error:nil];
    NSDictionary *headers = @{@"content-type": @"application/json", @"connect-protocol-version": @"1"};
    NSData *res = [_transport callWithRetryPath:@"/loams.approvals.v1.ApprovalService/ListApprovals" headers:headers body:body isMutation:NO idempotencyKey:nil error:error];
    if (!res) return nil;
    return [NSJSONSerialization JSONObjectWithData:res options:0 error:nil];
}

@end

@implementation LoamsDeviceService {
    LoamsTransport *_transport;
}

- (instancetype)initWithTransport:(LoamsTransport *)transport {
    self = [super init];
    if (self) {
        _transport = transport;
    }
    return self;
}

- (NSDictionary *)sendTestNotification:(NSDictionary *)request error:(NSError **)error {
    NSData *body = [NSJSONSerialization dataWithJSONObject:request options:0 error:nil];
    NSDictionary *headers = @{@"content-type": @"application/json", @"connect-protocol-version": @"1"};
    NSData *res = [_transport callWithRetryPath:@"/loams.devices.v1.DeviceService/SendTestNotification" headers:headers body:body isMutation:NO idempotencyKey:nil error:error];
    if (!res) return nil;
    return [NSJSONSerialization JSONObjectWithData:res options:0 error:nil];
}

@end

@implementation LoamsClient

- (instancetype)initWithEndpoint:(NSString *)endpoint tokenSource:(id<LoamsTokenSource>)tokenSource maxRetries:(NSInteger)maxRetries {
    self = [super init];
    if (self) {
        _transport = [[LoamsTransport alloc] initWithEndpoint:endpoint tokenSource:tokenSource maxRetries:maxRetries];
        _instances = [[LoamsInstanceService alloc] initWithTransport:_transport];
        _live = [[LoamsLiveService alloc] initWithTransport:_transport];
        _approvals = [[LoamsApprovalService alloc] initWithTransport:_transport];
        _devices = [[LoamsDeviceService alloc] initWithTransport:_transport];
    }
    return self;
}

- (instancetype)initWithEndpoint:(NSString *)endpoint {
    return [self initWithEndpoint:endpoint tokenSource:nil maxRetries:3];
}

@end
