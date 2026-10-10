#import <Foundation/Foundation.h>

extern NSString * _Nonnull const LoamsErrorDomain;
extern NSString * _Nonnull const LoamsErrorReasonKey;
extern NSString * _Nonnull const LoamsErrorCodeKey;
extern NSString * _Nonnull const LoamsErrorHTTPStatusKey;

@interface LoamsError : NSError

@property (nonatomic, copy, readonly, nonnull) NSString *codeString;
@property (nonatomic, copy, readonly, nullable) NSString *reason;
@property (nonatomic, copy, readonly, nullable) NSString *unknownReason;
@property (nonatomic, assign, readonly) NSInteger httpStatus;
@property (nonatomic, copy, readonly, nonnull) NSArray *rawDetails;

- (nonnull instancetype)initWithMessage:(nonnull NSString *)message
                             codeString:(nonnull NSString *)codeString
                                 reason:(nullable NSString *)reason
                          unknownReason:(nullable NSString *)unknownReason
                             httpStatus:(NSInteger)httpStatus
                             rawDetails:(nonnull NSArray *)rawDetails;

@end

@interface LoamsErrorParser : NSObject

+ (nonnull NSData *)decodeBase64Safe:(nonnull NSString *)b64;
+ (nullable NSString *)decodeErrorInfo:(nonnull NSData *)data;
+ (nonnull LoamsError *)parseWithStatus:(NSInteger)status body:(nonnull NSData *)body;

@end
