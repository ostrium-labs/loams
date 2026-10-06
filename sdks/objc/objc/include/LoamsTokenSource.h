#import <Foundation/Foundation.h>

@protocol LoamsTokenSource <NSObject>
- (nonnull NSString *)token;
- (void)refreshToken;
@end

@interface LoamsStaticTokenSource : NSObject <LoamsTokenSource>
- (nonnull instancetype)initWithToken:(nonnull NSString *)token;
@end

@interface LoamsEnvTokenSource : NSObject <LoamsTokenSource>
- (nonnull instancetype)initWithEnvVar:(nonnull NSString *)envVar;
- (nonnull instancetype)init;
@end

typedef NSString * _Nonnull (^LoamsTokenRefresher)(void);

@interface LoamsRefreshTokenSource : NSObject <LoamsTokenSource>
- (nonnull instancetype)initWithRefresher:(nonnull LoamsTokenRefresher)refresher;
@end
