#import "include/LoamsTokenSource.h"

@implementation LoamsStaticTokenSource {
    NSString *_token;
}

- (instancetype)initWithToken:(NSString *)token {
    self = [super init];
    if (self) {
        _token = [token copy];
    }
    return self;
}

- (NSString *)token {
    return _token;
}

- (void)refreshToken {
}

@end

@implementation LoamsEnvTokenSource {
    NSString *_envVar;
}

- (instancetype)initWithEnvVar:(NSString *)envVar {
    self = [super init];
    if (self) {
        _envVar = [envVar copy];
    }
    return self;
}

- (instancetype)init {
    return [self initWithEnvVar:@"LOAMS_API_KEY"];
}

- (NSString *)token {
    const char *val = getenv([_envVar UTF8String]);
    if (val != NULL) {
        return [NSString stringWithUTF8String:val];
    }
    return [[[NSProcessInfo processInfo] environment] objectForKey:_envVar] ?: @"";
}

- (void)refreshToken {
}

@end

@implementation LoamsRefreshTokenSource {
    LoamsTokenRefresher _refresher;
    NSString *_currentToken;
}

- (instancetype)initWithRefresher:(LoamsTokenRefresher)refresher {
    self = [super init];
    if (self) {
        _refresher = [refresher copy];
    }
    return self;
}

- (NSString *)token {
    if (!_currentToken) {
        [self refreshToken];
    }
    return _currentToken ?: @"";
}

- (void)refreshToken {
    if (_refresher) {
        _currentToken = _refresher();
    }
}

@end
