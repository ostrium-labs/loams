import AuthenticationServices
import Foundation
import LoamsCore
import UIKit

/// OIDC authorization code with PKCE S256 at Authentik through ASWebAuthenticationSession
/// (which does no PKCE of its own). Returns Authentik's access token.
@MainActor
final class WebAuth: NSObject, ASWebAuthenticationPresentationContextProviding {
    /// Held for the duration of a sign-in: the system cancels a session that is deallocated.
    private var session: ASWebAuthenticationSession?

    struct Failure: Error, LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    func signIn(idpIssuer: String, clientID: String) async throws -> String {
        let base = idpIssuer.hasSuffix("/") ? idpIssuer : idpIssuer + "/"
        guard let discoveryURL = URL(string: base + ".well-known/openid-configuration") else {
            throw Failure(message: "The instance's sign-in address is not a URL.")
        }
        let (discovery, _) = try await URLSession.shared.data(from: discoveryURL)
        guard let config = try JSONSerialization.jsonObject(with: discovery) as? [String: Any],
              let authorize = config["authorization_endpoint"] as? String,
              let tokenEndpoint = config["token_endpoint"] as? String,
              var components = URLComponents(string: authorize),
              let tokenURL = URL(string: tokenEndpoint)
        else { throw Failure(message: "Authentik's discovery document is incomplete.") }

        let pkce = Pkce.generate()
        let state = UUID().uuidString
        components.queryItems = [
            .init(name: "response_type", value: "code"),
            .init(name: "client_id", value: clientID),
            .init(name: "redirect_uri", value: Loams.redirectURI),
            .init(name: "scope", value: "openid profile email offline_access"),
            .init(name: "state", value: state),
            .init(name: "code_challenge", value: pkce.challenge),
            .init(name: "code_challenge_method", value: pkce.method),
        ]
        guard let authorizeURL = components.url else { throw Failure(message: "Could not build the sign-in address.") }
        let callback: URL = try await withCheckedThrowingContinuation { continuation in
            let session = ASWebAuthenticationSession(url: authorizeURL, callbackURLScheme: Loams.callbackScheme) { url, error in
                if let url { continuation.resume(returning: url) } else { continuation.resume(throwing: error ?? Failure(message: "Sign-in was cancelled.")) }
            }
            // Keep Authentik's session cookie between sign-ins (AP3 Task 4).
            session.prefersEphemeralWebBrowserSession = false
            session.presentationContextProvider = self
            self.session = session
            session.start()
        }
        session = nil
        let items = URLComponents(url: callback, resolvingAgainstBaseURL: false)?.queryItems ?? []
        guard items.first(where: { $0.name == "state" })?.value == state, let code = items.first(where: { $0.name == "code" })?.value else {
            throw Failure(message: "The sign-in answer did not match this request.")
        }

        var request = URLRequest(url: tokenURL)
        request.httpMethod = "POST"
        request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        var form = URLComponents()
        form.queryItems = [
            .init(name: "grant_type", value: "authorization_code"),
            .init(name: "code", value: code),
            .init(name: "redirect_uri", value: Loams.redirectURI),
            .init(name: "client_id", value: clientID),
            .init(name: "code_verifier", value: pkce.verifier),
        ]
        request.httpBody = Data((form.percentEncodedQuery ?? "").utf8)
        let (data, _) = try await URLSession.shared.data(for: request)
        guard let token = (try JSONSerialization.jsonObject(with: data) as? [String: Any])?["access_token"] as? String else {
            throw Failure(message: "Authentik returned no access token.")
        }
        return token
    }

    nonisolated func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        MainActor.assumeIsolated {
            UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.flatMap(\.windows).first { $0.isKeyWindow } ?? ASPresentationAnchor()
        }
    }
}
