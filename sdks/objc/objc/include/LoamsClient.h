#import <Foundation/Foundation.h>
#import "LoamsErrors.h"
#import "LoamsTokenSource.h"
#import "LoamsStreams.h"

@interface LoamsTransport : NSObject
@property (nonatomic, copy, readonly, nonnull) NSString *endpoint;
@property (nonatomic, strong, nullable) id<LoamsTokenSource> tokenSource;
@property (nonatomic, assign) NSInteger maxRetries;

- (nonnull instancetype)initWithEndpoint:(nonnull NSString *)endpoint
                             tokenSource:(nullable id<LoamsTokenSource>)tokenSource
                              maxRetries:(NSInteger)maxRetries;

- (nullable NSData *)executePath:(nonnull NSString *)path
                         headers:(nonnull NSDictionary<NSString *, NSString *> *)headers
                            body:(nonnull NSData *)body
                      statusCode:(nullable NSInteger *)statusCode
                           error:(NSError * _Nullable * _Nullable)error;

- (nullable NSData *)callWithRetryPath:(nonnull NSString *)path
                               headers:(nonnull NSDictionary<NSString *, NSString *> *)headers
                                  body:(nonnull NSData *)body
                            isMutation:(BOOL)isMutation
                        idempotencyKey:(nullable NSString *)idempotencyKey
                                 error:(NSError * _Nullable * _Nullable)error;
@end

@interface LoamsInstanceService : NSObject
- (nonnull instancetype)initWithTransport:(nonnull LoamsTransport *)transport;
- (nullable NSDictionary *)getInstance:(nonnull NSDictionary *)request error:(NSError * _Nullable * _Nullable)error;
- (nullable NSDictionary *)whoAmIWithError:(NSError * _Nullable * _Nullable)error;
@end

@interface LoamsLiveService : NSObject
- (nonnull instancetype)initWithTransport:(nonnull LoamsTransport *)transport;
- (nullable NSDictionary *)query:(nonnull NSDictionary *)request error:(NSError * _Nullable * _Nullable)error;
- (nullable LoamsStreamHandle *)watchWithError:(NSError * _Nullable * _Nullable)error;
@end

@interface LoamsApprovalService : NSObject
- (nonnull instancetype)initWithTransport:(nonnull LoamsTransport *)transport;
- (nullable NSDictionary *)decideApproval:(nonnull NSDictionary *)request
                           idempotencyKey:(nullable NSString *)idempotencyKey
                                    error:(NSError * _Nullable * _Nullable)error;
- (nullable LoamsStreamHandle *)watchApprovalsWithCursor:(nullable NSString *)cursor
                                                   error:(NSError * _Nullable * _Nullable)error;
- (nullable NSDictionary *)listApprovals:(nonnull NSDictionary *)request error:(NSError * _Nullable * _Nullable)error;
@end

@interface LoamsDeviceService : NSObject
- (nonnull instancetype)initWithTransport:(nonnull LoamsTransport *)transport;
- (nullable NSDictionary *)sendTestNotification:(nonnull NSDictionary *)request error:(NSError * _Nullable * _Nullable)error;
@end

@interface LoamsClient : NSObject
@property (nonatomic, strong, readonly, nonnull) LoamsTransport *transport;
@property (nonatomic, strong, readonly, nonnull) LoamsInstanceService *instances;
@property (nonatomic, strong, readonly, nonnull) LoamsLiveService *live;
@property (nonatomic, strong, readonly, nonnull) LoamsApprovalService *approvals;
@property (nonatomic, strong, readonly, nonnull) LoamsDeviceService *devices;

- (nonnull instancetype)initWithEndpoint:(nonnull NSString *)endpoint
                             tokenSource:(nullable id<LoamsTokenSource>)tokenSource
                              maxRetries:(NSInteger)maxRetries;
- (nonnull instancetype)initWithEndpoint:(nonnull NSString *)endpoint;
@end
