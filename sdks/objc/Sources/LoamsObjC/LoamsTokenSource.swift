import Foundation

public typealias LoamsTokenRefresher = () -> String

public protocol LoamsTokenSource: AnyObject {
    func token() -> String
    func refreshToken()
}

#if canImport(ObjectiveC)
@objcMembers
public class LoamsStaticTokenSource: NSObject, LoamsTokenSource {
    private let tokenString: String

    public init(token: String) {
        self.tokenString = token
        super.init()
    }

    public func token() -> String {
        return tokenString
    }

    public func refreshToken() {}
}

@objcMembers
public class LoamsEnvTokenSource: NSObject, LoamsTokenSource {
    private let envVar: String

    public init(envVar: String = "LOAMS_API_KEY") {
        self.envVar = envVar
        super.init()
    }

    public func token() -> String {
        if let val = ProcessInfo.processInfo.environment[envVar] {
            return val
        }
        return ""
    }

    public func refreshToken() {}
}

@objcMembers
public class LoamsRefreshTokenSource: NSObject, LoamsTokenSource {
    private let refresher: LoamsTokenRefresher
    private var currentToken: String?

    public init(refresher: @escaping LoamsTokenRefresher) {
        self.refresher = refresher
        super.init()
    }

    public func token() -> String {
        if currentToken == nil {
            refreshToken()
        }
        return currentToken ?? ""
    }

    public func refreshToken() {
        currentToken = refresher()
    }
}
#else
public class LoamsStaticTokenSource: NSObject, LoamsTokenSource {
    private let tokenString: String

    public init(token: String) {
        self.tokenString = token
        super.init()
    }

    public func token() -> String {
        return tokenString
    }

    public func refreshToken() {}
}

public class LoamsEnvTokenSource: NSObject, LoamsTokenSource {
    private let envVar: String

    public init(envVar: String = "LOAMS_API_KEY") {
        self.envVar = envVar
        super.init()
    }

    public func token() -> String {
        if let val = ProcessInfo.processInfo.environment[envVar] {
            return val
        }
        return ""
    }

    public func refreshToken() {}
}

public class LoamsRefreshTokenSource: NSObject, LoamsTokenSource {
    private let refresher: LoamsTokenRefresher
    private var currentToken: String?

    public init(refresher: @escaping LoamsTokenRefresher) {
        self.refresher = refresher
        super.init()
    }

    public func token() -> String {
        if currentToken == nil {
            refreshToken()
        }
        return currentToken ?? ""
    }

    public func refreshToken() {
        currentToken = refresher()
    }
}
#endif
