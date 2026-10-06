// The one client object (design §44 §7.1, D606).
//
// One object with namespaced modules, in C++'s `PascalCase` per design §44 §7.1:
//
//     auto loams = loams::Client::Make({.endpoint = "http://127.0.0.1:8080"});
//     loams::instance::v1::GetInstanceResponse info;
//     loams->Instance()->GetInstance(&info);
//
// Every module method goes through `Client::Invoke`, which is the whole runtime:
// auth (R1), one idempotency key per logical call reused on retry (R3), the
// retry policy from the generated bindings (R2), and the typed error mapping
// (R8). Nothing in a module re-implements any of that, which is what makes the
// conformance suite's claim that it exercises "the public facade" true.

#ifndef LOAMS_CLIENT_HPP
#define LOAMS_CLIENT_HPP

#include <chrono>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include <google/protobuf/message.h>

#include "loams/call.hpp"
#include "loams/consistency.hpp"
#include "loams/http.hpp"
#include "loams/retry.hpp"
#include "loams/stream.hpp"
#include "loams/system.hpp"
#include "loams/token_source.hpp"
#include "loams/wire.hpp"

namespace loams {

/// How a client is built.
struct Options {
  /// The instance's base URL, `http://host:port` or `https://host`. A trailing
  /// slash is trimmed, because `"/rpc"` and `"//rpc"` are two different paths and
  /// only one of them is recorded.
  std::string endpoint;
  /// Where the bearer comes from. Null means unauthenticated, which is what
  /// `GetInstance` needs and what keeps a bearer out of the conformance suite.
  std::shared_ptr<TokenSource> token_source;
  /// The HTTP layer. Null means libcurl.
  std::shared_ptr<HttpTransport> transport;
  /// One attempt's deadline. Every outbound call has one; a client that can wait
  /// forever is a client whose thread never comes back.
  std::chrono::milliseconds timeout{20000};
  /// The retries after the first attempt. `0` disables retries entirely, which
  /// is a supported setting and not the same as "use the default".
  int max_retries = kDefaultMaxRetries;
  /// Turn the session consistency store on. **Off by default** (R4): a caller
  /// who has not asked for read-your-writes should not get them.
  bool session_consistency = false;
  /// The encoding to send. `kProto` is the default, because an SDK sends binary
  /// and `curl` is what sends JSON; a caller can pick any of the four and the
  /// error mapping follows the choice.
  ContentType content_type = ContentType::kProto;
  /// Extra headers on every request, for a header a proxy needs.
  std::vector<std::pair<std::string, std::string>> headers;
};

/// One Loams instance, reached over Connect.
class Client {
 public:
  /// Builds a client. Throws `loams::LoamsError` (`internal`) when the endpoint
  /// is empty or has no scheme: a client with no address fails on its first call
  /// otherwise, with a message that says nothing about the real cause.
  static std::shared_ptr<Client> Make(Options options);

  ~Client();
  Client(const Client&) = delete;
  Client& operator=(const Client&) = delete;

  /// The instance this client talks to, without a trailing slash.
  const std::string& Endpoint() const;

  /// The proto revision this SDK was generated from (R9).
  const std::string& ProtoRev() const;

  /// The content type every call sends. Named `Encoding` rather than
  /// `ContentType` because a member function named after the type it returns
  /// changes the meaning of `ContentType` inside the class body, which clang
  /// rejects outright and which is a trap for a reader besides.
  ContentType Encoding() const;

  /// The consistency token the session store holds, attaching to later reads.
  /// Always absent unless `Options::session_consistency` is on (R4).
  ConsistencyToken SessionToken() const;

  // --- The runtime -------------------------------------------------------------------------------

  /// The whole unary call path. `attempt` performs one try and fills in the
  /// response; `Invoke` wraps it in R3's key, R2's retry and R1's one refresh.
  ///
  /// **`attempt` must not interpret the response.** Deciding whether a response is
  /// a failure is `Invoke`'s job and it is the whole of R8: a Connect error
  /// arrives as HTTP 501 with a JSON body that is not the response message, so an
  /// `attempt` that parsed the body would throw a parse error instead of the
  /// `UnimplementedError` the caller needs, and would report the wrong thing for
  /// every failure. `on_success` runs only once `Invoke` has decided the response
  /// is a success.
  ///
  /// Throws the typed error the failure maps to.
  void Invoke(const CallPlan& plan, HttpTransport& transport,
              const std::function<void(const HttpRequest&, HttpResponse&)>& attempt,
              const std::function<void(const HttpResponse&)>& on_success);

  /// Marshals a request, mints R3's key when the binding declares the field, and
  /// calls `Invoke`. On return `response` holds the parsed response.
  ///
  /// This is what every module method calls. `idempotency_key` is the caller's
  /// own key, or empty to have one minted.
  void Unary(const MethodBinding& binding, const google::protobuf::Message& request,
             google::protobuf::Message* response, ContentType content_type,
             const std::string& idempotency_key = "",
             const std::vector<std::pair<std::string, std::string>>& headers = {});

  /// Opens a server stream.
  ///
  /// `resume` is required for the stream to re-open on a retryable failure; with
  /// it null a broken stream is **reported** rather than spun on, which is what
  /// an `unimplemented` stream needs (R7).
  MessageStream OpenStream(const MethodBinding& binding, const google::protobuf::Message& request,
                           ContentType content_type, std::shared_ptr<StreamResume> resume,
                           const std::vector<std::pair<std::string, std::string>>& headers = {});

  /// The bytes a call to this RPC would send: the message in this encoding,
  /// framed when the encoding frames.
  ///
  /// Exposed because a caller building a raw call — a conformance suite driving
  /// the corpus, an application reaching an RPC the facade does not serve — must
  /// encode through **this** encoder and not one of its own. The fixture server
  /// compares the request it is sent against the recorded one byte for byte, so a
  /// second encoder would make that comparison a test of the second encoder.
  std::string EncodeBody(const google::protobuf::Message& request, ContentType content_type) const;

  /// R1's one refresh, applied where it belongs. Public because a caller with its
  /// own retry loop needs it and because `cpp_token_source_refresh` drives it
  /// directly. Returns whether a refresh happened.
  bool RefreshOnce();

  /// The transport this client uses. Exposed so a caller can share one libcurl
  /// handle pool across clients, and so the conformance suite can assert on what
  /// a request looked like.
  HttpTransport& Transport();

  /// The token source, or null when the client is unauthenticated.
  const std::shared_ptr<TokenSource>& Token() const;

  /// Feature detection and the version report (R5, R9). Named `SystemModule`
  /// for the same reason `Encoding` is: a member named after the type it returns
  /// changes that type's meaning inside the class.
  SystemModule* System();

 private:
  friend class System;
  struct Impl;
  explicit Client(Options options);
  std::unique_ptr<Impl> impl_;
};

}  // namespace loams

#endif  // LOAMS_CLIENT_HPP